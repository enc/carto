package orders

// Run calls bare Handler() from within the *same* component --
// before ADR-0034/0035 this was ambiguous against billing's and
// admin's own same-named Handler (three repo-wide candidates, no
// edge at all, INV-8). Tier (c1) now resolves it to this component's
// own Handler unambiguously, evidence "same-component".
func Run() string {
	return Handler()
}
