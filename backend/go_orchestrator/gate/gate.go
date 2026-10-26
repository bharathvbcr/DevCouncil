// Package gate composes the three seams a write must pass: the flag registry
// that says how strict the gate is, the policy ladder that decides, and the
// grant ledger that may clear a soft denial.
//
// It is the consumer in the seam rule — it owns none of the three definitions
// and is the only place that knows all three exist.
package gate

import (
	"fmt"
	"sync"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/grants"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// Gate decides whether a file operation may proceed.
type Gate struct {
	Flags  *flags.Registry
	Ledger *grants.Ledger
	Root   string
	// Subsystems may be nil; the policy layer reports that as a degradation.
	Subsystems policy.SubsystemMap
	// GlobalAllowedCommands supplement every task's own command allowlist.
	GlobalAllowedCommands []string
	// Matcher answers every policy decision's pattern questions; nil is
	// fnmatch, which every host uses (docs/gusset-candidates.md). A matcher
	// that can fail turns its error into a hard denial under the engine
	// rules.
	Matcher policy.Matcher
	// OnIssue is notified after a grant is issued, so a composition root can
	// make it durable.
	//
	// The hook lives here because this is the one place every grant passes
	// through — the CLI's allow command, an agent's own request_override, and
	// an operator answering an approval card all land on RequestOverride. Left
	// to the callers, persistence is something each has to remember, and the
	// two that forgot issued grants that could never be reviewed afterwards,
	// which is the whole reason the ledger is written down.
	OnIssue func(grants.Grant)
	// OnChange is notified after every ledger mutation: issue, a single-use
	// grant consumed by a decision, and revoke. A host that persists the
	// ledger writes it down here; OnIssue alone left a spent single-use grant
	// and every revoke to evaporate on restart.
	OnChange func(grants.Change)

	mu sync.Mutex
	// decided is a ring of the most recent maxRecordedDecisions decisions,
	// oldest at next once it has filled. A long-lived host asks the gate about
	// every write for the life of the process, so keeping them all grew without
	// end. tally holds the exact counts Report gives however many fell out of
	// the ring, and grantsUsed every grant any decision applied, which is what
	// IssuedSummary needs to be exact and is bounded by the ledger.
	decided    []policy.Decision
	next       int
	tally      tally
	grantsUsed map[string]bool
}

// maxRecordedDecisions is how many decisions the gate keeps for Decisions and
// for the grant lines in Report. Older ones are counted, not kept.
const maxRecordedDecisions = 4096

// tally is the exact per-outcome count of every decision the gate recorded.
type tally struct {
	total, clean, granted, demoted, widened, denied, degraded int
}

// New builds a gate from a flag registry.
func New(reg *flags.Registry, root string, subsystems policy.SubsystemMap) (*Gate, error) {
	p, err := grantPolicyFrom(reg)
	if err != nil {
		return nil, err
	}
	g := &Gate{
		Flags:      reg,
		Ledger:     grants.NewLedger(p),
		Root:       root,
		Subsystems: subsystems,
	}
	// The ledger is the one place every mutation passes through, so the gate's
	// hooks hang off its single change callback rather than off each caller.
	g.Ledger.SetOnChange(func(c grants.Change) {
		if c.Kind == grants.Issued && g.OnIssue != nil {
			g.OnIssue(c.Grant)
		}
		if g.OnChange != nil {
			g.OnChange(c)
		}
	})
	return g, nil
}

// ReloadPolicy recomputes the grant policy from the registry and installs it.
//
// Every other flag this gate consults is read at the point of use, so a change
// lands on the next decision without anyone doing anything. The six grants.*
// flags are the exception: they are copied into the ledger's policy when the
// gate is built, which was fine while nothing could move them after boot. Once
// a setting can be moved at runtime, that copy is a second answer to "are agent
// grants enabled" — and the registry, which is what the flag table reports, is
// not the one deciding.
//
// Callers run this after a successful move. The grants already in the ledger
// are kept; see Ledger.SetPolicy.
func (g *Gate) ReloadPolicy() error {
	p, err := grantPolicyFrom(g.Flags)
	if err != nil {
		return err
	}
	g.Ledger.SetPolicy(p)
	return nil
}

// EvaluateWrite runs the full decision path for one file operation.
//
// The order is deliberate. The policy ladder runs first and at full strength,
// so the record always shows what the rules actually said. Only then does a
// grant clear it, and only then does the gate mode demote it. A decision that
// was allowed because enforcement is off still carries the rule that would
// have fired — which is what makes an advisory run useful rather than blank.
func (g *Gate) EvaluateWrite(path string, task *dc.Task, op dc.Operation) (policy.Decision, error) {
	mode, modeOrigin, err := flags.EffectiveGateMode(g.Flags, flags.PolicyFileMode)
	if err != nil {
		return policy.Decision{}, err
	}
	hardRules, _, err := flags.EffectiveHardRules(g.Flags)
	if err != nil {
		return policy.Decision{}, err
	}
	neighbors, _, err := g.Flags.Bool(flags.PolicyNeighborScope)
	if err != nil {
		return policy.Decision{}, err
	}
	sameDir, _, err := g.Flags.Bool(flags.PolicyScopeSameDir)
	if err != nil {
		return policy.Decision{}, err
	}

	fg := policy.FileGate{
		Root:           g.Root,
		Subsystems:     g.Subsystems,
		AllowNeighbors: neighbors,
		AllowSameDir:   sameDir,
		HardRules:      hardRules,
		Matcher:        g.Matcher,
	}
	decision := fg.EvaluateFileChange(path, task, op, false)
	return g.settle(decision, mode, modeOrigin, flags.PolicyFileMode), nil
}

// EvaluateRead runs the decision path for reading one file.
//
// Composed like EvaluateWrite — ladder, then grant, then mode — so an operator
// meets one model rather than two. What it does not share is the scope half:
// see policy.FileGate.EvaluateRead for why reading is judged only against the
// credential paths.
func (g *Gate) EvaluateRead(path string, task *dc.Task) (policy.Decision, error) {
	mode, modeOrigin, err := flags.EffectiveGateMode(g.Flags, flags.PolicyFileMode)
	if err != nil {
		return policy.Decision{}, err
	}
	hardRules, _, err := flags.EffectiveHardRules(g.Flags)
	if err != nil {
		return policy.Decision{}, err
	}
	fg := policy.FileGate{
		Root:       g.Root,
		Subsystems: g.Subsystems,
		HardRules:  hardRules,
		Matcher:    g.Matcher,
	}
	return g.settle(fg.EvaluateRead(path, task), mode, modeOrigin, flags.PolicyFileMode), nil
}

// EvaluateCommand runs the shell-command gate through the same three seams:
// the policy ladder decides, a grant may clear a soft denial, and the mode flag
// may demote one. Identical composition to EvaluateWrite, so an operator does
// not have to learn two models.
//
// Matching normalises away trailing redirections so allowlist entries stay
// single-clause, which means an approved command can still carry a redirect
// the ladder never looked at. Each target is therefore evaluated as the write
// it actually is, through exactly the path a WriteFile call faces — but only
// once the command's own decision has settled, because settling is where a
// grant and a mode demotion turn a denial into an allow. See EvaluateRedirects
// for the invariant and for what the earlier ordering let through.
func (g *Gate) EvaluateCommand(command string, task *dc.Task) (policy.Decision, error) {
	mode, modeOrigin, err := flags.EffectiveGateMode(g.Flags, flags.PolicyCommandMode)
	if err != nil {
		return policy.Decision{}, err
	}
	hardRules, _, err := flags.EffectiveHardRules(g.Flags)
	if err != nil {
		return policy.Decision{}, err
	}

	decision := policy.CommandGate{
		GlobalAllowedCommands: g.GlobalAllowedCommands,
		HardRules:             hardRules,
		Matcher:               g.Matcher,
		// Without the root, argv[0] normalisation cannot tell this tree's
		// .venv/bin/dev from a foreign one at the same layout, and fails
		// closed by normalising neither. Passing it restores the repo's own
		// venv CLI without restoring the launderer.
		Root: g.Root,
	}.EvaluateCommand(command, task)

	// A hard denial for what the line does needs nothing further. It is
	// undemotable and ungrantable by construction, so the command cannot run and
	// its redirections cannot happen; analysing them would only replace a precise
	// refusal with a vaguer one.
	//
	// A hard denial for an unreadable construct is not that. It means the gate
	// could not read the line, and a host may treat that more gently than a
	// refusal for what the line does. Stopping here let `cd src && echo x > .env`
	// come back as a directory change, and a host that softened that would have
	// written .env having never shown it to the write gate. Those lines still
	// have their redirections judged, below.
	//
	// Every other outcome — allow, warn, and a *soft* denial — must have its
	// redirections judged too, and the soft denial is the case a `Blocked()` test
	// here gets wrong. A command the ladder soft-refuses would skip the rung
	// entirely; the posture then demotes that refusal, or a grant clears it, and
	// the write runs having never been shown to the write gate. That turns a
	// scope rule an operator chose to relax into a way past the credential rules
	// they did not: `exec > .env`, `false || echo x > .env` and `(echo x > .env)`
	// all wrote .env, while the same redirect alone was refused as path.secret.
	if decision.Blocked() && decision.Severity == policy.Hard && !policy.IsUnreadableRule(decision.Rule) {
		return g.settle(decision, mode, modeOrigin, flags.PolicyCommandMode), nil
	}

	refusal, err := EvaluateRedirectsOps(command, taskIDOf(task), task != nil, func(target string, op dc.Operation) (policy.Decision, error) {
		// Through EvaluateWrite, not the bare ladder: a redirect target must be
		// judged by exactly what a direct write to the same path faces, ledger
		// and file-mode posture included, or the two surfaces contradict each
		// other. It also records the target decision, which is why the
		// FromTarget branch below does not record it again.
		return g.EvaluateWrite(target, task, op)
	})
	if err != nil {
		return policy.Decision{}, err
	}
	if refusal.Refused {
		// Returned without settling the command decision, so a grant scoped to
		// the command is not consumed for a command that will not run — and so
		// nothing on the command side can clear a refusal the file side made.
		// The redirect verdict has already been through the file gate's own
		// ledger and mode inside EvaluateWrite; if it still blocks after that,
		// it is a refusal those seams declined to clear.
		if refusal.FromTarget {
			return refusal.Decision, nil
		}
		// Synthesised by the rung itself, so nothing else has seen it. Recorded
		// here or the run summary undercounts a denial this gate made — a
		// fail-closed decision that Report() could not account for.
		return g.record(refusal.Decision), nil
	}
	// The redirects were judged and passed. A refusal that remains is only
	// "the gate could not read this", and the marker is what lets a host tell
	// that from "the gate forbids this". The ladder does not add it: it does
	// not judge redirects.
	if decision.Blocked() && decision.Severity == policy.Hard && policy.IsUnreadableRule(decision.Rule) {
		decision = policy.WithUnreadableOnly(decision)
	}
	return g.settle(decision, mode, modeOrigin, flags.PolicyCommandMode), nil
}

// WriteEvaluator judges one file the command would write — a redirection
// target or a file a command writes through its arguments — as the write it
// actually is. Every such file is judged as a write (dc.OpWrite in the gate's
// own evaluator), including the source mv removes and the file rm deletes; a
// WriteEvaluator is not told which operation the command performs. A host
// whose evaluator can tell the two apart takes an OperationEvaluator through
// EvaluateRedirectsOps and judges removals as dc.OpDelete.
//
// It is injected rather than fixed so the rung below has exactly one
// implementation across every plane that runs commands: the DevCouncil gate
// passes its own EvaluateWrite, flags and grant ledger and all, and the serve
// plane passes the taskless host evaluation it already answers
// policy.check.file with. What must not vary — which components a command line
// has, what an unresolvable one means, and that a refused component refuses the
// command — is decided in one place.
type WriteEvaluator func(target string) (policy.Decision, error)

// OperationEvaluator is a WriteEvaluator that is told the operation: dc.OpDelete
// for a file a command only removes (rm, rmdir, unlink, shred -u, git rm,
// git clean), dc.OpWrite for everything else.
type OperationEvaluator func(target string, op dc.Operation) (policy.Decision, error)

// RedirectRefusal is the redirect rung's outcome.
type RedirectRefusal struct {
	// Decision refuses the command. Meaningful only when Refused.
	Decision policy.Decision
	// Refused reports whether any component was refused.
	Refused bool
	// FromTarget distinguishes the two kinds of refusal, and the distinction
	// is not cosmetic. True means the refusal is a target's own write decision:
	// the caller's WriteEvaluator produced it, so whatever that caller does
	// with decisions — record it, count it, log it — has already happened.
	// False means the rung synthesised the refusal itself and no evaluator ever
	// saw it, so a caller that keeps a run log must record it or summarise a
	// denial it never accounts for.
	FromTarget bool
}

// EvaluateRedirects is the harness's one redirect rung — the rung for every
// file a command line writes, named for the redirections it was first written
// for.
//
// INVARIANT: no command reaches execution with any component left unevaluated.
// A command line is not one subject but several — the command itself, and every
// file it would write: each output redirection's target, and each file a
// command writes through its arguments (sed -i, tee, cp, mv; see
// policy.RedirectTargets). Allowlist matching deliberately
// normalises the redirections away so entries stay single-clause, so the string
// that matched is never the string that runs, and the targets are judged here
// or by nobody.
//
// The invariant is about *ordering* as much as coverage, and that is the half
// that was missing. This rung must run on every command a caller is about to
// permit, no matter how the command came to be permitted: cleanly, by a grant
// that cleared a soft denial, by a gate-mode demotion, or by a posture that
// demotes soft denials wholesale. Placed behind a test of the policy ladder's
// own Deny — which is what gate.EvaluateCommand used to do, before it called
// settle — the rung silently skips exactly the commands that grants and
// demotions let through. `cat src/calc.go > .env.local` was denied by the
// allowlist rung, so its target was never looked at; a grant then cleared the
// allowlist rung and the write went with it, .devcouncil grant ledger included.
// Callers therefore run this after settling and before returning an allow, and
// never behind a test of the command decision's own action.
//
// A refusal is returned as-is, not folded back through the caller's command-mode
// demotion: a target that survived the file gate's own mode has been judged as
// the write it is, and demoting it a second time under a command flag would let
// the harness contradict how it treats a direct write to the same path.
//
// scopeBound reports whether the caller judges writes against a task's
// declared scope. Only then does a write the command line does not name — git
// apply, patch with no file operand, xargs or find running a writer — matter:
// the file gate cannot be asked about a path nobody can state, so the command
// is refused rather than run past a fence it cannot be checked against. With no
// scope bound there is no fence, and such a command is left to the rest of the
// ladder as before. The refusal reuses command.substitution, the rung's word
// for a write it cannot judge, so no new rule enters the contract.
func EvaluateRedirects(command, taskID string, scopeBound bool, evalWrite WriteEvaluator) (RedirectRefusal, error) {
	return EvaluateRedirectsOps(command, taskID, scopeBound, func(target string, _ dc.Operation) (policy.Decision, error) {
		return evalWrite(target)
	})
}

// EvaluateRedirectsOps is EvaluateRedirects for an evaluator that judges
// removals as the deletes they are. A file planned for modification is not a
// file the task may delete, and `rm` of it was judged as a write and allowed.
func EvaluateRedirectsOps(command, taskID string, scopeBound bool, evalWrite OperationEvaluator) (RedirectRefusal, error) {
	set, err := policy.RedirectWrites(command)
	targets, opaque := set.Targets, set.Opaque
	if err != nil {
		return RedirectRefusal{}, err
	}
	if opaque {
		return RedirectRefusal{
			Decision: policy.Decision{
				Action:   policy.Deny,
				Rule:     policy.RuleCommandSubstitution,
				Severity: policy.SeverityOf(policy.RuleCommandSubstitution),
				Reason: "A file this command writes — a redirection target or a file operand — is chosen " +
					"by a command substitution, or the line writes more files than the gate enumerates, " +
					"and was judged as an unverifiable write.",
				Target: command,
				TaskID: taskID,
			},
			Refused: true,
		}, nil
	}
	deleted := make(map[string]bool, len(set.Deletes))
	for _, target := range set.Deletes {
		deleted[target] = true
	}
	// Ranked like a compound command's clauses. Returning the first blocked
	// target let `cp -t . src/calc.go` — an unplanned `./calc.go`, soft — hide
	// the `mv calc.go .env` after it, and a posture that demotes scope then
	// reached the credential file. A hard refusal for what a write does cannot
	// be outranked, so judging stops there.
	var blocked []policy.Decision
	for _, target := range targets {
		op := dc.OpWrite
		if deleted[target] {
			op = dc.OpDelete
		}
		written, err := evalWrite(target, op)
		if err != nil {
			return RedirectRefusal{}, err
		}
		if !written.Blocked() {
			continue
		}
		written.Reason = fmt.Sprintf("Command writes a file the write gate refuses (%s): %s",
			target, written.Reason)
		stop := written.Severity == policy.Hard && !policy.IsUnreadableRule(written.Rule)
		blocked = append(blocked, written)
		if stop {
			break
		}
	}
	if chosen, refused := policy.MostImportant(blocked); refused {
		return RedirectRefusal{Decision: chosen, Refused: true, FromTarget: true}, nil
	}
	if scopeBound && set.Unjudged != "" {
		return RedirectRefusal{
			Decision: policy.Decision{
				Action:   policy.Deny,
				Rule:     policy.RuleCommandSubstitution,
				Severity: policy.SeverityOf(policy.RuleCommandSubstitution),
				Reason: "This task has a declared scope, and this command writes files its text does not name (" +
					set.Unjudged + "), so they cannot be checked against the scope before it runs.",
				Target: command,
				TaskID: taskID,
			},
			Refused: true,
		}, nil
	}
	return RedirectRefusal{}, nil
}

func taskIDOf(task *dc.Task) string {
	if task == nil {
		return ""
	}
	return task.ID
}

// settle applies the override ledger and then the gate mode, and records the
// decision. Shared by both gates so the two cannot drift apart.
func (g *Gate) settle(decision policy.Decision, mode string, modeOrigin flags.Origin, modeFlag string) policy.Decision {
	if decision.Blocked() {
		if cleared, used := g.Ledger.Apply(decision); used {
			decision = cleared
		}
	}

	// Mode demotion never touches a hard denial. A gate mode is an operator
	// saying they do not want scope enforcement, and that is a different
	// statement from wanting the credential and git-safety rules gone. Turning
	// those off is its own decision — policy.hard_rules.enabled, or the yolo
	// posture that resolves it — and it takes effect by stopping those rungs
	// from firing at all, not by demoting what they decided. A hard denial that
	// reached this point still stands.
	if decision.Blocked() && decision.Severity != policy.Hard {
		switch mode {
		case flags.ModeAdvisory, flags.ModeOff:
			decision.Action = policy.Allow
			decision.Demoted = fmt.Sprintf("%s=%s (%s)", modeFlag, mode, modeOrigin)
		}
	}

	return g.record(decision)
}

// record appends a decision to the run log without touching it.
//
// settle is the path for a decision the ledger and the gate mode still have a
// say over. This one is for a decision that has already been resolved by the
// rung that produced it and that neither seam may move again — the redirect
// rung's own refusal, which no evaluator recorded because no evaluator made it.
// Split out so there is a single place appending to g.decided: a second one is
// how a denial comes to be made and never counted.
func (g *Gate) record(decision policy.Decision) policy.Decision {
	g.mu.Lock()
	defer g.mu.Unlock()
	if len(g.decided) < maxRecordedDecisions {
		g.decided = append(g.decided, decision)
	} else {
		g.decided[g.next] = decision
		g.next = (g.next + 1) % maxRecordedDecisions
	}
	t := &g.tally
	t.total++
	switch {
	case decision.Blocked():
		t.denied++
	case decision.GrantID != "":
		t.granted++
	case decision.Demoted != "":
		t.demoted++
	case decision.Widened != "":
		t.widened++
	case decision.Clean():
		t.clean++
	}
	if len(decision.Degraded) > 0 {
		t.degraded++
	}
	if decision.GrantID != "" {
		if g.grantsUsed == nil {
			g.grantsUsed = map[string]bool{}
		}
		g.grantsUsed[decision.GrantID] = true
	}
	return decision
}

// RequestOverride issues a grant that would clear a decision, on the given
// authority. It is the single entry point for both surfaces: a human answering
// a prompt and an agent calling a tool reach the same code and the same limits.
func (g *Gate) RequestOverride(d policy.Decision, by grants.Grantor, reason string, agentTask string) (grants.Grant, error) {
	req, err := grants.SuggestRequest(d, by)
	if err != nil {
		return grants.Grant{}, err
	}
	req.Reason = reason
	if by.Authority == grants.Agent {
		req.AgentTask = agentTask
	}
	grant, err := g.Ledger.Issue(req)
	if err != nil {
		return grant, err
	}
	return grant, nil
}

// AgentCanGrant reports whether an agent may clear this rule for itself, under
// the policy this gate was composed with. The recovery advice handed to an
// agent is built from this, so advice and ledger cannot disagree.
func (g *Gate) AgentCanGrant(rule policy.RuleID) bool {
	return g.Ledger.AgentMayGrant(rule)
}

// Decisions returns the most recent decisions this gate made, oldest first: all
// of them until there have been more than maxRecordedDecisions, then the last
// maxRecordedDecisions. Report's DecisionsDropped says how many fell out.
func (g *Gate) Decisions() []policy.Decision {
	g.mu.Lock()
	defer g.mu.Unlock()
	return g.decisionsLocked()
}

func (g *Gate) decisionsLocked() []policy.Decision {
	out := make([]policy.Decision, 0, len(g.decided))
	out = append(out, g.decided[g.next:]...)
	return append(out, g.decided[:g.next]...)
}

// Report is the run summary that keeps a green result honest: how many writes
// passed cleanly, how many were cleared by grant, how many were let through by
// a demoted gate, and which safety flags were moved off their defaults.
type Report struct {
	// Posture is the development posture the run executed under. It is
	// reported unconditionally because a clean result means something
	// different in each: under dev, the soft rules did not block, so "nothing
	// was denied" is not evidence that nothing would have been.
	Posture string `json:"posture"`
	// HardRules is whether the credential, restricted-path, outside-root, and
	// git-safety rungs actually ran. It is reported even when no decision was
	// made, because "nothing was denied" means something different when the
	// rules that deny were not asked.
	HardRules bool `json:"hard_rules"`
	Total     int  `json:"total"`
	Clean     int  `json:"clean"`
	Granted   int  `json:"granted"`
	Demoted   int  `json:"demoted"`
	Denied    int  `json:"denied"`
	// Widened counts writes authorised by scope an executor appended to its own
	// task. They are reported apart from Granted because the grant that first
	// argued for them expires and this does not: in a later run the appended
	// scope is still there with no grant beside it, and without this counter
	// that run would summarise as though the plan had authorised every write.
	Widened  int `json:"widened"`
	Degraded int `json:"degraded"`
	// The counts above cover every decision. GrantLines is drawn from the
	// decisions the gate still holds, a capped sample of the most recent
	// ones: DecisionsSampled of them, with DecisionsDropped older ones
	// counted but no longer held. A non-zero DecisionsDropped means a grant
	// applied only by a dropped decision is missing from GrantLines.
	DecisionsSampled int      `json:"decisions_sampled"`
	DecisionsDropped int      `json:"decisions_dropped"`
	GrantLines       []string `json:"grants,omitempty"`
	// IssuedGrantLines are grants that were issued and never applied. On an
	// unplanned path the grant's real effect is the scope it caused to be
	// written, so this is where the reason for a widening is recorded.
	IssuedGrantLines []string `json:"issued_grants,omitempty"`
	WeakenedFlags    []string `json:"weakened_flags,omitempty"`
}

// Strict reports whether the run both enforced the rules and passed them.
//
// The posture is part of the predicate on purpose. A dev-posture run with no
// violations did not demote anything, so by the narrower reading it "passed on
// the rules alone" — but the soft rules were not enforcing, and a summary that
// cannot tell those two runs apart is the failure this whole layer exists to
// avoid. Strict is the honest, conservative answer to "was this checked?".
func (r Report) Strict() bool {
	// Denied is part of the predicate, and its absence was a defect rather than
	// a subtlety: a run whose only decision was a hard refusal to write .env
	// reported Strict() == true. "Was this checked?" and "did it pass?" are
	// both halves of what this answers, and a refusal is a failure of the
	// second one however cleanly the rules were enforced.
	return r.Posture == flags.PostureStrict && r.HardRules &&
		r.Denied == 0 && r.Granted == 0 && r.Demoted == 0 && r.Widened == 0 &&
		r.Degraded == 0 && len(r.WeakenedFlags) == 0
}

// Report summarises the run.
func (g *Gate) Report() Report {
	g.mu.Lock()
	decisions := g.decisionsLocked()
	t := g.tally
	used := make(map[string]bool, len(g.grantsUsed))
	for id := range g.grantsUsed {
		used[id] = true
	}
	g.mu.Unlock()

	rep := Report{
		Total: t.total, Clean: t.clean, Granted: t.granted, Demoted: t.demoted,
		Widened: t.widened, Denied: t.denied, Degraded: t.degraded,
		DecisionsSampled: len(decisions),
		DecisionsDropped: t.total - len(decisions),
	}
	if posture, _, err := g.Flags.String(flags.HarnessPosture); err == nil {
		rep.Posture = posture
	}
	if hard, _, err := flags.EffectiveHardRules(g.Flags); err == nil {
		rep.HardRules = hard
	}
	rep.GrantLines = grants.Summary(decisions)
	rep.IssuedGrantLines = grants.IssuedSummary(g.Ledger.Issued(), used)
	for _, v := range g.Flags.Weakened() {
		rep.WeakenedFlags = append(rep.WeakenedFlags, fmt.Sprintf("%s=%s (%s)", v.Key, v.Raw, v.Origin))
	}
	return rep
}

func grantPolicyFrom(reg *flags.Registry) (grants.Policy, error) {
	p := grants.DefaultPolicy()
	var err error
	if p.Enabled, _, err = reg.Bool(flags.GrantsEnabled); err != nil {
		return p, err
	}
	if p.AgentEnabled, _, err = reg.Bool(flags.GrantsAgentEnabled); err != nil {
		return p, err
	}
	if p.RequireReason, _, err = reg.Bool(flags.GrantsRequireReason); err != nil {
		return p, err
	}
	if p.AgentMaxTTL, _, err = reg.Duration(flags.GrantsAgentMaxTTL); err != nil {
		return p, err
	}
	if p.HumanMaxTTL, _, err = reg.Duration(flags.GrantsHumanMaxTTL); err != nil {
		return p, err
	}

	allowCommands, _, err := reg.Bool(flags.GrantsAgentCommands)
	if err != nil {
		return p, err
	}
	if allowCommands {
		p.AgentGrantableExtra = map[policy.RuleID]bool{policy.RuleCommandNotAllowed: true}
	}
	return p, nil
}
