//! Grammar loading (spec §5.4). Native grammars behind the
//! `native-grammars` feature (default; M1–M4). `wasm-grammars` (M5,
//! default from M5) is not implemented yet.
//!
//! This crate is the workspace's designated exception to
//! `unsafe_code = "forbid"` (spec §3.1) — see `Cargo.toml`'s
//! `[lints.rust]`. In practice, `tree-sitter` 0.26 / `tree-sitter-rust`
//! 0.24's `LanguageFn -> Language` conversion below turned out not to
//! require an `unsafe` block at all (verified by attempting the build;
//! `docs.rs` isn't reachable from this sandbox to check the changelog).
//! See `docs/adr/0007-carto-grammars-unsafe-carveout.md` for why the
//! carve-out stays in place regardless.

#[cfg(feature = "native-grammars")]
pub fn rust_language() -> tree_sitter::Language {
    tree_sitter_rust::LANGUAGE.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_language_loads_without_panicking() {
        // The real assertion is that constructing a Parser and setting
        // this language succeeds — an ABI mismatch between the grammar's
        // compiled parse tables and the linked tree-sitter runtime would
        // surface here, not as a panic in `rust_language()` itself.
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&rust_language()).expect(
            "tree-sitter-rust's language must be compatible with the linked tree-sitter runtime",
        );
    }
}
