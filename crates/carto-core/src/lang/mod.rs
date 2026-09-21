//! Language identification (spec §5.2) and the `LangExtractor` trait +
//! its implementations. Rust (M1.b.2a), Python (M1.b.2b), PHP
//! (ADR-0012), TypeScript/TSX/JavaScript (ADR-0013), and Go (ADR-0015)
//! form the full spec §5.2 v1 language set (closed M1.b.2b); C#
//! (ADR-0016) is the first post-v1 addition.

pub mod csharp;
pub mod ecma;
pub mod extractor;
pub mod go;
pub mod hcl;
pub mod php;
pub mod python;
pub mod resolve;
pub mod rust;

pub use csharp::CSharpExtractor;
pub use ecma::{JavaScriptExtractor, TsxExtractor, TypeScriptExtractor};
pub use extractor::{
    ExtractOut, ImportedName, LangExtractor, RawCallSite, RawImport, RawLiteral, RawSymbol,
};
pub use go::GoExtractor;
pub use hcl::HclExtractor;
pub use php::PhpExtractor;
pub use python::PythonExtractor;
pub use resolve::{FileExtraction, ResolvedExtraction, resolve};
pub use rust::RustExtractor;

use crate::graph::Node;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The v1 language set (spec §5.2 — amended by ADR-0012 to include PHP
/// and ADR-0016 to include C#), plus `Other`/`PlainText` for anything
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
    Php,
    /// serde-renamed: snake_case would derive `c_sharp`, but the
    /// ecosystem-conventional (and graph.json-friendlier) spelling is
    /// `csharp`.
    #[serde(rename = "csharp")]
    CSharp,
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
            "cs" => Lang::CSharp,
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
        Box::new(CSharpExtractor),
        Box::new(HclExtractor),
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
pub fn extract_and_resolve(
    repo_root: &Path,
    file_nodes: &[Node],
    components: &[crate::components::Component],
) -> crate::error::Result<ResolvedExtraction> {
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
            ns_separator: extractor.namespace_separator(),
            qualified_external_is_full_fqn: extractor.qualified_external_is_full_fqn(),
            declares_module: extractor.relative_import_declares_module(),
            lang: extractor.lang(),
            component: file.component.clone(),
        });
    }

    // ADR-0026/0027: built-in contract-classification rules plus this
    // repo's own `.carto/contracts.json`, if any — a malformed override
    // file stops the index rather than silently classifying nothing.
    let contract_rules = crate::contracts::ContractRules::load(repo_root)?;

    // ADR-0039: `resolve` builds its own `component -> depends_on`
    // index from `components` (alongside `contract_rules`) — passed
    // straight through here, not pre-indexed, since `resolve.rs`
    // already builds every other per-file/per-component lookup it
    // needs from raw inputs rather than having its caller do it.
    Ok(resolve(extractions, &contract_rules, components))
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
        assert_eq!(Lang::from_extension("cs"), Lang::CSharp);
        assert_eq!(Lang::from_extension("tf"), Lang::Hcl);
        assert_eq!(Lang::from_extension("yaml"), Lang::Yaml);
        assert_eq!(Lang::from_extension("json"), Lang::Json);
    }

    #[test]
    fn unrecognized_extension_is_other() {
        assert_eq!(Lang::from_extension("xyz123"), Lang::Other);
    }
}
