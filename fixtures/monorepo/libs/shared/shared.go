package shared

// Shared is the only symbol named Shared repo-wide -- services/orders'
// handler.go calls it with no import statement at all, exercising
// tier (c2)'s "cross-component" evidence label (ADR-0035): a real,
// unambiguous bare-name resolution that happens to cross a component
// boundary, the legitimate use of the repo-wide fallback tier.
func Shared() string {
	return "shared"
}
