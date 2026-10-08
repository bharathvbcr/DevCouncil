package policy

// DegradedUnreadableOnly is the Degraded entry on a hard refusal whose line was
// refused for nothing but unreadable constructs: every clause was judged, no
// other hard rule fired, and the redirection targets were judged and passed.
//
// It is the promise an embedding host needs before it may treat "the gate could
// not read this" differently from "the gate forbids this", and the rule ID alone
// cannot make it. It is added only by the layers that also judge redirects —
// gate.Gate and the serve plane — never by the ladder, which does not.
const DegradedUnreadableOnly = "policy.unreadable_only"

// WithUnreadableOnly returns d carrying DegradedUnreadableOnly, once.
func WithUnreadableOnly(d Decision) Decision {
	for _, have := range d.Degraded {
		if have == DegradedUnreadableOnly {
			return d
		}
	}
	d.Degraded = append(append([]string(nil), d.Degraded...), DegradedUnreadableOnly)
	return d
}
