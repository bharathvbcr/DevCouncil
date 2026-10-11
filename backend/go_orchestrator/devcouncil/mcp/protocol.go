package mcp

import (
	"encoding/json"
	"os"
	"slices"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/version"
)

// Two protocol eras, one server.
//
// The legacy era (2025-11-25 and earlier) opens with an `initialize`
// handshake that negotiates a revision for the connection. The modern era
// (2026-07-28) has no handshake: every request carries its own revision in
// `_meta`, and `server/discover` reports what the server speaks. A server that
// serves both selects by how the client opens — per-request `_meta` is modern,
// `initialize` is legacy — which is the rule the specification gives a
// dual-era server and the one `devmap mcp` already follows
// (rust/devmap-serve/src/mcp.rs). The shared transcript
// protocols/mcp/negotiation.v1.json holds both to the same answers.
const (
	// DefaultProtocolEnv overrides Server.DefaultProtocol.
	DefaultProtocolEnv = "DEVCOUNCIL_MCP_PROTOCOL_DEFAULT"

	// DefaultLegacy answers an `initialize` that names no revision this
	// server supports with the newest handshake revision. It is the default:
	// every host that handshakes today (Claude Code, Cursor, Codex) is legacy,
	// and counter-offering is what the specification asks of a server.
	DefaultLegacy = "legacy"
	// DefaultModern refuses such an `initialize` with the revisions the
	// server speaks instead of counter-offering a handshake revision, for
	// deployments that want legacy clients to see an actionable error rather
	// than silently negotiate down. A client that names a supported handshake
	// revision is still served it.
	DefaultModern = "modern"

	metaProtocolVersion     = "io.modelcontextprotocol/protocolVersion"
	metaClientCapabilities  = "io.modelcontextprotocol/clientCapabilities"
	metaServerInfo          = "io.modelcontextprotocol/serverInfo"
	codeUnsupportedProtocol = -32022
	codeInvalidParams       = -32602
	cacheTTLMs              = 5 * 60 * 1000
)

// handshakeVersions are the legacy revisions this server negotiates, oldest
// first; the last is the newest.
var handshakeVersions = []string{"2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"}

// modernVersions are the revisions served per-request.
var modernVersions = []string{"2026-07-28"}

// resolveDefault turns the configured default into either a handshake
// revision to counter-offer or DefaultModern. Anything unrecognised is
// DefaultLegacy: a typo in an environment variable must not change the era a
// fleet of hosts negotiates.
func (s *Server) resolveDefault() string {
	def := s.DefaultProtocol
	if def == "" {
		def = os.Getenv(DefaultProtocolEnv)
	}
	def = strings.TrimSpace(def)
	switch {
	case def == DefaultModern:
		return DefaultModern
	case slices.Contains(handshakeVersions, def):
		return def
	}
	return handshakeVersions[len(handshakeVersions)-1]
}

// negotiate picks the legacy revision for an `initialize`: the client's own
// when this server supports it, else the configured default. ok is false when
// the default is DefaultModern and the client named nothing supported.
func (s *Server) negotiate(asked string) (revision string, ok bool) {
	if slices.Contains(handshakeVersions, asked) {
		return asked, true
	}
	def := s.resolveDefault()
	if def == DefaultModern {
		return "", false
	}
	return def, true
}

func serverInfo() map[string]any {
	return map[string]any{"name": "devcouncil", "version": version.Version}
}

