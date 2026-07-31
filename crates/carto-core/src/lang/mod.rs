//! Language identification (spec §5.2). This slice only needs `Lang` for
//! `FileNode.lang` (spec §4.1) — the `LangExtractor` trait, tree-sitter
//! queries, and `queries/` directory (spec §5.2) arrive in M1.b.2 with the
//! extractors that use them.

use serde::{Deserialize, Serialize};

/// The v1 language set (spec §5.2), plus `Other`/`PlainText` for anything
/// `walk` sees that isn't in that set yet (M1.b.1 has no extractors, so
/// every file is currently either recognized-by-extension or `Other`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lang {
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Rust,
    Go,
    Hcl,
    Yaml,
    Json,
    /// Recognized as text but outside the v1 language set (spec §5.2),
    /// e.g. `.md`, `.toml`.
    PlainText,
    /// Binary-sniffed (spec §5.1) or otherwise not text.
    Other,
}

impl Lang {
    /// Best-effort language-from-extension mapping. Used by `walk` to
    /// populate `FileNode.lang`; independent of whether an extractor for
    /// that language exists yet (none do, this slice).
    pub fn from_extension(ext: &str) -> Lang {
        match ext.to_ascii_lowercase().as_str() {
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
            "py" | "pyi" => Lang::Python,
            "rs" => Lang::Rust,
            "go" => Lang::Go,
            "tf" | "tfvars" | "hcl" => Lang::Hcl,
            "yaml" | "yml" => Lang::Yaml,
            "json" => Lang::Json,
            "md" | "txt" | "toml" | "cfg" | "ini" | "lock" => Lang::PlainText,
            _ => Lang::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_v1_language_extensions() {
        assert_eq!(Lang::from_extension("ts"), Lang::TypeScript);
        assert_eq!(Lang::from_extension("tsx"), Lang::Tsx);
        assert_eq!(Lang::from_extension("JS"), Lang::JavaScript);
        assert_eq!(Lang::from_extension("py"), Lang::Python);
        assert_eq!(Lang::from_extension("rs"), Lang::Rust);
        assert_eq!(Lang::from_extension("go"), Lang::Go);
        assert_eq!(Lang::from_extension("tf"), Lang::Hcl);
        assert_eq!(Lang::from_extension("yaml"), Lang::Yaml);
        assert_eq!(Lang::from_extension("json"), Lang::Json);
    }

    #[test]
    fn unrecognized_extension_is_other() {
        assert_eq!(Lang::from_extension("xyz123"), Lang::Other);
    }
}
