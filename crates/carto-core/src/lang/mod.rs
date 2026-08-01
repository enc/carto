//! Language identification (spec §5.2) and the `LangExtractor` trait +
//! its implementations. Rust (M1.b.2a), Python (M1.b.2b), PHP
//! (ADR-0012), TypeScript/TSX/JavaScript (ADR-0013), and Go (ADR-0015)
//! exist — the full spec §5.2 v1 language set, closing M1.b.2b.

pub mod ecma;
pub mod extractor;
pub mod go;
pub mod php;
pub mod python;
pub mod resolve;
pub mod rust;

pub use ecma::{JavaScriptExtractor, TsxExtractor, TypeScriptExtractor};
pub use extractor::{ExtractOut, ImportedName, LangExtractor, RawCallSite, RawImport, RawSymbol};
pub use go::GoExtractor;
pub use php::PhpExtractor;
pub use python::PythonExtractor;
pub use resolve::{FileExtraction, ResolvedExtraction, resolve};
pub use rust::RustExtractor;

use crate::graph::Node;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The v1 language set (spec §5.2 — amended by ADR-0012 to include PHP),
/// plus `Other`/`PlainText` for anything `walk` sees that isn't in that
/// set yet (M1.b.1 has no extractors, so every file is currently either
/// recognized-by-extension or `Other`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lang {
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Rust,
    Go,
    Php,
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
            "php" => Lang::Php,
            "tf" | "tfvars" | "hcl" => Lang::Hcl,
            "yaml" | "yml" => Lang::Yaml,
            "json" => Lang::Json,
            "md" | "txt" | "toml" | "cfg" | "ini" | "lock" => Lang::PlainText,
            _ => Lang::Other,
        }
    }
}

/// Every registered `LangExtractor` (M1.b.2b: a small registry, not a
/// single hard-coded extractor — `extract_and_resolve` looks each file
/// up by its own `Lang`). Order here has no bearing on output
/// determinism: `resolve`'s output nodes/edges are `Graph`-sorted
/// regardless of extraction order, and a file only ever matches exactly
/// one extractor (by `Lang`).
fn extractors() -> Vec<Box<dyn LangExtractor>> {
    vec![
        Box::new(RustExtractor),
        Box::new(PythonExtractor),
        Box::new(PhpExtractor),
        Box::new(TypeScriptExtractor),
        Box::new(TsxExtractor),
        Box::new(JavaScriptExtractor),
        Box::new(GoExtractor),
    ]
}

/// Orchestrates extraction + resolution for every walked `File` node
/// whose language has a registered extractor (`file_nodes` is expected
/// to be `walk::WalkOutput::nodes`, but this function only depends on
/// the `Node` shape, not on `walk` itself). Files that are
/// `skipped`/`excluded` (spec §5.1's walk classification) or whose
/// `Lang` has no extractor yet are left alone — only their `File` node
/// exists, same as before any extractor existed.
///
/// Re-reads each eligible file's content from disk (`walk` already read
/// it once, for hashing/binary-sniffing, but doesn't retain the bytes) —
/// a second read is simpler than threading raw content through `walk`'s
/// public output, and every file this touches already passed `walk`'s
/// size cap. A file that becomes unreadable between the two reads
/// (TOCTOU) is silently skipped, same tolerance `walk::classify`
/// already has for the same race.
pub fn extract_and_resolve(repo_root: &Path, file_nodes: &[Node]) -> ResolvedExtraction {
    let extractors = extractors();
    let mut extractions = Vec::new();

    for node in file_nodes {
        let Some(file) = node.data.as_file() else {
            continue;
        };
        if file.skipped.is_some() || file.excluded.is_some() {
            continue;
        }
        let Some(extractor) = extractors.iter().find(|e| e.lang() == file.lang) else {
            continue;
        };
        let Ok(content) = std::fs::read(repo_root.join(&file.path)) else {
            continue;
        };
        extractions.push(FileExtraction {
            file_id: node.id.clone(),
            relpath: file.path.clone(),
            extract: extractor.extract(&content, &file.path),
            origin: extractor.origin(),
            dir_scoped: extractor.package_scope_is_directory(),
        });
    }

    resolve(extractions)
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
        assert_eq!(Lang::from_extension("php"), Lang::Php);
        assert_eq!(Lang::from_extension("tf"), Lang::Hcl);
        assert_eq!(Lang::from_extension("yaml"), Lang::Yaml);
        assert_eq!(Lang::from_extension("json"), Lang::Json);
    }

    #[test]
    fn unrecognized_extension_is_other() {
        assert_eq!(Lang::from_extension("xyz123"), Lang::Other);
    }
}
