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

/// Python grammar (M1.b.2b). Same `LanguageFn -> Language` conversion as
/// [`rust_language`] — verified empirically to not need an `unsafe`
/// block here either, for the same `tree-sitter-language`-mediated
/// reason ADR-0007 records for Rust.
#[cfg(feature = "native-grammars")]
pub fn python_language() -> tree_sitter::Language {
    tree_sitter_python::LANGUAGE.into()
}

/// PHP grammar (third language, ADR-0012). `tree-sitter-php` exposes two
/// grammars (`LANGUAGE_PHP` and `LANGUAGE_PHP_ONLY`, the latter for
/// files with no surrounding `<?php ... ?>` tags); real `.php` files
/// open with `<?php`, so `LANGUAGE_PHP` is the correct one here. Same
/// `LanguageFn -> Language` conversion as [`rust_language`]/
/// [`python_language`] — no `unsafe` block needed for the same reason.
#[cfg(feature = "native-grammars")]
pub fn php_language() -> tree_sitter::Language {
    tree_sitter_php::LANGUAGE_PHP.into()
}

/// TypeScript grammar (fourth/fifth/sixth languages, ADR-0013).
/// `tree-sitter-typescript` exposes two grammars from one crate —
/// `LANGUAGE_TYPESCRIPT` (no JSX) and `LANGUAGE_TSX` (JSX; see
/// [`tsx_language`]) — verified structurally near-identical for
/// symbols/imports/calls, which is why `lang::ecma` shares one
/// extraction module across both plus JavaScript. Same
/// `LanguageFn -> Language` conversion as the others, no `unsafe`
/// needed for the same reason.
#[cfg(feature = "native-grammars")]
pub fn typescript_language() -> tree_sitter::Language {
    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
}

/// TSX grammar — see [`typescript_language`].
#[cfg(feature = "native-grammars")]
pub fn tsx_language() -> tree_sitter::Language {
    tree_sitter_typescript::LANGUAGE_TSX.into()
}

/// JavaScript grammar — see [`typescript_language`].
#[cfg(feature = "native-grammars")]
pub fn javascript_language() -> tree_sitter::Language {
    tree_sitter_javascript::LANGUAGE.into()
}

/// Go grammar (seventh language, M1.b.2b's closing slice — see
/// `docs/adr/0015-go-resolution-policy-mapping.md`). Same
/// `LanguageFn -> Language` conversion as the others, no `unsafe` needed
/// for the same reason.
#[cfg(feature = "native-grammars")]
pub fn go_language() -> tree_sitter::Language {
    tree_sitter_go::LANGUAGE.into()
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

    #[test]
    fn python_language_loads_without_panicking() {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&python_language()).expect(
            "tree-sitter-python's language must be compatible with the linked tree-sitter runtime",
        );
    }

    #[test]
    fn php_language_loads_without_panicking() {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&php_language()).expect(
            "tree-sitter-php's language must be compatible with the linked tree-sitter runtime",
        );
    }

    #[test]
    fn typescript_language_loads_without_panicking() {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&typescript_language()).expect(
            "tree-sitter-typescript's language must be compatible with the linked tree-sitter runtime",
        );
    }

    #[test]
    fn tsx_language_loads_without_panicking() {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&tsx_language()).expect(
            "tree-sitter-typescript's TSX language must be compatible with the linked tree-sitter runtime",
        );
    }

    #[test]
    fn javascript_language_loads_without_panicking() {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&javascript_language()).expect(
            "tree-sitter-javascript's language must be compatible with the linked tree-sitter runtime",
        );
    }

    #[test]
    fn go_language_loads_without_panicking() {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&go_language()).expect(
            "tree-sitter-go's language must be compatible with the linked tree-sitter runtime",
        );
    }
}
