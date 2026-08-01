package orders

// normalize trims and lowercases order input. Unexported and never
// imported anywhere — reachable from ParseOrder (order.go) purely
// through Go's own package-is-a-directory visibility: resolution tier
// (a′), same directory, no import needed.
func normalize(input string) string {
	return input
}

// Auditable describes anything that can produce an audit trail.
type Auditable interface {
	Audit() string
}
