package secret

// W1 (added for the dead-policy card): an exported function under internal/
// that nothing calls. Go forbids importing internal/ from outside the module
// root, so the export proves nothing — this is a 0.4 `exported_unreferenced`
// candidate.
func ValidateChunks(chunks []string) bool { return len(chunks) > 0 }

// W2: an exported type under internal/ whose method satisfies the in-repo
// `store.Logger` interface. Interface dispatch reaches it; it stays exempt.
type FileSink struct{}

// Log09 implements store.Logger.
func (FileSink) Log09(string) {}
