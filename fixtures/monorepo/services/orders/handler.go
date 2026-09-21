package orders

// Handler is declared three times repo-wide, one per component
// (here, services/billing/Handler.cs, web/admin/src/handler.ts) --
// the exact same-bare-name-across-components shape that, before
// ADR-0034/0035, made every one of them unresolvable to any caller.
// Handler itself calls Shared (libs/shared/shared.go) with no Go
// import at all -- a real cross-component bare-name resolution,
// tier (c2), evidence "cross-component".
func Handler() string {
	return Shared()
}
