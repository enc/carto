//! Enforces the INV-5 compile-time boundary: constructing report/MCP output
//! from raw repository text MUST NOT compile (spec §2, INV-5). `trybuild`
//! (dev-dependency, see docs/adr/0002-trybuild-dev-dep.md) compiles each
//! case under `tests/ui/` and asserts it fails, diffing against the
//! recorded `.stderr` so a case that fails for the *wrong* reason (e.g. a
//! typo) shows up as a mismatch rather than a false pass.

#[test]
fn taint_boundary_holds() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