// eraOf reads the era off a request. A request carrying `_meta` with a
// revision is modern and must state a revision this server serves and the
// client capabilities; anything else is legacy.
func eraOf(req rpcRequest) (modern bool, refusal *rpcError) {
	var params struct {
		Meta map[string]json.RawMessage `json:"_meta"`
	}
	if len(req.Params) == 0 || json.Unmarshal(req.Params, &params) != nil {
		return false, nil
	}
	raw, present := params.Meta[metaProtocolVersion]
	if !present {
		return false, nil
	}
	var stated string
	if err := json.Unmarshal(raw, &stated); err != nil {
		return true, &rpcError{Code: codeInvalidParams, Message: "_meta." + metaProtocolVersion + " must be a version string such as \"" + modernVersions[0] + "\""}
	}
	if !slices.Contains(modernVersions, stated) {
		msg := "this server serves " + strings.Join(modernVersions, ", ") + " through per-request _meta; the request declared " + stated + "."
		if slices.Contains(handshakeVersions, stated) {
			msg += " " + stated + " is a handshake revision: it is reached by sending `initialize` with no per-request protocol version."
		}
		return true, &rpcError{Code: codeUnsupportedProtocol, Message: msg,
			Data: map[string]any{"supported": modernVersions, "requested": stated}}
	}
	if _, ok := params.Meta[metaClientCapabilities]; !ok {
		return true, &rpcError{Code: codeInvalidParams, Message: "_meta." + metaClientCapabilities +
			" is required on every request of this revision; an empty object declares no optional capabilities"}
	}
	return true, nil
}

func (s *Server) handleInitialize(req rpcRequest) rpcResponse {
	var params struct {
		ProtocolVersion string `json:"protocolVersion"`
	}
	if len(req.Params) > 0 {
		_ = json.Unmarshal(req.Params, &params)
	}
	revision, ok := s.negotiate(params.ProtocolVersion)
	if !ok {
		return rpcResponse{JSONRPC: "2.0", ID: req.ID, Error: &rpcError{
			Code: codeInvalidParams,
			Message: "this server is configured to prefer " + strings.Join(modernVersions, ", ") +
				" and does not counter-offer a handshake revision; it also serves " + strings.Join(handshakeVersions, ", ") +
				" to a client that names one. Use `server/discover` for the modern revision.",
			Data: map[string]any{"supported": append(slices.Clone(modernVersions), handshakeVersions...), "requested": params.ProtocolVersion},
		}}
	}
	return okResult(req.ID, map[string]any{
		"protocolVersion": revision,
		"capabilities":    map[string]any{"tools": map[string]any{}},
		// `version.Version`, never a literal: this field is how a host
		// reports which DevCouncil answered it, and a literal here held
		// 0.1.0 from the first release through 0.2.3.
		"serverInfo": serverInfo(),
	})
}

// modernHandle serves one modern-era request.
func (s *Server) modernHandle(req rpcRequest) rpcResponse {
	switch req.Method {
	case "initialize":
		return errResult(req.ID, -32601, "'initialize' is not a method of "+strings.Join(modernVersions, ", ")+
			": that revision replaced the handshake with per-request metadata. Use 'server/discover'. "+
			"To handshake, send 'initialize' without a per-request _meta."+metaProtocolVersion)
	case "server/discover":
		return s.completed(s.discoverResult(req))
	}
	return s.completed(s.legacyHandle(req))
}

func (s *Server) discoverResult(req rpcRequest) rpcResponse {
	return okResult(req.ID, map[string]any{
		"supportedVersions": modernVersions,
		"capabilities":      map[string]any{"tools": map[string]any{}},
		"ttlMs":             cacheTTLMs,
		"cacheScope":        "private",
	})
}

// completed stamps the modern revision's required `resultType` and the
// server identity on a successful result: the modern era keeps no connection
// state, so `_meta` is the only place a client learns which build answered.
func (s *Server) completed(resp rpcResponse) rpcResponse {
	result, ok := resp.Result.(map[string]any)
	if !ok || resp.Error != nil {
		return resp
	}
	if _, set := result["resultType"]; !set {
		result["resultType"] = "complete"
	}
	meta, _ := result["_meta"].(map[string]any)
	if meta == nil {
		meta = map[string]any{}
	}
	if _, set := meta[metaServerInfo]; !set {
		meta[metaServerInfo] = serverInfo()
	}
	result["_meta"] = meta
	return resp
}
