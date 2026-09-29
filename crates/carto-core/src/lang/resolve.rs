//! Cross-file resolution (spec §5.3): turns every file's raw extraction
//! output into graph `Symbol`/`Module` nodes and `contains`/`imports`/
//! `calls` edges. Whole-repo and single-threaded by necessity — a call's
//! resolution needs to see every other file's exported symbols before it
//! can be judged unambiguous (or not).
//!
//! Rust-specific interpretation of spec §5.3's language-agnostic rules
//! (`mod` -> file resolution, `use`-root -> internal/external
//! classification, "same-package" -> "same walked repo") is recorded in
//! `docs/adr/0008-rust-resolution-policy-mapping.md`.

use super::Lang;
use super::extractor::{ExtractOut, RawImport, RawSymbol};
use crate::consts;
use crate::contracts::{ContractRules, Role};
use crate::graph::{
    self, Confidence, ContractNode, Edge, EdgeKind, InboundCallSite, ModuleNode, Node, NodeId,
    SymKind, SymbolNode, UnresolvedCall,
};
use crate::taint::{Provenance, TaintedString};
use std::collections::{BTreeMap, BTreeSet};

/// One file's raw extraction, paired with the identifying info `resolve`
/// needs but [`super::LangExtractor::extract`] doesn't produce itself.
pub struct FileExtraction {
    pub file_id: NodeId,
    pub relpath: String,
    pub extract: ExtractOut,
    /// The producing extractor's [`super::LangExtractor::origin`] (spec
    /// §4.1's node `origin` field). Per-file rather than a single
    /// module-wide constant since `resolve` processes files from more
    /// than one extractor in a single pass (M1.b.2b).
    pub origin: &'static str,
    /// [`super::LangExtractor::package_scope_is_directory`]'s value for
    /// the producing extractor — `true` only for Go (ADR-0015). Enables
    /// tier (a′) for this file's calls.
    pub dir_scoped: bool,
    /// [`super::LangExtractor::namespace_separator`]'s value for the
    /// producing extractor — `\` for PHP, `.` for C# (ADR-0016). Used
    /// wherever this file's `declared_namespace` or `Qualified`/
    /// `NamespaceImport` paths are composed into or split against the
    /// FQN/namespace indices.
    pub ns_separator: &'static str,
    /// [`super::LangExtractor::qualified_external_is_full_fqn`]'s value
    /// for the producing extractor — `false` for PHP (root-truncated
    /// dedup), `true` for C# (full-FQN identity). Decides how an
    /// unresolved `RawImport::Qualified`'s external `Module` node is
    /// keyed.
    pub qualified_external_is_full_fqn: bool,
    /// [`super::LangExtractor::relative_import_declares_module`]'s
    /// value for the producing extractor — `true` only for Rust.
    /// Decides `known_modules` membership and the `mod-declaration` vs.
    /// `relative-import` evidence label for an empty-names
    /// `RawImport::Relative`.
    pub declares_module: bool,
    /// The producing extractor's own [`super::LangExtractor::lang`] —
    /// ADR-0026's contract-literal pass classifies each
    /// `ExtractOut::literals` entry by `(lang, position)`, so it needs
    /// this alongside `origin` (a distinct string, not reused: `origin`
    /// carries a version suffix `contracts::ContractRules` has no reason
    /// to key on).
    pub lang: Lang,
    /// This file's [`crate::components::Component::name`] — mirrors
    /// `FileNode::component` exactly (set by `indexer::build_and_persist`
    /// before `extract_and_resolve` runs; `lang/mod.rs` just copies it
    /// across). `None` for a file under no recognized project root, the
    /// same real, meaningful bucket `FileNode::component`'s own doc
    /// comment describes. ADR-0035.
    pub component: Option<String>,
}

pub struct ResolvedExtraction {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

/// Every file declaring a given FQN, paired with that file's component
/// (`(declaring-file-id, component)`) — `resolve_fqn`'s candidate list.
/// A type alias purely to keep clippy's `type_complexity` lint quiet;
/// see the `fqn_to_file` binding in [`resolve`] for what it's for.
type FqnCandidates<'a> = BTreeMap<(&'static str, String), Vec<(&'a NodeId, Option<&'a str>)>>;

pub fn resolve(
    extractions: Vec<FileExtraction>,
    contract_rules: &ContractRules,
    components: &[crate::components::Component],
) -> ResolvedExtraction {
    // ADR-0039: `caller component -> its own declared dependencies`,
    // restricted to `crate::components::DEPENDENCY_AWARE_KINDS` — see
    // that const's own doc comment for why a component outside that
    // set must never produce an "undeclared-dependency" evidence
    // entry.
    let component_depends_on: BTreeMap<&str, BTreeSet<&str>> = components
        .iter()
        .filter(|c| crate::components::DEPENDENCY_AWARE_KINDS.contains(&c.kind.as_str()))
        .map(|c| {
            (
                c.name.as_str(),
                c.depends_on.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    // Every top-level `mod <name>;` declared anywhere — a `use` path
    // whose root matches one of these is treated as internal (spec §5.3
    // rule 1's "package imports become Module nodes" only applies to the
    // ones that don't). `Relative` with non-empty `imported_names` is
    // Python's shape (`from .pkg import a`), not Rust's `mod` shape, so
    // it's excluded here — Rust's `mod foo;` always has empty
    // `imported_names` (see `RawImport::Relative`'s doc comment). But
    // empty `imported_names` alone isn't Rust-specific: TS/JS
    // side-effect (`import './x'`), default, namespace, and re-export
    // imports all produce the same shape (ADR-0013), so this is also
    // gated on `fe.declares_module` (`LangExtractor::
    // relative_import_declares_module`, true only for Rust) — otherwise
    // a TS `import './orders'` would make a bare npm package named
    // `orders` classify as internal and silently drop its
    // external-module edge.
    // ADR-0035: partitioned per component (`Option<&str>`, so every file
    // with no component shares the one `None` bucket — exactly today's
    // single repo-wide set, when no component exists anywhere).
    // Deliberately a *strict* partition, not component-first-with-
    // fallback the way the call/type-ref tier ladder below is: a `mod
    // orders;` declared inside one crate is never visible to a *different*
    // crate's `use orders::x` under Rust's own module system, so falling
    // back to a repo-wide check here would resurrect exactly the false
    // suppression this ADR exists to fix (component A's `mod orders;`
    // wrongly marking component B's external `orders` crate as internal,
    // silently dropping the real external-module edge).
    let mut known_modules: BTreeMap<Option<&str>, BTreeSet<&str>> = BTreeMap::new();
    for fe in extractions.iter().filter(|fe| fe.declares_module) {
        for imp in &fe.extract.imports {
            if let RawImport::Relative {
                module_path,
                imported_names,
                ..
            } = imp
            {
                if imported_names.is_empty() {
                    known_modules
                        .entry(fe.component.as_deref())
                        .or_default()
                        .insert(module_path.as_str());
                }
            }
        }
    }
    let is_known_module = |fe: &FileExtraction, root: &str| -> bool {
        known_modules
            .get(&fe.component.as_deref())
            .is_some_and(|s| s.contains(root))
    };

    let relpath_to_file_id: BTreeMap<&str, &NodeId> = extractions
        .iter()
        .map(|fe| (fe.relpath.as_str(), &fe.file_id))
        .collect();

    // ADR-0036: every file's own component, keyed by its `NodeId` rather
    // than `[fi]` index — needed at each file->file `Imports` edge site
    // below, several of which resolve a target only as a `&NodeId`
    // (`resolve_relative_import`, `resolve_fqn`) with no extraction
    // index in hand. A file with no component maps to `None`, same as
    // `file_component`'s `[fi]`-indexed twin below (built further down,
    // for sites that already have a `usize` index instead).
    let file_id_to_component: BTreeMap<&NodeId, Option<&str>> = extractions
        .iter()
        .map(|fe| (&fe.file_id, fe.component.as_deref()))
        .collect();

    // PHP's `use App\Orders\Order;` (ADR-0012, `RawImport::Qualified`) —
    // a fully-qualified name with no path semantics, resolved against a
    // repo-wide index instead of a directory walk. `known_namespace_roots`
    // is the first `\`-segment of every file's declared namespace (a
    // `use` whose root matches one is internal-but-maybe-unresolvable,
    // same "internal, no guessing" honesty as an unresolvable `mod`).
    // `fqn_to_file` maps each *top-level* declaration's fully-qualified
    // name (namespace + bare name; methods excluded — a `use` never
    // names a method) to its declaring file. A file with no `namespace`
    // declaration contributes its bare names under PHP's global
    // namespace. A colliding key (two files illegally declaring the same
    // FQN) keeps whichever file is encountered first rather than
    // erroring — carto only reads source, it doesn't enforce PHP's own
    // rules.
    // Splitting/composition uses each file's own `ns_separator` (`\` for
    // PHP, `.` for C# — ADR-0016) rather than a hard-coded `\`, but the
    // separator spelling alone does NOT make cross-language keys unequal
    // by construction: a *root segment* or a single-segment name
    // contains no separator at all (`namespace System;` in C# and
    // `namespace System\Legacy;` in PHP both split to root `System`;
    // `namespace App;` is the same string in both languages' `.`/`\`
    // schemes). So every index below is additionally keyed by
    // `fe.origin` — the actual per-language discriminant already on
    // every `FileExtraction` — not just the namespace string.
    //
    // ADR-0035: deliberately *not* made component-aware, unlike
    // `fqn_to_file`/`namespace_to_files` below — this index's only job
    // is "internal but unresolvable, so stay silent instead of guessing
    // external" (outcome 2 of `Qualified`/`NamespaceImport` below), and
    // a namespace root declared *anywhere* in the repo is still good
    // evidence the root is the repo's own, not a third-party package,
    // regardless of which component declared it. Narrowing this to the
    // caller's own component would risk the opposite failure mode this
    // ADR is trying to avoid: misclassifying a genuinely-internal-but-
    // cross-component import as external and fabricating a spurious
    // `Module` node for it.
    let known_namespace_roots: BTreeSet<(&str, &str)> = extractions
        .iter()
        .filter_map(|fe| {
            let ns = fe.extract.declared_namespace.as_deref()?;
            let root = ns.split(fe.ns_separator).next()?;
            Some((fe.origin, root))
        })
        .filter(|(_, root)| !root.is_empty())
        .collect();

    // ADR-0035: every declaring file per FQN, not just the first —
    // `resolve_fqn` below picks a same-component one when the caller has
    // a component and one exists, falling back to the *first* overall
    // (index order, i.e. `extractions`' own order) otherwise — exactly
    // preserving the pre-ADR-0035 `.or_insert` "first file wins"
    // behavior for every case component-scoping doesn't help. Two
    // components each illegally-from-carto's-view declaring the same FQN
    // (a real, not-illegal occurrence in a monorepo — two services each
    // scoped under one shared namespace root) previously resolved to
    // whichever file `extractions` happened to list first, an arbitrary
    // choice with no relationship to which file a given `use` actually
    // meant; preferring the caller's own component first is almost
    // always the intended target.
    let mut fqn_to_file: FqnCandidates = BTreeMap::new();
    for fe in &extractions {
        let ns = fe.extract.declared_namespace.as_deref().unwrap_or("");
        for sym in &fe.extract.symbols {
            if matches!(sym.sym_kind, SymKind::Method) {
                continue;
            }
            let fqn = if ns.is_empty() {
                sym.name.clone()
            } else {
                format!("{ns}{}{}", fe.ns_separator, sym.name)
            };
            fqn_to_file
                .entry((fe.origin, fqn))
                .or_default()
                .push((&fe.file_id, fe.component.as_deref()));
        }
    }
    let resolve_fqn = |origin: &'static str, fqn: &str, caller_component: Option<&str>| {
        let candidates = fqn_to_file.get(&(origin, fqn.to_string()))?;
        if let Some(component) = caller_component {
            if let Some((id, _)) = candidates.iter().find(|(_, c)| *c == Some(component)) {
                return Some(*id);
            }
            // ADR-0039: between same-component (above) and the
            // arbitrary first-file-wins fallback (below), prefer a
            // candidate in one of the caller's own *declared
            // dependencies* — still just a preference among an
            // already-existing ambiguity, never a new filter (this can
            // only turn one arbitrary choice into a better-justified
            // one, never manufacture a match where none existed).
            if let Some(deps) = component_depends_on.get(component) {
                if let Some((id, _)) = candidates
                    .iter()
                    .find(|(_, c)| c.is_some_and(|c| deps.contains(c)))
                {
                    return Some(*id);
                }
            }
        }
        candidates.first().map(|(id, _)| *id)
    };

    // C#'s `using Acme.Orders;` (ADR-0016, `RawImport::NamespaceImport`)
    // resolves against declared namespaces *as wholes*, fanning out one
    // edge per declaring file — a namespace spans files the way a Go
    // package spans a directory. PHP files enter this index too
    // (harmlessly — PHP never emits `NamespaceImport`), but are kept
    // apart by the `fe.origin` key component for the same reason as
    // `known_namespace_roots`/`fqn_to_file` above.
    let mut namespace_to_files: BTreeMap<(&str, &str), Vec<usize>> = BTreeMap::new();
    for (fi, fe) in extractions.iter().enumerate() {
        if let Some(ns) = fe.extract.declared_namespace.as_deref() {
            namespace_to_files
                .entry((fe.origin, ns))
                .or_default()
                .push(fi);
        }
    }

    // Each symbol's stable ID, indexed the same way as
    // `extractions[i].extract.symbols[j]` so the two stay in lockstep.
    let symbol_ids: Vec<Vec<NodeId>> = extractions
        .iter()
        .map(|fe| {
            fe.extract
                .symbols
                .iter()
                .map(|s| {
                    graph::sym_id(
                        &fe.relpath,
                        s.sym_kind.as_str(),
                        &s.qualified_name,
                        s.start_line,
                    )
                })
                .collect()
        })
        .collect();

    // Spec §5.3 rule 2 tier (a): same-file, any visibility. All indices
    // per name, not just one — a name declared more than once in the
    // same file (two classes with a same-named method, a redefined
    // Python function, TS declaration merging) is *ambiguous*, and must
    // produce no edge (INV-8), not silently resolve to whichever
    // declaration happened to be inserted last.
    let same_file_by_name: Vec<BTreeMap<&str, Vec<usize>>> = extractions
        .iter()
        .map(|fe| {
            let mut m: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
            for (si, s) in fe.extract.symbols.iter().enumerate() {
                m.entry(s.name.as_str()).or_default().push(si);
            }
            m
        })
        .collect();

    // Spec §5.3 rule 2 tiers (b)/(c): every *exported* symbol, by bare
    // name, repo-wide — deliberately not scoped further per-file, since
    // v1 doesn't walk `use` paths deep enough to verify a specific import
    // resolves to a specific file (see ADR-0008). A name with more than
    // one exported candidate is ambiguous for both tiers; only an exactly-
    // one match resolves (INV-8: no guessing).
    let mut pub_by_name: BTreeMap<&str, Vec<(usize, usize)>> = BTreeMap::new();
    for (fi, fe) in extractions.iter().enumerate() {
        for (si, sym) in fe.extract.symbols.iter().enumerate() {
            if sym.is_pub {
                pub_by_name
                    .entry(sym.name.as_str())
                    .or_default()
                    .push((fi, si));
            }
        }
    }

    // ADR-0035: each file's own component label, in the same `[fi]`
    // lockstep every other per-file vector here uses, plus
    // `pub_by_name`'s exact shape further split by `(component, name)` —
    // backs tiers (b1)/(c1) in `CallResolver::resolve`. A file with no
    // component contributes nothing here (it's still in `pub_by_name`);
    // a repo with no components anywhere leaves this map empty, so tiers
    // (b1)/(c1) never fire and every evidence string below stays
    // byte-identical to before this ADR.
    let file_component: Vec<Option<&str>> = extractions
        .iter()
        .map(|fe| fe.component.as_deref())
        .collect();
    let mut pub_by_component: BTreeMap<(&str, &str), Vec<(usize, usize)>> = BTreeMap::new();
    for (fi, fe) in extractions.iter().enumerate() {
        let Some(component) = fe.component.as_deref() else {
            continue;
        };
        for (si, sym) in fe.extract.symbols.iter().enumerate() {
            if sym.is_pub {
                pub_by_component
                    .entry((component, sym.name.as_str()))
                    .or_default()
                    .push((fi, si));
            }
        }
    }

    // Bound-name -> declared-name, per file — tier (b)'s precondition
    // *and* its resolution key in a single map, not two independently-
    // matched strings the way an `imported_names` set + a same-string
    // `pub_by_name.get` used to be. A call site's own identifier is
    // checked against this map's keys; a hit's *value* is what's
    // actually looked up in `pub_by_name` below — the same string only
    // for an unaliased import (`bound_name == declared_name`), which is
    // exactly what makes this alias-aware without changing behavior for
    // every import that isn't aliased. `Relative`/`Absolute` carry
    // `ImportedName` pairs directly; `Qualified`'s FQN's own last
    // segment already *is* the declared name, so nothing extra is
    // needed there. See
    // `docs/adr/0013-typescript-javascript-resolution-policy-mapping.md`.
    let alias_to_declared: Vec<BTreeMap<&str, &str>> = extractions
        .iter()
        .map(|fe| {
            let mut m = BTreeMap::new();
            for imp in &fe.extract.imports {
                match imp {
                    RawImport::Relative { imported_names, .. }
                    | RawImport::Absolute { imported_names, .. } => {
                        for n in imported_names {
                            m.insert(n.bound_name.as_str(), n.declared_name.as_str());
                        }
                    }
                    RawImport::Qualified { fqn, bound_name } => {
                        let declared = fqn.rsplit(fe.ns_separator).next().unwrap_or(fqn.as_str());
                        m.insert(bound_name.as_str(), declared);
                    }
                    // Go's package-import binds a *package* name, never a
                    // symbol name — nothing to pair here. See
                    // `RawImport::PackagePath`'s own doc comment
                    // (ADR-0015): Go can never feed tier (b). C#'s
                    // namespace `using` is the same shape (ADR-0016): it
                    // makes a whole namespace's types visible without
                    // binding any one name. A bare reference (ADR-0024)
                    // binds no name at all either — same reasoning.
                    RawImport::PackagePath { .. }
                    | RawImport::NamespaceImport { .. }
                    | RawImport::BareReference { .. } => {}
                }
            }
            m
        })
        .collect();

    // Go's directory-scoped visibility (ADR-0015): files in one directory
    // see each other's symbols regardless of visibility, with no import
    // at all — a shape none of Rust/Python/PHP/TS/JS have. Built only
    // from `dir_scoped` files (every other language's extractor leaves
    // this `false`, so these maps are empty and tier (a′) never fires for
    // them). `file_dir` also backs `RawImport::PackagePath` resolution
    // below, which needs "every directory containing a Go file" the same
    // shape describes.
    let file_dir: Vec<&str> = extractions
        .iter()
        .map(|fe| relpath_dir(&fe.relpath))
        .collect();
    let dir_scoped: Vec<bool> = extractions.iter().map(|fe| fe.dir_scoped).collect();

    let mut same_dir_by_name: BTreeMap<(&str, &str), Vec<(usize, usize)>> = BTreeMap::new();
    let mut dir_files: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (fi, fe) in extractions.iter().enumerate() {
        if !fe.dir_scoped {
            continue;
        }
        let dir = file_dir[fi];
        dir_files.entry(dir).or_default().push(fi);
        for (si, sym) in fe.extract.symbols.iter().enumerate() {
            same_dir_by_name
                .entry((dir, sym.name.as_str()))
                .or_default()
                .push((fi, si));
        }
    }

    // §1.1's honest-absence signal (ADR-0020): repo-wide count of call
    // sites whose bare callee name matches, keyed by `(origin, name)` —
    // the same per-language keying `fqn_to_file`/`known_namespace_roots`
    // use, so a Rust `graph::load()` call site never inflates a Python
    // symbol also named `load`. Every extractor except Rust's leaves
    // `uncaptured_call_sites` empty today, so this map is empty for them
    // and every symbol's count stays 0 — unchanged behavior.
    let mut uncaptured_by_name: BTreeMap<(&'static str, &str), u32> = BTreeMap::new();
    for fe in &extractions {
        for call in &fe.extract.uncaptured_call_sites {
            *uncaptured_by_name
                .entry((fe.origin, call.callee_name.as_str()))
                .or_insert(0) += 1;
        }
    }

    // ADR-0032's owner-type disambiguation tier: two indices, in
    // lockstep with `symbol_ids`/`same_file_by_name`'s own per-`(fi,
    // si)` shape. `sym_owner` is each symbol's `RawSymbol::owner`
    // (`None` for a free function/top-level type); `type_ref_names` is,
    // per file, every type name that file's own `type_refs` names in a
    // type position (a field/parameter/base-clause type — the same
    // `RawTypeRef` channel ADR-0029 added for `references` edges).
    // Together they answer "does the calling file name this candidate's
    // owner type anywhere" — the evidence this tier requires before
    // narrowing an otherwise-ambiguous bare-name match.
    let sym_owner: Vec<Vec<Option<&str>>> = extractions
        .iter()
        .map(|fe| {
            fe.extract
                .symbols
                .iter()
                .map(|s| s.owner.as_deref())
                .collect()
        })
        .collect();
    let type_ref_names: Vec<BTreeSet<&str>> = extractions
        .iter()
        .map(|fe| {
            fe.extract
                .type_refs
                .iter()
                .map(|t| t.name.as_str())
                .collect()
        })
        .collect();

    let resolver = CallResolver {
        same_file_by_name: &same_file_by_name,
        alias_to_declared: &alias_to_declared,
        sym_owner: &sym_owner,
        type_ref_names: &type_ref_names,
        pub_by_name: &pub_by_name,
        dir_scoped: &dir_scoped,
        file_dir: &file_dir,
        same_dir_by_name: &same_dir_by_name,
        file_component: &file_component,
        pub_by_component: &pub_by_component,
    };

    // ADR-0033's inbound honesty pre-pass: every *attempted* call site
    // repo-wide (every `call_sites` entry, attached to a symbol or not —
    // ADR-0031 file-scope calls spell a name too) that this resolver
    // couldn't resolve, keyed by `(origin, name)` exactly like
    // `uncaptured_by_name` above, so a Rust `load` call never inflates a
    // Python `load` symbol's count. Distinct from `uncaptured_by_name`:
    // that counts syntax never even attempted; this counts syntax that
    // *was* attempted and still produced no edge (the common case being
    // bare-name ambiguity — an interface method and its implementation).
    // A second resolve() pass over the same call sites the node loop
    // below resolves again is deliberate, not an oversight: `resolve()`
    // is a pure BTreeMap-lookup function, and keeping this pass separate
    // (rather than threading a second output channel through the
    // per-symbol loop) keeps the two honesty signals' bookkeeping
    // independent, each easy to verify on its own.
    let mut unresolved_by_name: BTreeMap<(&'static str, &str), Vec<InboundCallSite>> =
        BTreeMap::new();
    for (fi, fe) in extractions.iter().enumerate() {
        for call in &fe.extract.call_sites {
            if resolver.resolve(fi, call.callee_name.as_str()).is_none() {
                unresolved_by_name
                    .entry((fe.origin, call.callee_name.as_str()))
                    .or_default()
                    .push(InboundCallSite {
                        file: fe.relpath.clone(),
                        line: call.line,
                        component: fe.component.clone(),
                    });
            }
        }
    }

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut external_modules: BTreeMap<&str, NodeId> = BTreeMap::new();

    // ADR-0041: Terraform references resolve in their own
    // directory-scoped pass, never through the tier ladder above.
    let mut tf = super::terraform::resolve(&extractions, &symbol_ids);
    edges.append(&mut tf.edges);

    for (fi, fe) in extractions.iter().enumerate() {
        let (calls_by_symbol, unattached_calls) =
            assign_to_innermost_symbol(&fe.extract.symbols, &fe.extract.call_sites, |c| c.line);
        // ADR-0023, the outbound half of §1.1's honest-absence signal:
        // the same innermost-containment assignment `calls_by_symbol`
        // uses, applied to the *uncaptured* call sites instead —
        // answers "how many of this symbol's own calls did the
        // extractor never even attempt", not "who calls this symbol
        // that the extractor missed" (that's `uncaptured_by_name`,
        // below, keyed by name repo-wide rather than by containment).
        // Its own leftover bucket is unused: `uncaptured_call_sites`
        // today comes only from Rust's path-qualified-call exclusion,
        // which never occurs in top-level-statement-shaped code, and
        // its repo-wide counting doesn't go through this function's
        // per-symbol assignment at all (see `uncaptured_by_name`).
        let (uncaptured_by_symbol, _) = assign_to_innermost_symbol(
            &fe.extract.symbols,
            &fe.extract.uncaptured_call_sites,
            |c| c.line,
        );
        // ADR-0029: type refs get the same innermost-containment
        // assignment as calls — a field/parameter/base-clause type
        // name is attributed to its own enclosing symbol exactly like
        // a call site would be.
        let (type_refs_by_symbol, unattached_type_refs) =
            assign_to_innermost_symbol(&fe.extract.symbols, &fe.extract.type_refs, |t| t.line);

        for (si, sym) in fe.extract.symbols.iter().enumerate() {
            let id = symbol_ids[fi][si].clone();

            let mut unresolved_calls = tf.unresolved.remove(&(fi, si)).unwrap_or_default();
            for &call in &calls_by_symbol[si] {
                match resolver.resolve(fi, call.callee_name.as_str()) {
                    Some((target_fi, target_si, evidence)) => {
                        let to_id = symbol_ids[target_fi][target_si].clone();
                        let mut edge = Edge::new(
                            EdgeKind::Calls,
                            id.clone(),
                            to_id,
                            Confidence::Inferred,
                            evidence.to_string(),
                        );
                        if is_undeclared_dependency(
                            &component_depends_on,
                            file_component[fi],
                            file_component[target_fi],
                        ) {
                            edge.evidence.push("undeclared-dependency".to_string());
                        }
                        edges.push(edge);
                    }
                    None => unresolved_calls.push(UnresolvedCall {
                        name: call.callee_name.clone(),
                        line: call.line,
                    }),
                }
            }

            // ADR-0029: a type ref goes through the identical tier
            // ladder a call would, since it's the same "which
            // declaration does this bare name mean" question. Unlike
            // calls, an unresolved type ref is silently dropped (see
            // `RawTypeRef`'s doc comment) — no `unresolved_`-style
            // list, and a resolved self-reference (a field/generic
            // naming its own enclosing type) is suppressed rather than
            // emitted, since a symbol referencing itself is not a
            // dependency edge worth reporting.
            for &tref in &type_refs_by_symbol[si] {
                if let Some((target_fi, target_si, evidence)) =
                    resolver.resolve(fi, tref.name.as_str())
                {
                    let to_id = symbol_ids[target_fi][target_si].clone();
                    if to_id != id {
                        let mut edge = Edge::new(
                            EdgeKind::References,
                            id.clone(),
                            to_id,
                            Confidence::Inferred,
                            format!("type-reference:{evidence}"),
                        );
                        if is_undeclared_dependency(
                            &component_depends_on,
                            file_component[fi],
                            file_component[target_fi],
                        ) {
                            edge.evidence.push("undeclared-dependency".to_string());
                        }
                        edges.push(edge);
                    }
                }
            }

            let signature = if sym.signature.trim().is_empty() {
                None
            } else {
                Some(TaintedString::new(&sym.signature, Provenance::Syntactic))
            };

            let uncaptured_inbound_calls = uncaptured_by_name
                .get(&(fe.origin, sym.name.as_str()))
                .copied()
                .unwrap_or(0);
            let uncaptured_outbound_calls = uncaptured_by_symbol[si].len() as u32;

            // ADR-0033: capped list + uncapped count, same pairing
            // `unresolved_inbound_calls`'s doc comment describes. Sorted
            // by `(file, line)` — `InboundCallSite`'s derived `Ord` —
            // rather than relying on `unresolved_by_name`'s incidental
            // insertion order, so the rendered list is legible on its
            // own and stays order-stable regardless of extraction order
            // (INV-7).
            let unresolved_inbound_call_count = unresolved_by_name
                .get(&(fe.origin, sym.name.as_str()))
                .map(|sites| sites.len() as u32)
                .unwrap_or(0);
            let mut unresolved_inbound_calls = unresolved_by_name
                .get(&(fe.origin, sym.name.as_str()))
                .cloned()
                .unwrap_or_default();
            unresolved_inbound_calls.sort();
            unresolved_inbound_calls.truncate(consts::UNRESOLVED_INBOUND_SITES_CAP);

            nodes.push(Node::symbol(
                id.clone(),
                Provenance::Syntactic,
                fe.origin,
                SymbolNode {
                    name: sym.name.clone(),
                    sym_kind: sym.sym_kind,
                    file: fe.file_id.clone(),
                    start_line: sym.start_line,
                    end_line: sym.end_line,
                    signature,
                    unresolved_calls,
                    uncaptured_inbound_calls,
                    uncaptured_outbound_calls,
                    unresolved_inbound_calls,
                    unresolved_inbound_call_count,
                },
            ));

            edges.push(Edge::new(
                EdgeKind::Contains,
                fe.file_id.clone(),
                id,
                Confidence::Certain,
                fe.origin.to_string(),
            ));
        }

        // ADR-0031: a call or type ref with no enclosing symbol at all
        // (top-level-statement code — see `assign_to_innermost_symbol`'s
        // own doc comment) is resolved at *file* scope instead of being
        // silently dropped, through the identical tier ladder the
        // symbol-scoped loops above already use — `resolve_call` itself
        // doesn't care whether the caller is a symbol or a file. Only
        // the *resolved* case is handled (user-confirmed scope): an
        // unattached item that fails to resolve is simply not pushed,
        // the same invisible-miss behavior as before this ADR — there
        // is no `FileNode` equivalent of `unresolved_calls` yet, and
        // adding one is a separate, unscoped decision. No self-
        // reference suppression is needed the way the symbol-scoped
        // type-ref loop above needs one: a `File` ID and a `Symbol` ID
        // are never equal.
        for &call in &unattached_calls {
            if let Some((target_fi, target_si, evidence)) =
                resolver.resolve(fi, call.callee_name.as_str())
            {
                let to_id = symbol_ids[target_fi][target_si].clone();
                let mut edge = Edge::new(
                    EdgeKind::Calls,
                    fe.file_id.clone(),
                    to_id,
                    Confidence::Inferred,
                    evidence.to_string(),
                );
                if is_undeclared_dependency(
                    &component_depends_on,
                    file_component[fi],
                    file_component[target_fi],
                ) {
                    edge.evidence.push("undeclared-dependency".to_string());
                }
                edges.push(edge);
            }
        }
        for &tref in &unattached_type_refs {
            if let Some((target_fi, target_si, evidence)) = resolver.resolve(fi, tref.name.as_str())
            {
                let to_id = symbol_ids[target_fi][target_si].clone();
                let mut edge = Edge::new(
                    EdgeKind::References,
                    fe.file_id.clone(),
                    to_id,
                    Confidence::Inferred,
                    format!("type-reference:{evidence}"),
                );
                if is_undeclared_dependency(
                    &component_depends_on,
                    file_component[fi],
                    file_component[target_fi],
                ) {
                    edge.evidence.push("undeclared-dependency".to_string());
                }
                edges.push(edge);
            }
        }

        for imp in &fe.extract.imports {
            match imp {
                RawImport::Relative {
                    levels_up,
                    module_path,
                    imported_names,
                } => {
                    if imported_names.is_empty() || !module_path.is_empty() {
                        // The module itself is the file-level target;
                        // one edge, certain per spec rule 1, regardless
                        // of how many names are drawn from it. Covers
                        // Rust's `mod foo;` (empty names — evidence
                        // "mod-declaration", gated on `fe.declares_module`
                        // (`LangExtractor::relative_import_declares_module`,
                        // true only for Rust): TS/JS side-effect/default/
                        // namespace/re-export imports produce the same
                        // empty-names shape, ADR-0013, and are ordinary
                        // relative imports, not mod declarations) and
                        // Python's `from .pkg import a, b` / TS's `import
                        // x from './pkg'`.
                        let evidence = if fe.declares_module {
                            "mod-declaration"
                        } else {
                            "relative-import"
                        };
                        if let Some(target) = resolve_relative_import(
                            &fe.relpath,
                            *levels_up,
                            module_path,
                            &relpath_to_file_id,
                        ) {
                            let target_component =
                                file_id_to_component.get(target).copied().flatten();
                            let mut edge = Edge::new(
                                EdgeKind::Imports,
                                fe.file_id.clone(),
                                target.clone(),
                                Confidence::Certain,
                                evidence.to_string(),
                            );
                            if let Some(marker) =
                                cross_component_marker(fe.component.as_deref(), target_component)
                            {
                                edge.evidence.push(marker.to_string());
                            }
                            if is_undeclared_dependency(
                                &component_depends_on,
                                fe.component.as_deref(),
                                target_component,
                            ) {
                                edge.evidence.push("undeclared-dependency".to_string());
                            }
                            edges.push(edge);
                        }
                    } else {
                        // Python's `from . import pkg[, pkg2]` — each
                        // name IS itself a submodule to resolve relative
                        // to the current directory (no separate
                        // `module_path` to anchor on). Resolved by
                        // `declared_name` (the real submodule/file name),
                        // not `bound_name` — an alias (`from . import
                        // pkg as p`) renames the local binding, not the
                        // file on disk.
                        for name in imported_names {
                            if let Some(target) = resolve_relative_import(
                                &fe.relpath,
                                *levels_up,
                                &name.declared_name,
                                &relpath_to_file_id,
                            ) {
                                let target_component =
                                    file_id_to_component.get(target).copied().flatten();
                                let mut edge = Edge::new(
                                    EdgeKind::Imports,
                                    fe.file_id.clone(),
                                    target.clone(),
                                    Confidence::Certain,
                                    "relative-import".to_string(),
                                );
                                if let Some(marker) = cross_component_marker(
                                    fe.component.as_deref(),
                                    target_component,
                                ) {
                                    edge.evidence.push(marker.to_string());
                                }
                                if is_undeclared_dependency(
                                    &component_depends_on,
                                    fe.component.as_deref(),
                                    target_component,
                                ) {
                                    edge.evidence.push("undeclared-dependency".to_string());
                                }
                                edges.push(edge);
                            }
                        }
                    }
                }
                RawImport::Absolute { root, .. } => {
                    // `known_modules` only ever contains Rust `mod`
                    // names (see its own doc comment) — Python has no
                    // walked-repo equivalent (no explicit module
                    // declaration to collect), so every Python absolute
                    // import is classified external here, even when it
                    // actually names a local top-level package. A known
                    // v1 simplification (ADR-0011), analogous to but
                    // distinct from Rust's own "same-package = same
                    // walked repo" one.
                    let is_internal = root == "crate"
                        || root == "self"
                        || root == "super"
                        || is_known_module(fe, root);
                    if is_internal {
                        continue;
                    }
                    let module_id = external_modules
                        .entry(root.as_str())
                        .or_insert_with(|| {
                            let id = graph::module_id(root, true);
                            nodes.push(Node::module(
                                id.clone(),
                                Provenance::Syntactic,
                                fe.origin,
                                ModuleNode {
                                    path: root.clone(),
                                    external: true,
                                },
                            ));
                            id
                        })
                        .clone();
                    edges.push(Edge::new(
                        EdgeKind::Imports,
                        fe.file_id.clone(),
                        module_id,
                        Confidence::Certain,
                        "external-package".to_string(),
                    ));
                }
                RawImport::BareReference { root } => {
                    // ADR-0024: same internal/external classification
                    // `Absolute` uses immediately above (a bare
                    // reference to a genuinely internal module — e.g.
                    // `orders::helper()` naming a local `mod orders;`
                    // in a call-callee-excluded position — is still
                    // internal, no edge needed since call resolution's
                    // own tiers already cover same-repo references).
                    // Only the evidence string differs, so a caller can
                    // tell this heuristic detection apart from a
                    // verified `use` declaration.
                    let is_internal = root == "crate"
                        || root == "self"
                        || root == "super"
                        || is_known_module(fe, root);
                    if is_internal {
                        continue;
                    }
                    let module_id = external_modules
                        .entry(root.as_str())
                        .or_insert_with(|| {
                            let id = graph::module_id(root, true);
                            nodes.push(Node::module(
                                id.clone(),
                                Provenance::Syntactic,
                                fe.origin,
                                ModuleNode {
                                    path: root.clone(),
                                    external: true,
                                },
                            ));
                            id
                        })
                        .clone();
                    edges.push(Edge::new(
                        EdgeKind::Imports,
                        fe.file_id.clone(),
                        module_id,
                        Confidence::Certain,
                        "external-package-bare-reference".to_string(),
                    ));
                }
                RawImport::Qualified { fqn, .. } => {
                    // PHP's `use App\Orders\Order;` (ADR-0012) and C#'s
                    // `using F = X.Y.Z;` (ADR-0016) — resolved against
                    // the repo-wide FQN index built above, not a
                    // directory walk. Three outcomes, in order:
                    let fqn = fqn.trim_start_matches(fe.ns_separator);
                    if let Some(target) = resolve_fqn(fe.origin, fqn, fe.component.as_deref()) {
                        // 1. Exact FQN match: certain edge to the
                        //    declaring file.
                        let target_component = file_id_to_component.get(target).copied().flatten();
                        let mut edge = Edge::new(
                            EdgeKind::Imports,
                            fe.file_id.clone(),
                            target.clone(),
                            Confidence::Certain,
                            "namespace-import".to_string(),
                        );
                        if let Some(marker) =
                            cross_component_marker(fe.component.as_deref(), target_component)
                        {
                            edge.evidence.push(marker.to_string());
                        }
                        if is_undeclared_dependency(
                            &component_depends_on,
                            fe.component.as_deref(),
                            target_component,
                        ) {
                            edge.evidence.push("undeclared-dependency".to_string());
                        }
                        edges.push(edge);
                    } else if fqn
                        .split(fe.ns_separator)
                        .next()
                        .is_some_and(|root| known_namespace_roots.contains(&(fe.origin, root)))
                    {
                        // 2. Internal namespace root, but no file
                        //    declares this exact FQN: honest omission
                        //    (INV-8) rather than a guess — no edge, no
                        //    node.
                    } else {
                        // 3. Unknown root: an external package. PHP
                        //    dedups by root (Composer-style: `Psr\Log\X`
                        //    and `Psr\Http\Y` share one `Psr` node); C#
                        //    keys by the *whole* FQN instead
                        //    (`qualified_external_is_full_fqn`,
                        //    ADR-0016) — `System.Text.Json.JsonSerializer`
                        //    must not collapse onto the same node as a
                        //    plain `using System;`, matching
                        //    `NamespaceImport`'s own full-string policy
                        //    below.
                        let key = if fe.qualified_external_is_full_fqn {
                            fqn
                        } else {
                            fqn.split(fe.ns_separator).next().unwrap_or(fqn)
                        };
                        let module_id = external_modules
                            .entry(key)
                            .or_insert_with(|| {
                                let id = graph::module_id(key, true);
                                nodes.push(Node::module(
                                    id.clone(),
                                    Provenance::Syntactic,
                                    fe.origin,
                                    ModuleNode {
                                        path: key.to_string(),
                                        external: true,
                                    },
                                ));
                                id
                            })
                            .clone();
                        edges.push(Edge::new(
                            EdgeKind::Imports,
                            fe.file_id.clone(),
                            module_id,
                            Confidence::Certain,
                            "external-package".to_string(),
                        ));
                    }
                }
                RawImport::NamespaceImport { path } => {
                    // C#'s `using Acme.Orders;` (ADR-0016) — matched
                    // exactly against every file's declared namespace.
                    // Three outcomes, mirroring `Qualified`'s, except a
                    // hit fans out per declaring file (a namespace spans
                    // files the way a Go package spans a directory):
                    if let Some(target_fis) = namespace_to_files.get(&(fe.origin, path.as_str())) {
                        // 1. Some walked file declares exactly this
                        //    namespace: one certain edge per such file —
                        //    ADR-0035: narrowed to the caller's own
                        //    component's declaring files when at least
                        //    one exists, so `using Acme.Orders;` in
                        //    service "orders" fans out only within
                        //    "orders", not also into every other
                        //    service that happens to declare the same
                        //    namespace (the exact over-broad-fan-out
                        //    shape ADR-0029's field report traced
                        //    `imports` itself to). Falls back to the
                        //    full repo-wide fan-out — today's unchanged
                        //    behavior — when the caller has no
                        //    component, or no same-component file
                        //    declares it.
                        let same_component: Vec<usize> = target_fis
                            .iter()
                            .copied()
                            .filter(|&tfi| {
                                extractions[tfi].component.as_deref() == fe.component.as_deref()
                            })
                            .collect();
                        // ADR-0039: between same-component (above) and
                        // the full repo-wide fan-out (below), prefer
                        // narrowing to the caller's own *declared
                        // dependencies* when at least one declaring
                        // file's component qualifies — the same
                        // "prefer, fall through, never a new filter"
                        // discipline `resolve_fqn` above uses. Only
                        // reached when `same_component` is empty, so
                        // this never overrides the same-component
                        // narrowing, only the arbitrary full-fan-out
                        // default.
                        let declared_dep: Vec<usize> = fe
                            .component
                            .as_deref()
                            .and_then(|c| component_depends_on.get(c))
                            .map(|deps| {
                                target_fis
                                    .iter()
                                    .copied()
                                    .filter(|&tfi| {
                                        extractions[tfi]
                                            .component
                                            .as_deref()
                                            .is_some_and(|c| deps.contains(c))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        let chosen: &[usize] =
                            if fe.component.is_some() && !same_component.is_empty() {
                                &same_component
                            } else if !declared_dep.is_empty() {
                                &declared_dep
                            } else {
                                target_fis
                            };
                        for &target_fi in chosen {
                            if target_fi == fi {
                                continue; // no self-edge
                            }
                            let mut edge = Edge::new(
                                EdgeKind::Imports,
                                fe.file_id.clone(),
                                extractions[target_fi].file_id.clone(),
                                Confidence::Certain,
                                "namespace-import".to_string(),
                            );
                            if let Some(marker) = cross_component_marker(
                                fe.component.as_deref(),
                                file_component[target_fi],
                            ) {
                                edge.evidence.push(marker.to_string());
                            }
                            if is_undeclared_dependency(
                                &component_depends_on,
                                fe.component.as_deref(),
                                file_component[target_fi],
                            ) {
                                edge.evidence.push("undeclared-dependency".to_string());
                            }
                            edges.push(edge);
                        }
                    } else if path
                        .split(fe.ns_separator)
                        .next()
                        .is_some_and(|root| known_namespace_roots.contains(&(fe.origin, root)))
                    {
                        // 2. Internal namespace root, but no file
                        //    declares this exact namespace (e.g. `using
                        //    Acme;` in a repo that only declares
                        //    `Acme.Orders`): honest omission (INV-8) —
                        //    no edge, no node.
                    } else {
                        // 3. Unknown root: an external namespace, keyed
                        //    by the *full* string — `System.Text.Json`
                        //    is not the same package as `System`, so no
                        //    truncation to a root (same full-identity
                        //    reasoning as Go's import paths, ADR-0015).
                        let module_id = external_modules
                            .entry(path.as_str())
                            .or_insert_with(|| {
                                let id = graph::module_id(path, true);
                                nodes.push(Node::module(
                                    id.clone(),
                                    Provenance::Syntactic,
                                    fe.origin,
                                    ModuleNode {
                                        path: path.clone(),
                                        external: true,
                                    },
                                ));
                                id
                            })
                            .clone();
                        edges.push(Edge::new(
                            EdgeKind::Imports,
                            fe.file_id.clone(),
                            module_id,
                            Confidence::Certain,
                            "external-package".to_string(),
                        ));
                    }
                }
                RawImport::PackagePath { path } => {
                    // Go's `import "github.com/acme/svc/internal/orders"`
                    // (ADR-0015) — resolved against `dir_files` (every
                    // walked directory containing a Go file) by longest
                    // suffix match, not a `go.mod`-anchored exact path.
                    match resolve_go_package_path(path, &dir_files) {
                        Some(dir) => {
                            // One edge per file in the target directory:
                            // Go's import unit is the package (directory),
                            // not a single file the way every other
                            // language's import target is.
                            for &target_fi in &dir_files[dir] {
                                if target_fi == fi {
                                    continue; // no self-edge
                                }
                                let mut edge = Edge::new(
                                    EdgeKind::Imports,
                                    fe.file_id.clone(),
                                    extractions[target_fi].file_id.clone(),
                                    Confidence::Certain,
                                    "package-import".to_string(),
                                );
                                if let Some(marker) = cross_component_marker(
                                    fe.component.as_deref(),
                                    file_component[target_fi],
                                ) {
                                    edge.evidence.push(marker.to_string());
                                }
                                if is_undeclared_dependency(
                                    &component_depends_on,
                                    fe.component.as_deref(),
                                    file_component[target_fi],
                                ) {
                                    edge.evidence.push("undeclared-dependency".to_string());
                                }
                                edges.push(edge);
                            }
                        }
                        None => {
                            // No walked directory's path matches: an
                            // external package. Keyed by the *full*
                            // import path, not a truncated root — a Go
                            // import path is canonical package identity
                            // (unlike npm's `@scope/pkg/subpath`
                            // convention, where a sub-path is the same
                            // package), so no truncation is correct here.
                            let module_id = external_modules
                                .entry(path.as_str())
                                .or_insert_with(|| {
                                    let id = graph::module_id(path, true);
                                    nodes.push(Node::module(
                                        id.clone(),
                                        Provenance::Syntactic,
                                        fe.origin,
                                        ModuleNode {
                                            path: path.clone(),
                                            external: true,
                                        },
                                    ));
                                    id
                                })
                                .clone();
                            edges.push(Edge::new(
                                EdgeKind::Imports,
                                fe.file_id.clone(),
                                module_id,
                                Confidence::Certain,
                                "external-package".to_string(),
                            ));
                        }
                    }
                }
            }
        }

        // ADR-0026: every literal this file's extractor recognized in a
        // contract-relevant position, classified by `(lang, position)`.
        // An unclassified position (no rule matches) is dropped — same
        // "unmapped beats guessed" honesty `resolve_call` already
        // applies to an ambiguous call. Node/edge duplicates across
        // files (the same metric name produced or consumed more than
        // once) are handled by `Graph::insert_node`'s overwrite and
        // `insert_edge`'s §4.3 merge once `indexer::build_and_persist`
        // folds this Vec in — both are pure functions of
        // (category, qualifier, value)/(kind, from, to), so re-pushing
        // identical content here needs no dedup of its own.
        for lit in &fe.extract.literals {
            let Some((category, role, confidence, evidence_prefix)) =
                contract_rules.classify(fe.lang, &lit.position)
            else {
                continue;
            };

            let attach_id = smallest_containing_symbol(&fe.extract.symbols, lit.line)
                .map(|si| symbol_ids[fi][si].clone())
                .unwrap_or_else(|| fe.file_id.clone());

            let contract_node_id =
                graph::contract_id(category, lit.qualifier.as_deref(), &lit.value);
            nodes.push(Node::contract(
                contract_node_id.clone(),
                Provenance::Syntactic,
                fe.origin,
                ContractNode {
                    category: category.to_string(),
                    qualifier: lit
                        .qualifier
                        .as_deref()
                        .map(|q| TaintedString::new(q, Provenance::Syntactic)),
                    value: TaintedString::new(&lit.value, Provenance::Syntactic),
                },
            ));

            let edge_kind = match role {
                Role::Producer => EdgeKind::Produces,
                Role::Consumer => EdgeKind::Consumes,
            };
            edges.push(Edge::new(
                edge_kind,
                attach_id,
                contract_node_id,
                confidence,
                format!("{evidence_prefix}:{}", lit.position),
            ));
        }
    }

    ResolvedExtraction { nodes, edges }
}

/// The innermost-containment primitive shared by
/// [`assign_to_innermost_symbol`] (batched over every call site or type
/// ref in a file) and the contract-literal pass above
/// (`innermost_symbol_for_line`, one line at a time): the *smallest*
/// symbol (by line-range width) whose range contains `line`, or `None`
/// if no symbol does. A class/interface/trait/enum symbol's own range
/// spans its methods' bodies too (PHP, Python — a Rust `struct`/`impl`
/// doesn't have this shape, since an `impl` block itself is never a
/// symbol), so naive per-symbol containment would attribute a method's
/// call to both the method *and* its enclosing class: two `calls` edges
/// (or two `unresolved_calls` entries) for what is textually one call
/// site — caught by eyeballing real `fixtures/php-app` output
/// (ADR-0012), not by a unit test on an isolated snippet, since
/// `fixtures/py-lib` happens to have no call site inside a class body's
/// methods and never exercised this. Ties (two symbols with identical
/// ranges) fall back to whichever is encountered first — doesn't arise
/// from any current extractor, which never emits two symbols with
/// identical ranges. Kept as one function (rather than two independent
/// copies of the same loop, ADR-0026's contract-literal pass had before
/// this) so a future change to the tie-break rule can't apply to one
/// call site and silently miss the other.
pub(super) fn smallest_containing_symbol(symbols: &[RawSymbol], line: u32) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (si, sym) in symbols.iter().enumerate() {
        if line < sym.start_line || line > sym.end_line {
            continue;
        }
        let smaller = match best {
            None => true,
            Some(b) => {
                (sym.end_line - sym.start_line) < (symbols[b].end_line - symbols[b].start_line)
            }
        };
        if smaller {
            best = Some(si);
        }
    }
    best
}

/// Assigns each item (a call site or a type ref — anything with a
/// `line`) to the *innermost* symbol whose line range contains it,
/// rather than every symbol whose range does — see
/// [`smallest_containing_symbol`] for the containment/tie-break rule
/// itself. Generalized (ADR-0029) from an original `RawCallSite`-only
/// `assign_calls_to_innermost_symbol` once `RawTypeRef` needed the
/// identical assignment; `line_of` is the one difference between the
/// two callers' item shapes.
///
/// Also returns every item `smallest_containing_symbol` couldn't place
/// at all (ADR-0031) — most commonly C#/TS/JS/Python top-level-
/// statement code, which no `symbols.scm` pattern captures as a
/// container of anything (ASP.NET Core's minimal-API `Program.cs`
/// style, with no `Main` method at all, is the concrete case that
/// motivated this). Before ADR-0031, such an item was silently
/// dropped here — never resolved, never counted, never visible
/// anywhere, stricter silence than `unresolved_calls` gives an
/// ordinary same-symbol miss. The two return values partition `items`
/// completely: every item is in exactly one bucket.
fn assign_to_innermost_symbol<'a, T>(
    symbols: &[RawSymbol],
    items: &'a [T],
    line_of: impl Fn(&T) -> u32,
) -> (Vec<Vec<&'a T>>, Vec<&'a T>) {
    let mut by_symbol: Vec<Vec<&T>> = vec![Vec::new(); symbols.len()];
    let mut unattached: Vec<&T> = Vec::new();
    for item in items {
        match smallest_containing_symbol(symbols, line_of(item)) {
            Some(si) => by_symbol[si].push(item),
            None => unattached.push(item),
        }
    }
    (by_symbol, unattached)
}

/// Spec §5.3 rule 2, in order, first match wins: (a) same-file, (a′)
/// same-directory (Go only, ADR-0015), (b) imported into the file, (c)
/// same-package (v1: same walked repo). Returns `(target_file_index,
/// target_symbol_index, evidence)` or `None` if no tier produced exactly
/// one candidate.
///
/// Takes a bare `name` rather than a `&RawCallSite` (ADR-0029) — the
/// question "which declaration does this bare identifier mean" is
/// identical for a call site's callee and a type ref's type name, and
/// this tier ladder is the one place that answers it; `resolve`'s
/// several call sites (calls, type refs, the ADR-0031 file-scope
/// leftovers, and ADR-0033's inbound-honesty pre-pass) each extract
/// their own `name` before calling in. Bundled into a struct (rather
/// than passed as six loose slice/map parameters, as before ADR-0033)
/// since the pre-pass added a fifth call site to what was already
/// `#[allow(clippy::too_many_arguments)]`.
struct CallResolver<'a> {
    same_file_by_name: &'a [BTreeMap<&'a str, Vec<usize>>],
    alias_to_declared: &'a [BTreeMap<&'a str, &'a str>],
    pub_by_name: &'a BTreeMap<&'a str, Vec<(usize, usize)>>,
    dir_scoped: &'a [bool],
    file_dir: &'a [&'a str],
    same_dir_by_name: &'a BTreeMap<(&'a str, &'a str), Vec<(usize, usize)>>,
    /// ADR-0032: each symbol's `RawSymbol::owner`, in the same `[fi][si]`
    /// lockstep as `symbol_ids`/`same_file_by_name`.
    sym_owner: &'a [Vec<Option<&'a str>>],
    /// ADR-0032: per file, every type name that file's own `type_refs`
    /// names in a type position — what `disambiguate_by_owner` checks a
    /// candidate's owner against.
    type_ref_names: &'a [BTreeSet<&'a str>],
    /// ADR-0035: each file's own `FileExtraction::component`, in the
    /// same `[fi]` lockstep as `file_dir`/`dir_scoped`. `None` for a
    /// file under no recognized project root.
    file_component: &'a [Option<&'a str>],
    /// ADR-0035: `pub_by_name`'s exact shape, further split by
    /// component — `(component, name) -> [(fi, si)]`, built only from
    /// files that have a component (a file with none contributes
    /// nothing here; it still shows up in `pub_by_name`). Backs tiers
    /// (b1)/(c1) below.
    pub_by_component: &'a BTreeMap<(&'a str, &'a str), Vec<(usize, usize)>>,
}

impl<'a> CallResolver<'a> {
    fn resolve(&self, caller_file_idx: usize, name: &str) -> Option<(usize, usize, &'static str)> {
        match self.same_file_by_name[caller_file_idx]
            .get(name)
            .map(Vec::as_slice)
        {
            Some([si]) => return Some((caller_file_idx, *si, "same-file")),
            // Two-or-more same-file declarations: the callee is almost
            // certainly one of them (local scope wins in every supported
            // language), and truly ambiguous only if ADR-0032's owner-type
            // filter (below) can't narrow it further — no fall-through to
            // a lower tier that would "resolve" it elsewhere either way
            // (INV-8: missing honestly beats guessing).
            Some(candidates) if candidates.len() >= 2 => {
                let pairs = candidates.iter().map(|&si| (caller_file_idx, si));
                if let Some((fi, si)) = self.disambiguate_by_owner(caller_file_idx, pairs) {
                    return Some((fi, si, "same-file+owner-type-referenced"));
                }
                return None;
            }
            _ => {}
        }

        // Tier (a′), Go only (ADR-0015): same directory, any visibility —
        // files in one Go package see each other's unexported symbols with
        // no import at all. Gated on `dir_scoped` so a non-Go file that
        // happens to share a directory with a Go file (an unusual mixed-repo
        // layout) never picks up this tier. Not extended with the
        // owner-type filter below — ADR-0032 scopes that to the three
        // tiers real field feedback motivated it for.
        if self.dir_scoped[caller_file_idx] {
            let dir = self.file_dir[caller_file_idx];
            if let Some(candidates) = self.same_dir_by_name.get(&(dir, name)) {
                let mut others = candidates.iter().filter(|(fi, _)| *fi != caller_file_idx);
                if let (Some(&(fi, si)), None) = (others.next(), others.next()) {
                    return Some((fi, si, "same-directory"));
                }
            }
        }

        // Tier (b1), ADR-0035: imported, narrowed to the caller's own
        // component — tried before the repo-wide tier (b2) below, not
        // instead of it. Only eligible when the caller *has* a component;
        // a file in the `None` bucket has no scope narrower than the
        // repo, so it goes straight to (b2), unchanged. Falls through
        // (not `return None`) on 0 or ambiguous candidates, exactly like
        // every other tier here — `pub_by_component`'s entry for
        // `(component, declared)` is always a *subset* of `pub_by_name`'s
        // entry for `declared`, so an ambiguous result here is
        // necessarily still ambiguous on the full set (b2 tries the
        // identical `disambiguate_by_owner` call and can only agree or
        // still fail, never contradict) — this tier can only turn an
        // *unresolved* b2 answer into a resolved one, never a different
        // resolved one.
        if let Some(component) = self.file_component[caller_file_idx] {
            if let Some(&declared) = self.alias_to_declared[caller_file_idx].get(name) {
                if let Some(candidates) = self.pub_by_component.get(&(component, declared)) {
                    if let Some(hit) = self.resolve_candidates(
                        caller_file_idx,
                        candidates,
                        "imported",
                        "imported+owner-type-referenced",
                    ) {
                        return Some(hit);
                    }
                }
            }
        }

        // Tier (b2): imported, repo-wide — spec §5.3's original tier
        // (b), unchanged in candidate set and evidence when caller and
        // target share a component (including both in the `None`
        // bucket, the pre-ADR-0035 case). Only the evidence label
        // changes, to `imported-cross-component`, when they don't —
        // `bucket_evidence` is a labeling decision made *after* the
        // identical resolution logic runs, never a filter on which
        // candidates are considered (ADR-0014's "restrict what's
        // listed, not what's computed" principle, applied to resolution
        // labeling here instead of query-time listing).
        //
        // Alias-aware: `name` is what the call site spells; the map's
        // value (if any) is the *declared* name `pub_by_name` is keyed by
        // — the same string for an unaliased import, a different one for
        // `import { foo as bar }`/`from x import foo as bar`/`use Foo as
        // X;`. See `ImportedName`'s doc comment.
        if let Some(&declared) = self.alias_to_declared[caller_file_idx].get(name) {
            if let Some(candidates) = self.pub_by_name.get(declared) {
                if let [(fi, si)] = candidates[..] {
                    let evidence = self.bucket_evidence(
                        caller_file_idx,
                        fi,
                        "imported",
                        "imported-cross-component",
                    );
                    return Some((fi, si, evidence));
                }
                if candidates.len() >= 2 {
                    if let Some((fi, si)) =
                        self.disambiguate_by_owner(caller_file_idx, candidates.iter().copied())
                    {
                        let evidence = self.bucket_evidence(
                            caller_file_idx,
                            fi,
                            "imported+owner-type-referenced",
                            "imported-cross-component+owner-type-referenced",
                        );
                        return Some((fi, si, evidence));
                    }
                }
            }
        }

        // Tier (c1), ADR-0035: exported, narrowed to the caller's own
        // component — same "tried before, not instead of" relationship
        // to (c2) as (b1) has to (b2), and the same subset argument for
        // why it can only turn an unresolved (c2) answer resolved, never
        // a different one.
        if let Some(component) = self.file_component[caller_file_idx] {
            if let Some(candidates) = self.pub_by_component.get(&(component, name)) {
                let others: Vec<(usize, usize)> = candidates
                    .iter()
                    .filter(|(fi, _)| *fi != caller_file_idx)
                    .copied()
                    .collect();
                if let Some(hit) = self.resolve_candidates(
                    caller_file_idx,
                    &others,
                    "same-component",
                    "same-component+owner-type-referenced",
                ) {
                    return Some(hit);
                }
            }
        }

        // Tier (c2): exported, repo-wide — spec §5.3's original tier
        // (c), same "unchanged when same bucket, `cross-component` label
        // when not" relationship (b2) has to (b).
        if let Some(candidates) = self.pub_by_name.get(name) {
            let others: Vec<(usize, usize)> = candidates
                .iter()
                .filter(|(fi, _)| *fi != caller_file_idx)
                .copied()
                .collect();
            match others.as_slice() {
                [(fi, si)] => {
                    let evidence = self.bucket_evidence(
                        caller_file_idx,
                        *fi,
                        "same-package",
                        "cross-component",
                    );
                    return Some((*fi, *si, evidence));
                }
                many if many.len() >= 2 => {
                    if let Some((fi, si)) =
                        self.disambiguate_by_owner(caller_file_idx, many.iter().copied())
                    {
                        let evidence = self.bucket_evidence(
                            caller_file_idx,
                            fi,
                            "same-package+owner-type-referenced",
                            "cross-component+owner-type-referenced",
                        );
                        return Some((fi, si, evidence));
                    }
                }
                _ => {}
            }
        }

        None
    }

    /// Shared shape for tiers (b1)/(c1): a single candidate resolves with
    /// `plain` evidence; ambiguity falls through to
    /// [`Self::disambiguate_by_owner`], resolving with `disambig`
    /// evidence on exactly one survivor or producing nothing (letting the
    /// caller fall through to the next tier) otherwise.
    fn resolve_candidates(
        &self,
        caller_file_idx: usize,
        candidates: &[(usize, usize)],
        plain: &'static str,
        disambig: &'static str,
    ) -> Option<(usize, usize, &'static str)> {
        match candidates {
            [(fi, si)] => Some((*fi, *si, plain)),
            many if many.len() >= 2 => self
                .disambiguate_by_owner(caller_file_idx, many.iter().copied())
                .map(|(fi, si)| (fi, si, disambig)),
            _ => None,
        }
    }

    /// `same` if `caller_file_idx` and `target_file_idx` share a
    /// component (including both being in the `None` bucket — every
    /// repo with no components at all compares `None == None` here,
    /// keeping every evidence string byte-identical to before ADR-0035),
    /// `cross` otherwise. A pure labeling decision over an
    /// already-resolved candidate — never changes *which* candidate a
    /// tier picks, only what the resulting edge's evidence says about
    /// it.
    fn bucket_evidence(
        &self,
        caller_file_idx: usize,
        target_file_idx: usize,
        same: &'static str,
        cross: &'static str,
    ) -> &'static str {
        if self.file_component[caller_file_idx] == self.file_component[target_file_idx] {
            same
        } else {
            cross
        }
    }

    /// ADR-0032: narrows an otherwise-ambiguous candidate list to the
    /// single one whose `owner` the calling file actually names in a
    /// type position (`type_ref_names[caller_file_idx]`) — e.g. a field
    /// typed `IQueryRepository` disambiguates a `CancelQuery` call
    /// between `IQueryRepository.CancelQuery` and
    /// `QueryRepository.CancelQuery`. A candidate with no owner (a free
    /// function, or a type name itself) never survives the filter.
    /// Exactly one survivor resolves; zero or several leave the
    /// ambiguity exactly as before this tier existed — this can only
    /// ever turn a `None` into a `Some`, never change which candidate an
    /// already-unambiguous tier would have picked.
    fn disambiguate_by_owner(
        &self,
        caller_file_idx: usize,
        candidates: impl Iterator<Item = (usize, usize)>,
    ) -> Option<(usize, usize)> {
        let referenced = &self.type_ref_names[caller_file_idx];
        let mut survivors = candidates
            .filter(|&(fi, si)| self.sym_owner[fi][si].is_some_and(|o| referenced.contains(o)));
        match (survivors.next(), survivors.next()) {
            (Some(only), None) => Some(only),
            _ => None,
        }
    }
}

/// ADR-0036: `Some("cross-component")` when a file->file edge's two
/// endpoints belong to different components (including one `Some` and
/// one `None` — a file under no recognized project root importing one
/// that is, or vice versa, is still a real crossing), `None` when they
/// match (including both `None` — a repo with no components anywhere
/// never gets this entry, keeping every existing evidence vector
/// byte-identical for that case). Callers push the marker as an
/// *additional* evidence entry, never a replacement — unlike
/// `CallResolver::bucket_evidence`'s label-substitution for
/// `calls`/`references` (whose evidence is chosen per-tier, one string
/// only), `Imports`' evidence here is always a single already-decided
/// `&'static str` constant with room to spare under
/// `MAX_EVIDENCE_ENTRIES`.
fn cross_component_marker(caller: Option<&str>, target: Option<&str>) -> Option<&'static str> {
    (caller != target).then_some("cross-component")
}

/// ADR-0039: whether a cross-component edge's own crossing is
/// *undeclared* — the caller's component is one of
/// [`crate::components::DEPENDENCY_AWARE_KINDS`] (so it has a real declared-dependency set
/// to check against, not merely an absent one) and the target's
/// component isn't in it. `caller == target` (not a crossing at all)
/// and a caller this index has no dependency data for (either kind,
/// e.g. terraform, or a `None` component) both correctly return
/// `false` — the former isn't a crossing, the latter has nothing to
/// have declared.
fn is_undeclared_dependency(
    component_depends_on: &BTreeMap<&str, BTreeSet<&str>>,
    caller: Option<&str>,
    target: Option<&str>,
) -> bool {
    match (caller, target) {
        (Some(c), Some(t)) if c != t => component_depends_on
            .get(c)
            .is_some_and(|deps| !deps.contains(t)),
        _ => false,
    }
}

/// Resolves a relative import (Rust's `mod <name>;`, `levels_up` always
/// 0; Python's `from <dots><module_path> import ...`, `levels_up` = dot
/// count; TS/JS's `import x from '<dots><module_path>'`, `levels_up` =
/// leading `../` count — ADR-0013) to a sibling/ancestor file. Walks
/// `levels_up` directories up from `declaring_relpath`'s own directory,
/// then looks for `<dir>/<module_path>.<ext>` or
/// `<dir>/<module_path>/<package-marker>` — the common case in each
/// language (`#[path]` overrides, Python namespace packages, and
/// TS/JS's real bundler/tsconfig-driven resolution order are all out of
/// scope, spec §5.3 "deliberately modest"). Which candidate suffixes to
/// try is inferred from `declaring_relpath`'s own extension: a `mod`
/// declaration only ever appears in a `.rs` file, a Python relative
/// import only ever in a `.py` file, and a TS/JS one only ever in a
/// `.ts`/`.tsx`/`.js`/`.jsx`/`.mts`/`.cts`/`.mjs`/`.cjs` file — so the
/// declaring file's extension is sufficient, no need to thread the
/// caller's language through separately. String-joined rather than
/// `std::path::Path`-joined: repo-relative paths are always
/// `/`-separated regardless of host OS (spec §4.1), and `Path::join` on
/// Windows would introduce a `\`-separated key that can't match the
/// `/`-keyed lookup table built from `walk`'s output.
fn resolve_relative_import<'a>(
    declaring_relpath: &str,
    levels_up: u32,
    module_path: &str,
    relpath_to_file_id: &BTreeMap<&str, &'a NodeId>,
) -> Option<&'a NodeId> {
    let mut dir = relpath_dir(declaring_relpath);
    for _ in 0..levels_up {
        dir = dir.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    }

    let ext = declaring_relpath.rsplit_once('.').map(|(_, e)| e);
    let candidates: Vec<String> = match ext {
        Some("py") => vec![
            format!("{module_path}.py"),
            format!("{module_path}/__init__.py"),
        ],
        // TS-first priority order (the user-facing language this
        // extractor was built to serve best), first match wins — real
        // bundler resolution can differ per tsconfig/bundler config,
        // out of scope. Tried regardless of the declaring file's own
        // exact extension within this family: a .ts file importing a
        // .jsx sibling (or vice versa) is common in mixed repos.
        Some("ts" | "tsx" | "js" | "jsx" | "mts" | "cts" | "mjs" | "cjs") => vec![
            format!("{module_path}.ts"),
            format!("{module_path}.tsx"),
            format!("{module_path}.js"),
            format!("{module_path}.jsx"),
            format!("{module_path}/index.ts"),
            format!("{module_path}/index.tsx"),
            format!("{module_path}/index.js"),
            format!("{module_path}/index.jsx"),
        ],
        _ => vec![format!("{module_path}.rs"), format!("{module_path}/mod.rs")],
    };

    candidates
        .iter()
        .find_map(|c| relpath_to_file_id.get(join(dir, c).as_str()))
        .copied()
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// The repo-relative directory containing `relpath` — everything before
/// the last `/`, or `""` for a repo-root file. Shared by
/// `resolve_relative_import`'s own directory-walking and Go's
/// directory-scoped resolution (ADR-0015), both of which need a file's
/// own containing directory as their starting point.
pub(super) fn relpath_dir(relpath: &str) -> &str {
    relpath.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// Resolves a Go import path (`RawImport::PackagePath`, ADR-0015) to a
/// walked directory by longest suffix match against every directory
/// containing a `dir_scoped` (Go) file — no `go.mod` parsing.
/// `github.com/acme/svc/internal/orders` matches a walked
/// `internal/orders` directory because the import path ends with
/// `/internal/orders` (or equals it exactly, for a top-level package
/// directory with no further nesting). Longest match wins so a deeper,
/// more-specific directory is preferred over a coincidentally-matching
/// shorter one. A coincidental suffix match — an external import whose
/// path happens to end with a local directory's own path — is a known,
/// accepted false positive: the cost of not parsing `go.mod`'s module
/// declaration to compute an exact prefix instead.
fn resolve_go_package_path<'a>(
    import_path: &str,
    dir_files: &BTreeMap<&'a str, Vec<usize>>,
) -> Option<&'a str> {
    dir_files
        .keys()
        .filter(|&&dir| {
            !dir.is_empty() && (import_path == dir || import_path.ends_with(&format!("/{dir}")))
        })
        .max_by_key(|dir| dir.len())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{NodeData, SymKind};
    use crate::lang::extractor::{ImportedName, RawCallSite, RawLiteral, RawSymbol, RawTypeRef};

    fn file(relpath: &str) -> FileExtraction {
        FileExtraction {
            file_id: graph::file_id(relpath),
            relpath: relpath.to_string(),
            extract: ExtractOut::default(),
            origin: "lang-rust@1",
            dir_scoped: false,
            ns_separator: "\\",
            qualified_external_is_full_fqn: false,
            // Matches `RustExtractor::relative_import_declares_module()`
            // -> `true` — every other field here already mirrors what a
            // real Rust `FileExtraction` looks like (trait defaults with
            // origin overridden to Rust), so this does too. Non-Rust
            // fixtures built from `file()` that need Rust's `mod`
            // semantics turned off set `declares_module = false`
            // explicitly (see `ts_relative_import_...` below).
            declares_module: true,
            lang: Lang::Rust,
            component: None,
        }
    }

    /// A C#-shaped `FileExtraction` — `.`-separated namespaces
    /// (ADR-0016), matching what `CSharpExtractor` produces.
    fn cs_file(relpath: &str) -> FileExtraction {
        let mut fe = file(relpath);
        fe.origin = "lang-csharp@1";
        fe.ns_separator = ".";
        fe.qualified_external_is_full_fqn = true;
        fe.declares_module = false;
        fe.lang = Lang::CSharp;
        fe
    }

    fn sym(name: &str, kind: SymKind, start: u32, end: u32, is_pub: bool) -> RawSymbol {
        RawSymbol {
            name: name.to_string(),
            qualified_name: name.to_string(),
            sym_kind: kind,
            start_line: start,
            end_line: end,
            signature: format!("fn {name}()"),
            is_pub,
            owner: None,
        }
    }

    /// Like `sym`, but with an owner type set — ADR-0032's
    /// disambiguation tier only ever fires for a symbol declared inside
    /// a type (a method, most commonly).
    fn sym_with_owner(
        name: &str,
        kind: SymKind,
        start: u32,
        end: u32,
        is_pub: bool,
        owner: &str,
    ) -> RawSymbol {
        RawSymbol {
            owner: Some(owner.to_string()),
            ..sym(name, kind, start, end, is_pub)
        }
    }

    fn call(name: &str, line: u32) -> RawCallSite {
        RawCallSite {
            callee_name: name.to_string(),
            line,
        }
    }

    /// Test-only shorthand for an unaliased `ImportedName` — bound and
    /// declared name are the same string, the common case for these
    /// tests; the alias-specific tests build `ImportedName` directly.
    fn name(s: &str) -> ImportedName {
        ImportedName {
            bound_name: s.to_string(),
            declared_name: s.to_string(),
        }
    }

    fn find_symbol<'a>(nodes: &'a [Node], name: &str) -> &'a SymbolNode {
        nodes
            .iter()
            .find_map(|n| match &n.data {
                NodeData::Symbol(s) if s.name == name => Some(s),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no symbol node named {name}"))
    }

    /// ADR-0039: a `crate::components::Component` for tests that need
    /// `depends_on` populated — every prior test in this module passes
    /// `&[]` for `resolve`'s `components` argument, so `component_of`
    /// alone (`FileExtraction.component`) already covers everything
    /// that doesn't need declared-dependency data.
    fn component(
        name: &str,
        path: &str,
        kind: &str,
        depends_on: Vec<&str>,
    ) -> crate::components::Component {
        crate::components::Component {
            name: name.to_string(),
            path: path.to_string(),
            kind: kind.to_string(),
            depends_on: depends_on.into_iter().map(str::to_string).collect(),
        }
    }

    fn calls_edges(edges: &[Edge]) -> Vec<&Edge> {
        edges.iter().filter(|e| e.kind == EdgeKind::Calls).collect()
    }

    fn imports_edges(edges: &[Edge]) -> Vec<&Edge> {
        edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Imports)
            .collect()
    }

    fn references_edges(edges: &[Edge]) -> Vec<&Edge> {
        edges
            .iter()
            .filter(|e| e.kind == EdgeKind::References)
            .collect()
    }

    fn type_ref(name: &str, line: u32) -> RawTypeRef {
        RawTypeRef {
            name: name.to_string(),
            line,
        }
    }

    #[test]
    fn same_file_call_resolves_with_same_file_evidence() {
        let mut f = file("src/orders.rs");
        f.extract.symbols = vec![
            sym("parse_order", SymKind::Function, 1, 5, true),
            sym("validate", SymKind::Function, 7, 9, false),
        ];
        f.extract.call_sites = vec![call("validate", 3)];

        let out = resolve(vec![f], &ContractRules::builtin(), &[]);
        assert!(
            find_symbol(&out.nodes, "parse_order")
                .unresolved_calls
                .is_empty()
        );

        let edges = calls_edges(&out.edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].confidence, Confidence::Inferred);
        assert_eq!(edges[0].evidence, vec!["same-file".to_string()]);
    }

    #[test]
    fn imported_call_resolves_via_use_when_unique() {
        let mut handlers = file("src/handlers.rs");
        handlers.extract.imports = vec![RawImport::Absolute {
            root: "crate".into(),
            imported_names: vec![name("parse_order")],
        }];
        handlers.extract.symbols = vec![sym("handle", SymKind::Function, 1, 5, true)];
        handlers.extract.call_sites = vec![call("parse_order", 3)];

        let mut orders = file("src/orders.rs");
        orders.extract.symbols = vec![sym("parse_order", SymKind::Function, 1, 3, true)];

        let out = resolve(vec![handlers, orders], &ContractRules::builtin(), &[]);
        assert!(
            find_symbol(&out.nodes, "handle")
                .unresolved_calls
                .is_empty()
        );
        assert_eq!(
            calls_edges(&out.edges)[0].evidence,
            vec!["imported".to_string()]
        );
    }

    // ADR-0032: owner-type disambiguation. The reported gap, reproduced
    // directly: an interface method and its implementation share a bare
    // name (`Save`), so tier (c)/(b)/(a) each see ≥2 candidates and
    // would otherwise drop the call — but the caller's own field/
    // parameter type (a `type_refs` entry, ADR-0029's channel) names
    // exactly one of the two owning types, which is enough evidence to
    // resolve it without guessing.

    #[test]
    fn ambiguous_same_package_call_resolves_via_callers_owner_type_reference() {
        let mut store = cs_file("Ports/IQueryJobStore.cs");
        store.extract.symbols = vec![sym_with_owner(
            "Save",
            SymKind::Method,
            3,
            3,
            true,
            "IQueryJobStore",
        )];

        let mut in_memory = cs_file("Services/InMemoryQueryJobStore.cs");
        in_memory.extract.symbols = vec![sym_with_owner(
            "Save",
            SymKind::Method,
            5,
            7,
            true,
            "InMemoryQueryJobStore",
        )];

        let mut service = cs_file("Services/QueryJobService.cs");
        service.extract.symbols = vec![sym("CancelQuery", SymKind::Method, 1, 6, true)];
        service.extract.call_sites = vec![call("Save", 4)];
        // A field/parameter typed `IQueryJobStore` — the same
        // `RawTypeRef` a constructor-injected field produces.
        service.extract.type_refs = vec![type_ref("IQueryJobStore", 2)];

        let out = resolve(
            vec![store, in_memory, service],
            &ContractRules::builtin(),
            &[],
        );

        let edges = calls_edges(&out.edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(
            edges[0].to,
            graph::sym_id("Ports/IQueryJobStore.cs", "method", "Save", 3)
        );
        assert_eq!(
            edges[0].evidence,
            vec!["same-package+owner-type-referenced".to_string()]
        );
        assert!(
            find_symbol(&out.nodes, "CancelQuery")
                .unresolved_calls
                .is_empty()
        );
    }

    #[test]
    fn ambiguous_call_stays_unresolved_when_caller_references_both_owner_types() {
        // The negative case: a registration-style file that names
        // *both* candidate owner types (e.g. `services.AddSingleton
        // <IQueryJobStore, InMemoryQueryJobStore>()`) leaves the filter
        // with two survivors, not one — exactly as ambiguous as before
        // this tier existed. This is also the case ADR-0033's
        // `unresolved_inbound_calls` exists to surface: neither
        // `IQueryJobStore.Save` nor `InMemoryQueryJobStore.Save` gets a
        // `calls` edge from this call site, but each records it as an
        // attempted-and-dropped inbound site.
        let mut store = cs_file("Ports/IQueryJobStore.cs");
        store.extract.symbols = vec![sym_with_owner(
            "Save",
            SymKind::Method,
            3,
            3,
            true,
            "IQueryJobStore",
        )];

        let mut in_memory = cs_file("Services/InMemoryQueryJobStore.cs");
        in_memory.extract.symbols = vec![sym_with_owner(
            "Save",
            SymKind::Method,
            5,
            7,
            true,
            "InMemoryQueryJobStore",
        )];

        let mut registration = cs_file("Program.cs");
        registration.extract.symbols = vec![sym("Register", SymKind::Method, 1, 6, true)];
        registration.extract.call_sites = vec![call("Save", 4)];
        registration.extract.type_refs = vec![
            type_ref("IQueryJobStore", 2),
            type_ref("InMemoryQueryJobStore", 3),
        ];

        let out = resolve(
            vec![store, in_memory, registration],
            &ContractRules::builtin(),
            &[],
        );

        assert!(calls_edges(&out.edges).is_empty());
        let register = find_symbol(&out.nodes, "Register");
        assert_eq!(register.unresolved_calls.len(), 1);
        assert_eq!(register.unresolved_calls[0].name, "Save");

        // Both `Save` symbols must carry the honest inbound signal —
        // ADR-0033's field this scenario motivated.
        let store_save = find_symbol(&out.nodes, "Save");
        assert_eq!(store_save.unresolved_inbound_call_count, 1);
        assert_eq!(store_save.unresolved_inbound_calls[0].file, "Program.cs");
    }

    #[test]
    fn owner_type_disambiguation_never_fires_for_ownerless_candidates() {
        // Two free functions (no owner) sharing a bare name, with an
        // unrelated type ref present — the filter must never engage for
        // a candidate with no owner (a free function or a type
        // declaration itself), so this stays exactly as ambiguous as
        // `ambiguous_same_package_candidates_produce_no_edge`.
        let mut caller = file("src/a.rs");
        caller.extract.symbols = vec![sym("run", SymKind::Function, 1, 5, true)];
        caller.extract.call_sites = vec![call("helper", 3)];
        caller.extract.type_refs = vec![type_ref("Helper", 1)];

        let mut b = file("src/b.rs");
        b.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, true)];
        let mut c = file("src/c.rs");
        c.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, true)];

        let out = resolve(vec![caller, b, c], &ContractRules::builtin(), &[]);
        assert!(calls_edges(&out.edges).is_empty());
        assert_eq!(find_symbol(&out.nodes, "run").unresolved_calls.len(), 1);
    }

    #[test]
    fn ambiguous_same_file_call_resolves_via_owner_type_reference() {
        let mut f = file("src/a.rs");
        f.extract.symbols = vec![
            sym_with_owner("Save", SymKind::Method, 1, 2, true, "Store"),
            sym_with_owner("Save", SymKind::Method, 4, 5, true, "OtherStore"),
            sym("run", SymKind::Function, 7, 10, true),
        ];
        f.extract.call_sites = vec![call("Save", 8)];
        f.extract.type_refs = vec![type_ref("Store", 7)];

        let out = resolve(vec![f], &ContractRules::builtin(), &[]);
        let edges = calls_edges(&out.edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, graph::sym_id("src/a.rs", "method", "Save", 1));
        assert_eq!(
            edges[0].evidence,
            vec!["same-file+owner-type-referenced".to_string()]
        );
    }

    #[test]
    fn ambiguous_imported_call_resolves_via_owner_type_reference() {
        let mut handlers = file("src/handlers.rs");
        handlers.extract.imports = vec![RawImport::Absolute {
            root: "crate".into(),
            imported_names: vec![name("Save")],
        }];
        handlers.extract.symbols = vec![sym("handle", SymKind::Function, 1, 5, true)];
        handlers.extract.call_sites = vec![call("Save", 3)];
        handlers.extract.type_refs = vec![type_ref("Store", 2)];

        let mut store = file("src/store.rs");
        store.extract.symbols = vec![sym_with_owner("Save", SymKind::Method, 1, 3, true, "Store")];
        let mut other = file("src/other_store.rs");
        other.extract.symbols = vec![sym_with_owner(
            "Save",
            SymKind::Method,
            1,
            3,
            true,
            "OtherStore",
        )];

        let out = resolve(vec![handlers, store, other], &ContractRules::builtin(), &[]);
        assert!(
            find_symbol(&out.nodes, "handle")
                .unresolved_calls
                .is_empty()
        );
        let edges = calls_edges(&out.edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(
            edges[0].to,
            graph::sym_id("src/store.rs", "method", "Save", 1)
        );
        assert_eq!(
            edges[0].evidence,
            vec!["imported+owner-type-referenced".to_string()]
        );
    }

    #[test]
    fn aliased_relative_import_resolves_a_call_written_as_the_alias() {
        // `from .orders import parse_order as po` — the fix this test
        // pins: before `ImportedName`/`alias_to_declared`, tier (b)
        // matched the call site's own identifier ("po") against
        // `pub_by_name`, which is keyed by the *declared* name
        // ("parse_order") — a guaranteed miss for any aliased import.
        // Shared machinery with the PHP case below (both go through
        // `resolve_call`'s single `alias_to_declared` lookup) — see
        // `docs/adr/0013-typescript-javascript-resolution-policy-mapping.md`.
        let mut handlers = file("src/handlers.py");
        handlers.origin = "lang-python@1";
        handlers.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "orders".into(),
            imported_names: vec![ImportedName {
                bound_name: "po".into(),
                declared_name: "parse_order".into(),
            }],
        }];
        handlers.extract.symbols = vec![sym("handle", SymKind::Function, 1, 5, true)];
        handlers.extract.call_sites = vec![call("po", 3)];

        let mut orders = file("src/orders.py");
        orders.origin = "lang-python@1";
        orders.extract.symbols = vec![sym("parse_order", SymKind::Function, 1, 3, true)];

        let out = resolve(vec![handlers, orders], &ContractRules::builtin(), &[]);
        assert!(
            find_symbol(&out.nodes, "handle")
                .unresolved_calls
                .is_empty()
        );
        assert_eq!(
            calls_edges(&out.edges)[0].evidence,
            vec!["imported".to_string()]
        );
    }

    #[test]
    fn aliased_qualified_import_resolves_a_call_written_as_the_alias() {
        // PHP's `use App\Orders\parseOrder as po;` — same fix, exercised
        // through `RawImport::Qualified` instead of `Relative`.
        let mut orders = file("src/Orders.php");
        orders.origin = "lang-php@1";
        orders.extract.declared_namespace = Some("App\\Orders".into());
        orders.extract.symbols = vec![sym("parseOrder", SymKind::Function, 1, 3, true)];

        let mut handlers = file("src/Handlers.php");
        handlers.origin = "lang-php@1";
        handlers.extract.imports = vec![RawImport::Qualified {
            fqn: "App\\Orders\\parseOrder".into(),
            bound_name: "po".into(),
        }];
        handlers.extract.symbols = vec![sym("handle", SymKind::Function, 5, 8, true)];
        handlers.extract.call_sites = vec![call("po", 6)];

        let out = resolve(vec![handlers, orders], &ContractRules::builtin(), &[]);
        assert!(
            find_symbol(&out.nodes, "handle")
                .unresolved_calls
                .is_empty()
        );
        assert_eq!(
            calls_edges(&out.edges)[0].evidence,
            vec!["imported".to_string()]
        );
    }

    #[test]
    fn same_package_call_resolves_when_unimported_and_unique() {
        let mut handlers = file("src/handlers.rs");
        handlers.extract.symbols = vec![sym("handle", SymKind::Function, 1, 5, true)];
        handlers.extract.call_sites = vec![call("audit_order", 3)];

        let mut orders = file("src/orders.rs");
        orders.extract.symbols = vec![sym("audit_order", SymKind::Function, 1, 3, true)];

        let out = resolve(vec![handlers, orders], &ContractRules::builtin(), &[]);
        assert!(
            find_symbol(&out.nodes, "handle")
                .unresolved_calls
                .is_empty()
        );
        assert_eq!(
            calls_edges(&out.edges)[0].evidence,
            vec!["same-package".to_string()]
        );
    }

    #[test]
    fn duplicate_same_file_names_are_ambiguous_and_produce_no_edge() {
        // Two classes in one file, each with a `summary` method (a shape
        // PHP/Python/TS all capture) — a call to `summary` in the same
        // file must be recorded unresolved, not silently attributed to
        // whichever declaration was extracted last, and must not fall
        // through to a lower tier either.
        let mut f = file("src/models.php");
        f.origin = "lang-php@1";
        f.extract.symbols = vec![
            sym("handle", SymKind::Function, 1, 4, true),
            sym("summary", SymKind::Method, 6, 8, true),
            sym("summary", SymKind::Method, 10, 12, true),
        ];
        f.extract.call_sites = vec![call("summary", 2)];

        // A same-named pub symbol elsewhere must NOT pick up the call
        // via tier (c) after the same-file ambiguity.
        let mut other = file("src/other.php");
        other.origin = "lang-php@1";
        other.extract.symbols = vec![sym("summary", SymKind::Function, 1, 2, true)];

        let out = resolve(vec![f, other], &ContractRules::builtin(), &[]);
        let handle = find_symbol(&out.nodes, "handle");
        assert_eq!(handle.unresolved_calls.len(), 1);
        assert_eq!(handle.unresolved_calls[0].name, "summary");
        assert!(calls_edges(&out.edges).is_empty());
    }

    #[test]
    fn ts_relative_import_without_names_does_not_make_a_matching_package_root_internal() {
        // `import './orders'` (side-effect) in a .ts file produces the
        // same empty-imported_names Relative shape as Rust's `mod foo;`
        // — it must not enter `known_modules` and suppress the external
        // module node for a bare npm package that happens to be named
        // `orders`.
        let mut app = file("src/app.ts");
        app.origin = "lang-ts@1";
        app.declares_module = false;
        app.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "orders".into(),
            imported_names: vec![],
        }];
        let orders = {
            let mut f = file("src/orders.ts");
            f.origin = "lang-ts@1";
            f.declares_module = false;
            f
        };
        let mut consumer = file("src/consumer.ts");
        consumer.origin = "lang-ts@1";
        consumer.declares_module = false;
        consumer.extract.imports = vec![RawImport::Absolute {
            root: "orders".into(),
            imported_names: vec![],
        }];

        let out = resolve(vec![app, orders, consumer], &ContractRules::builtin(), &[]);
        // The side-effect import still resolves file-to-file, with
        // relative-import (not mod-declaration) evidence...
        let imports = imports_edges(&out.edges);
        assert!(
            imports
                .iter()
                .any(|e| e.evidence == vec!["relative-import".to_string()])
        );
        // ...and the bare `orders` package still gets its external
        // module node + edge.
        assert!(
            out.nodes.iter().any(
                |n| matches!(&n.data, NodeData::Module(m) if m.path == "orders" && m.external)
            )
        );
    }

    #[test]
    fn ambiguous_same_package_candidates_produce_no_edge() {
        let mut caller = file("src/a.rs");
        caller.extract.symbols = vec![sym("run", SymKind::Function, 1, 5, true)];
        caller.extract.call_sites = vec![call("helper", 3)];

        let mut b = file("src/b.rs");
        b.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, true)];
        let mut c = file("src/c.rs");
        c.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, true)];

        let out = resolve(vec![caller, b, c], &ContractRules::builtin(), &[]);
        let run = find_symbol(&out.nodes, "run");
        assert_eq!(run.unresolved_calls.len(), 1);
        assert_eq!(run.unresolved_calls[0].name, "helper");
        assert!(calls_edges(&out.edges).is_empty());

        // ADR-0033: the same ambiguity, from the *inbound* side — both
        // `b::helper` and `c::helper` must record the attempted-and-
        // dropped call site from `src/a.rs:3`, since neither carto nor
        // this test can say which one it meant. This is the honesty
        // signal a `deps --dir in` answer on either symbol needs: an
        // empty `calls` edge list here is not "no callers".
        let helper_b = find_symbol(&out.nodes, "helper");
        assert_eq!(helper_b.unresolved_inbound_call_count, 1);
        assert_eq!(
            helper_b.unresolved_inbound_calls,
            vec![InboundCallSite {
                file: "src/a.rs".to_string(),
                line: 3,
                component: None,
            }]
        );
    }

    // --- ADR-0034/0035: component-scoped resolution -----------------

    #[test]
    fn component_scoping_fixes_the_exact_ambiguity_the_previous_test_leaves_unresolved() {
        // Identical shape to `ambiguous_same_package_candidates_produce_no_edge`
        // above, except `b` (the intended target) and the caller now
        // share a component while `c` (the unrelated same-named symbol)
        // sits in a different one — this is the motivating scenario for
        // the whole feature: two services each defining `Handler`
        // shouldn't make either one unresolvable.
        let mut caller = file("services/orders/a.rs");
        caller.component = Some("orders".to_string());
        caller.extract.symbols = vec![sym("run", SymKind::Function, 1, 5, true)];
        caller.extract.call_sites = vec![call("helper", 3)];

        let mut b = file("services/orders/b.rs");
        b.component = Some("orders".to_string());
        b.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, true)];

        let mut c = file("services/billing/c.rs");
        c.component = Some("billing".to_string());
        c.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, true)];

        let out = resolve(vec![caller, b, c], &ContractRules::builtin(), &[]);

        let edges = calls_edges(&out.edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(
            edges[0].to,
            graph::sym_id("services/orders/b.rs", "function", "helper", 1)
        );
        assert_eq!(edges[0].evidence, vec!["same-component".to_string()]);
        assert!(find_symbol(&out.nodes, "run").unresolved_calls.is_empty());
    }

    #[test]
    fn component_scoped_import_tier_resolves_within_component_over_a_cross_component_namesake() {
        let mut caller = file("services/orders/a.rs");
        caller.component = Some("orders".to_string());
        caller.extract.imports = vec![RawImport::Absolute {
            root: "helpers".to_string(),
            imported_names: vec![name("helper")],
        }];
        caller.extract.call_sites = vec![call("helper", 3)];

        let mut b = file("services/orders/b.rs");
        b.component = Some("orders".to_string());
        b.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, true)];

        let mut c = file("services/billing/c.rs");
        c.component = Some("billing".to_string());
        c.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, true)];

        let out = resolve(vec![caller, b, c], &ContractRules::builtin(), &[]);

        let edges = calls_edges(&out.edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(
            edges[0].to,
            graph::sym_id("services/orders/b.rs", "function", "helper", 1)
        );
        assert_eq!(edges[0].evidence, vec!["imported".to_string()]);
    }

    #[test]
    fn unique_repo_wide_candidate_in_a_different_component_gets_cross_component_evidence() {
        let mut caller = file("services/orders/a.rs");
        caller.component = Some("orders".to_string());
        caller.extract.symbols = vec![sym("run", SymKind::Function, 1, 5, true)];
        caller.extract.call_sites = vec![call("shared_helper", 3)];

        let mut shared = file("libs/shared/s.rs");
        shared.component = Some("shared".to_string());
        shared.extract.symbols = vec![sym("shared_helper", SymKind::Function, 1, 2, true)];

        let out = resolve(vec![caller, shared], &ContractRules::builtin(), &[]);

        let edges = calls_edges(&out.edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(
            edges[0].to,
            graph::sym_id("libs/shared/s.rs", "function", "shared_helper", 1)
        );
        assert_eq!(edges[0].evidence, vec!["cross-component".to_string()]);
    }

    #[test]
    fn ambiguity_within_one_component_still_falls_through_to_repo_wide_and_stays_unresolved() {
        // Both same-named candidates are in the caller's own component
        // (so (c1) is ambiguous too, not merely absent) — falling
        // through to (c2) must not somehow "resolve" this by widening
        // the search; the candidate set only grows, so it stays
        // ambiguous there too. Proves (b1)/(c1)'s fallthrough on
        // ambiguity doesn't paper over a genuine in-component collision.
        let mut caller = file("services/orders/a.rs");
        caller.component = Some("orders".to_string());
        caller.extract.symbols = vec![sym("run", SymKind::Function, 1, 5, true)];
        caller.extract.call_sites = vec![call("helper", 3)];

        let mut b = file("services/orders/b.rs");
        b.component = Some("orders".to_string());
        b.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, true)];

        let mut c = file("services/orders/c.rs");
        c.component = Some("orders".to_string());
        c.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, true)];

        let out = resolve(vec![caller, b, c], &ContractRules::builtin(), &[]);
        assert!(calls_edges(&out.edges).is_empty());
        assert_eq!(find_symbol(&out.nodes, "run").unresolved_calls.len(), 1);
    }

    #[test]
    fn a_repo_with_no_components_anywhere_resolves_exactly_as_before_this_adr() {
        // The real backward-compatibility guarantee: when *no* file has
        // a component, every `bucket_evidence` comparison is `None ==
        // None`, so (b1)/(c1) never fire (nothing narrower than the
        // repo to scope to) and (b2)/(c2) always report the same-bucket
        // evidence string, byte-identical to every pre-ADR-0035 test in
        // this module.
        let mut caller = file("scripts/a.rs");
        caller.extract.symbols = vec![sym("run", SymKind::Function, 1, 5, true)];
        caller.extract.call_sites = vec![call("helper", 3)];

        let mut b = file("scripts/b.rs");
        b.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, true)];

        let out = resolve(vec![caller, b], &ContractRules::builtin(), &[]);
        let edges = calls_edges(&out.edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].evidence, vec!["same-package".to_string()]);
    }

    #[test]
    fn a_caller_with_no_component_calling_into_a_component_is_flagged_cross_component() {
        // Not a backward-compatibility case (the pre-ADR-0035 resolver
        // never had a "component" concept at all) — a deliberate
        // labeling choice, checked explicitly: `None` and `Some(_)` are
        // different buckets, so a root-level script calling into a real
        // component's code is a genuine boundary crossing worth
        // flagging, the same as the reverse direction.
        let mut caller = file("scripts/a.rs");
        caller.extract.symbols = vec![sym("run", SymKind::Function, 1, 5, true)];
        caller.extract.call_sites = vec![call("helper", 3)];

        let mut b = file("services/orders/b.rs");
        b.component = Some("orders".to_string());
        b.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, true)];

        let out = resolve(vec![caller, b], &ContractRules::builtin(), &[]);
        let edges = calls_edges(&out.edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].evidence, vec!["cross-component".to_string()]);
    }

    // --- ADR-0039: component dependency graph -------------------------

    #[test]
    fn undeclared_cross_component_call_gets_the_undeclared_dependency_evidence_entry() {
        let mut caller = file("services/a/handler.go");
        caller.component = Some("a".to_string());
        caller.extract.symbols = vec![sym("Handler", SymKind::Function, 1, 5, true)];
        caller.extract.call_sites = vec![call("B", 3)];

        let mut target = file("services/b/b.go");
        target.component = Some("b".to_string());
        target.extract.symbols = vec![sym("B", SymKind::Function, 1, 2, true)];

        let components = vec![
            component("a", "services/a", "go", vec![]),
            component("b", "services/b", "go", vec![]),
        ];
        let out = resolve(vec![caller, target], &ContractRules::builtin(), &components);
        let edges = calls_edges(&out.edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(
            edges[0].evidence,
            vec![
                "cross-component".to_string(),
                "undeclared-dependency".to_string()
            ]
        );
    }

    #[test]
    fn declared_cross_component_call_does_not_get_the_undeclared_dependency_marker() {
        let mut caller = file("services/a/handler.go");
        caller.component = Some("a".to_string());
        caller.extract.symbols = vec![sym("Handler", SymKind::Function, 1, 5, true)];
        caller.extract.call_sites = vec![call("B", 3)];

        let mut target = file("services/b/b.go");
        target.component = Some("b".to_string());
        target.extract.symbols = vec![sym("B", SymKind::Function, 1, 2, true)];

        let components = vec![
            component("a", "services/a", "go", vec!["b"]),
            component("b", "services/b", "go", vec![]),
        ];
        let out = resolve(vec![caller, target], &ContractRules::builtin(), &components);
        let edges = calls_edges(&out.edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].evidence, vec!["cross-component".to_string()]);
    }

    #[test]
    fn a_caller_kind_with_no_manifest_identity_is_never_flagged_undeclared() {
        // A terraform/custom-kind caller has no declared-dependency
        // concept at all (see `crate::components::
        // DEPENDENCY_AWARE_KINDS`'s own doc comment) -- flagging every
        // one of its crossings "undeclared" would misrepresent "never
        // looked" as "confirmed violation," pure noise for the
        // contracts use case this data model was built for.
        let mut caller = file("infra/main.tf");
        caller.origin = "lang-hcl@1";
        caller.component = Some("infra".to_string());
        caller.extract.symbols = vec![sym("alarm", SymKind::Function, 1, 5, true)];
        caller.extract.call_sites = vec![call("Handler", 3)];

        let mut target = file("services/a/handler.go");
        target.component = Some("a".to_string());
        target.extract.symbols = vec![sym("Handler", SymKind::Function, 1, 2, true)];

        let components = vec![
            component("infra", "infra", "terraform", vec![]),
            component("a", "services/a", "go", vec![]),
        ];
        let out = resolve(vec![caller, target], &ContractRules::builtin(), &components);
        let edges = calls_edges(&out.edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].evidence, vec!["cross-component".to_string()]);
    }

    #[test]
    fn resolve_fqn_prefers_a_declared_dependency_over_arbitrary_first_file_wins() {
        // Two components (neither the caller's own) illegally-from-
        // carto's-view declare the same FQN -- the shape
        // `colliding_fqn_across_components_prefers_the_callers_own_
        // component` covers for a *same-component* candidate. Here
        // neither candidate is the caller's own component, so that
        // tier can't fire; `orders` is listed before `billing` in
        // extraction order, so a plain first-file-wins fallback would
        // pick `orders` -- but `checkout` declares `billing` (not
        // `orders`) as a dependency, so the preference tier must pick
        // `billing` instead.
        let mut orders_shared = file("services/orders/Shared.php");
        orders_shared.origin = "lang-php@1";
        orders_shared.component = Some("orders".to_string());
        orders_shared.extract.declared_namespace = Some("App\\Shared".into());
        orders_shared.extract.symbols = vec![sym("Constants", SymKind::Class, 1, 5, true)];

        let mut billing_shared = file("services/billing/Shared.php");
        billing_shared.origin = "lang-php@1";
        billing_shared.component = Some("billing".to_string());
        billing_shared.extract.declared_namespace = Some("App\\Shared".into());
        billing_shared.extract.symbols = vec![sym("Constants", SymKind::Class, 1, 5, true)];

        let mut checkout_handlers = file("services/checkout/Handlers.php");
        checkout_handlers.origin = "lang-php@1";
        checkout_handlers.component = Some("checkout".to_string());
        checkout_handlers.extract.imports = vec![RawImport::Qualified {
            fqn: "App\\Shared\\Constants".into(),
            bound_name: "Constants".into(),
        }];

        let components = vec![
            component("checkout", "services/checkout", "php", vec!["billing"]),
            component("orders", "services/orders", "php", vec![]),
            component("billing", "services/billing", "php", vec![]),
        ];
        let out = resolve(
            vec![orders_shared, billing_shared, checkout_handlers],
            &ContractRules::builtin(),
            &components,
        );
        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 1);
        assert_eq!(
            imports[0].to,
            graph::file_id("services/billing/Shared.php"),
            "must prefer billing (checkout's declared dependency) over orders (arbitrary first-file-wins)"
        );
    }

    #[test]
    fn component_scoping_can_retarget_an_owner_disambiguated_call_to_the_same_component_candidate()
    {
        // A disclosed, deliberate behavior change (ADR-0035), not a
        // regression: before component scoping, this call's only path
        // to resolution was tier (c)'s owner-type disambiguation over
        // the full repo-wide candidate set, which would have picked
        // `legacy.rs::Save` (the only owner the caller's own type_refs
        // happens to name). With components, (c1) finds a *single*
        // same-component candidate (`store.rs::Save`) before disambig
        // over the wider set is ever consulted — the more precise
        // answer, but a different one than the pre-ADR-0035 resolver
        // would have produced for this exact input.
        let mut caller = file("services/orders/caller.rs");
        caller.component = Some("orders".to_string());
        caller.extract.symbols = vec![sym("run", SymKind::Method, 1, 6, true)];
        caller.extract.call_sites = vec![call("Save", 4)];
        // Only references `LegacyStore` — not `Store` — which is what
        // makes the OLD repo-wide owner-disambig tier pick
        // `legacy.rs::Save` specifically, were it ever reached.
        caller.extract.type_refs = vec![type_ref("LegacyStore", 2)];

        let mut store = file("services/orders/store.rs");
        store.component = Some("orders".to_string());
        store.extract.symbols = vec![sym_with_owner("Save", SymKind::Method, 1, 3, true, "Store")];

        let mut legacy = file("misc/legacy.rs");
        legacy.extract.symbols = vec![sym_with_owner(
            "Save",
            SymKind::Method,
            1,
            3,
            true,
            "LegacyStore",
        )];

        let out = resolve(vec![caller, store, legacy], &ContractRules::builtin(), &[]);
        let edges = calls_edges(&out.edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(
            edges[0].to,
            graph::sym_id("services/orders/store.rs", "method", "Save", 1),
            "component scoping must prefer the same-component candidate \
             over a coincidental cross-component owner-type match"
        );
        assert_eq!(edges[0].evidence, vec!["same-component".to_string()]);
    }

    #[test]
    fn unresolved_inbound_calls_is_capped_but_the_count_stays_uncapped() {
        // One more unresolved site than the cap, each from its own file
        // (so `unresolved_by_name`'s bucket for `helper` gets exactly
        // `UNRESOLVED_INBOUND_SITES_CAP + 1` entries) — the list must
        // truncate at the cap while the count still reports the real
        // total, per ADR-0033.
        let mut b = file("src/b.rs");
        b.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, true)];
        let mut c = file("src/c.rs");
        c.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, true)];

        let total = crate::consts::UNRESOLVED_INBOUND_SITES_CAP as u32 + 1;
        let mut extractions = vec![b, c];
        for i in 0..total {
            let mut caller = file(&format!("src/caller_{i:02}.rs"));
            caller.extract.call_sites = vec![call("helper", 1)];
            extractions.push(caller);
        }

        let out = resolve(extractions, &ContractRules::builtin(), &[]);
        let helper = find_symbol(&out.nodes, "helper");
        assert_eq!(helper.unresolved_inbound_call_count, total);
        assert_eq!(
            helper.unresolved_inbound_calls.len(),
            crate::consts::UNRESOLVED_INBOUND_SITES_CAP
        );
        // Sorted by (file, line) — `src/caller_00.rs` sorts first.
        assert_eq!(helper.unresolved_inbound_calls[0].file, "src/caller_00.rs");
    }

    #[test]
    fn unknown_callee_is_recorded_as_unresolved() {
        let mut f = file("src/a.rs");
        f.extract.symbols = vec![sym("run", SymKind::Function, 1, 5, true)];
        f.extract.call_sites = vec![call("mystery", 3)];

        let out = resolve(vec![f], &ContractRules::builtin(), &[]);
        let run = find_symbol(&out.nodes, "run");
        assert_eq!(
            run.unresolved_calls,
            vec![UnresolvedCall {
                name: "mystery".to_string(),
                line: 3
            }]
        );
    }

    #[test]
    fn mod_declaration_resolves_to_sibling_file() {
        let mut lib = file("src/lib.rs");
        lib.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "orders".into(),
            imported_names: vec![],
        }];
        let orders = file("src/orders.rs");

        let out = resolve(vec![lib, orders], &ContractRules::builtin(), &[]);
        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].confidence, Confidence::Certain);
        assert_eq!(imports[0].evidence, vec!["mod-declaration".to_string()]);
    }

    #[test]
    fn mod_declaration_resolves_to_mod_rs_in_subdirectory() {
        let mut lib = file("src/lib.rs");
        lib.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "orders".into(),
            imported_names: vec![],
        }];
        let orders = file("src/orders/mod.rs");

        let out = resolve(vec![lib, orders], &ContractRules::builtin(), &[]);
        assert_eq!(imports_edges(&out.edges).len(), 1);
    }

    #[test]
    fn unresolvable_mod_declaration_produces_no_edge() {
        let mut lib = file("src/lib.rs");
        lib.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "missing".into(),
            imported_names: vec![],
        }];

        let out = resolve(vec![lib], &ContractRules::builtin(), &[]);
        assert!(imports_edges(&out.edges).is_empty());
    }

    #[test]
    fn python_relative_import_with_named_symbols_resolves_the_module_file() {
        // `from .orders import parse_order, validate` in pkg/handlers.py
        // -- one dot means "the current package" (levels_up: 0), so
        // `orders` resolves as a same-directory sibling of handlers.py.
        let mut python_file = file("pkg/handlers.py");
        python_file.origin = "lang-python@1";
        python_file.declares_module = false;
        python_file.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "orders".into(),
            imported_names: vec![name("parse_order"), name("validate")],
        }];
        let orders = file("pkg/orders.py");

        let out = resolve(vec![python_file, orders], &ContractRules::builtin(), &[]);
        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 1); // one edge to the module, not one per name
        assert_eq!(imports[0].confidence, Confidence::Certain);
        assert_eq!(imports[0].evidence, vec!["relative-import".to_string()]);
    }

    #[test]
    fn python_relative_import_walks_up_one_directory_for_two_dots() {
        // `from ..orders import parse_order` in pkg/sub/handlers.py --
        // two dots means "the parent package" (levels_up: 1): walk up
        // from pkg/sub/ to pkg/, then find orders.py there.
        let mut python_file = file("pkg/sub/handlers.py");
        python_file.origin = "lang-python@1";
        python_file.extract.imports = vec![RawImport::Relative {
            levels_up: 1,
            module_path: "orders".into(),
            imported_names: vec![name("parse_order")],
        }];
        let orders = file("pkg/orders.py");

        let out = resolve(vec![python_file, orders], &ContractRules::builtin(), &[]);
        assert_eq!(imports_edges(&out.edges).len(), 1);
    }

    #[test]
    fn python_bare_relative_submodule_import_resolves_each_name_as_a_submodule() {
        // `from . import a, b` -- each imported name IS itself a
        // submodule to resolve relative to the current directory
        // (no separate module_path to anchor on).
        let mut python_file = file("pkg/__init__.py");
        python_file.origin = "lang-python@1";
        python_file.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "".into(),
            imported_names: vec![name("orders"), name("handlers")],
        }];
        let orders = file("pkg/orders.py");
        let handlers = file("pkg/handlers.py");

        let out = resolve(
            vec![python_file, orders, handlers],
            &ContractRules::builtin(),
            &[],
        );
        assert_eq!(imports_edges(&out.edges).len(), 2);
    }

    #[test]
    fn php_qualified_import_resolves_via_fqn_index() {
        // `use App\Orders\Order;` in Handlers.php, matched against
        // Orders.php's `namespace App\Orders; class Order { .. }` — the
        // repo-wide FQN index (ADR-0012), not a directory walk.
        let mut orders = file("src/Orders.php");
        orders.origin = "lang-php@1";
        orders.extract.declared_namespace = Some("App\\Orders".into());
        orders.extract.symbols = vec![sym("Order", SymKind::Class, 1, 5, true)];

        let mut handlers = file("src/Handlers.php");
        handlers.origin = "lang-php@1";
        handlers.extract.imports = vec![RawImport::Qualified {
            fqn: "App\\Orders\\Order".into(),
            bound_name: "Order".into(),
        }];

        let out = resolve(vec![handlers, orders], &ContractRules::builtin(), &[]);
        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].confidence, Confidence::Certain);
        assert_eq!(imports[0].evidence, vec!["namespace-import".to_string()]);
    }

    /// ADR-0035: two services each illegally-from-carto's-view declaring
    /// `namespace App\Shared; class Constants { .. }` — a real,
    /// non-illegal occurrence across independently-compiled components,
    /// not the single-repo PHP autoload violation the pre-ADR-0035
    /// "first file wins" comment described. A `use App\Shared\Constants;`
    /// from *within* one of those services' own component must resolve
    /// to that service's own declaration, not whichever file
    /// `extractions` happened to list first.
    #[test]
    fn colliding_fqn_across_components_prefers_the_callers_own_component() {
        let mut orders_shared = file("services/orders/Shared.php");
        orders_shared.origin = "lang-php@1";
        orders_shared.component = Some("orders".to_string());
        orders_shared.extract.declared_namespace = Some("App\\Shared".into());
        orders_shared.extract.symbols = vec![sym("Constants", SymKind::Class, 1, 5, true)];

        let mut billing_shared = file("services/billing/Shared.php");
        billing_shared.origin = "lang-php@1";
        billing_shared.component = Some("billing".to_string());
        billing_shared.extract.declared_namespace = Some("App\\Shared".into());
        billing_shared.extract.symbols = vec![sym("Constants", SymKind::Class, 1, 5, true)];

        let mut billing_handlers = file("services/billing/Handlers.php");
        billing_handlers.origin = "lang-php@1";
        billing_handlers.component = Some("billing".to_string());
        billing_handlers.extract.imports = vec![RawImport::Qualified {
            fqn: "App\\Shared\\Constants".into(),
            bound_name: "Constants".into(),
        }];

        let out = resolve(
            vec![orders_shared, billing_shared, billing_handlers],
            &ContractRules::builtin(),
            &[],
        );
        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 1);
        assert_eq!(
            imports[0].to,
            graph::file_id("services/billing/Shared.php"),
            "must resolve to billing's own Shared.php, not orders' same-named one"
        );
    }

    #[test]
    fn php_qualified_import_with_known_root_but_no_fqn_match_produces_no_edge() {
        // `App` is a known namespace root (some file declares `namespace
        // App\Orders;`), but no file declares exactly `App\Missing\Thing`
        // -- internal but unresolvable, honest omission (INV-8), not an
        // external module.
        let mut orders = file("src/Orders.php");
        orders.origin = "lang-php@1";
        orders.extract.declared_namespace = Some("App\\Orders".into());
        orders.extract.symbols = vec![sym("Order", SymKind::Class, 1, 5, true)];

        let mut handlers = file("src/Handlers.php");
        handlers.origin = "lang-php@1";
        handlers.extract.imports = vec![RawImport::Qualified {
            fqn: "App\\Missing\\Thing".into(),
            bound_name: "Thing".into(),
        }];

        let out = resolve(vec![handlers, orders], &ContractRules::builtin(), &[]);
        assert!(imports_edges(&out.edges).is_empty());
        assert!(
            out.nodes
                .iter()
                .all(|n| !matches!(n.data, NodeData::Module(_)))
        );
    }

    #[test]
    fn php_qualified_import_with_unknown_root_produces_external_module_node() {
        let mut handlers = file("src/Handlers.php");
        handlers.origin = "lang-php@1";
        handlers.extract.imports = vec![RawImport::Qualified {
            fqn: "Psr\\Log\\LoggerInterface".into(),
            bound_name: "LoggerInterface".into(),
        }];

        let out = resolve(vec![handlers], &ContractRules::builtin(), &[]);
        let module_nodes: Vec<&ModuleNode> = out
            .nodes
            .iter()
            .filter_map(|n| match &n.data {
                NodeData::Module(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(module_nodes.len(), 1);
        assert_eq!(module_nodes[0].path, "Psr");
        assert!(module_nodes[0].external);

        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].confidence, Confidence::Certain);
        assert_eq!(imports[0].evidence, vec!["external-package".to_string()]);
    }

    #[test]
    fn php_qualified_import_resolves_call_via_tier_b() {
        // `use App\Orders\parseOrder;` (unaliased) makes `parseOrder`
        // resolve via tier (b) exactly like Rust's `use` / Python's
        // `from .. import` — the unaliased baseline, `bound_name ==
        // declared_name`, still works after the `alias_to_declared`
        // rework. The *aliased* case (`use ... as X`), once a
        // documented gap here, is now covered by
        // `aliased_qualified_import_resolves_a_call_written_as_the_alias`.
        let mut orders = file("src/Orders.php");
        orders.origin = "lang-php@1";
        orders.extract.declared_namespace = Some("App\\Orders".into());
        orders.extract.symbols = vec![sym("parseOrder", SymKind::Function, 1, 3, true)];

        let mut handlers = file("src/Handlers.php");
        handlers.origin = "lang-php@1";
        handlers.extract.imports = vec![RawImport::Qualified {
            fqn: "App\\Orders\\parseOrder".into(),
            bound_name: "parseOrder".into(),
        }];
        handlers.extract.symbols = vec![sym("handle", SymKind::Function, 5, 8, true)];
        handlers.extract.call_sites = vec![call("parseOrder", 6)];

        let out = resolve(vec![handlers, orders], &ContractRules::builtin(), &[]);
        assert!(
            find_symbol(&out.nodes, "handle")
                .unresolved_calls
                .is_empty()
        );
        assert_eq!(
            calls_edges(&out.edges)[0].evidence,
            vec!["imported".to_string()]
        );
    }

    #[test]
    fn csharp_namespace_import_fans_out_to_every_file_declaring_the_namespace() {
        // `using Acme.Orders;` (RawImport::NamespaceImport, ADR-0016)
        // matched exactly against declared namespaces — a namespace
        // spans files the way a Go package spans a directory, so a hit
        // fans out one certain edge per declaring file.
        let mut order = cs_file("Orders/Order.cs");
        order.extract.declared_namespace = Some("Acme.Orders".into());
        order.extract.symbols = vec![sym("Order", SymKind::Class, 1, 5, true)];

        let mut parser = cs_file("Orders/OrderParser.cs");
        parser.extract.declared_namespace = Some("Acme.Orders".into());
        parser.extract.symbols = vec![sym("OrderParser", SymKind::Class, 1, 5, true)];

        let mut program = cs_file("Program.cs");
        program.extract.declared_namespace = Some("Acme.App".into());
        program.extract.imports = vec![RawImport::NamespaceImport {
            path: "Acme.Orders".into(),
        }];

        let out = resolve(vec![program, order, parser], &ContractRules::builtin(), &[]);
        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 2);
        for e in &imports {
            assert_eq!(e.confidence, Confidence::Certain);
            assert_eq!(e.evidence, vec!["namespace-import".to_string()]);
        }
    }

    /// ADR-0035: the fan-out narrows to the caller's own component when
    /// at least one same-component file declares the namespace — a
    /// `using Acme.Orders;` inside the "orders" service must not also
    /// fan out into a *different* service's own, unrelated `Acme.Orders`
    /// namespace (the same over-broad-fan-out shape ADR-0029's field
    /// report traced the coarser `imports` edge to in the first place).
    #[test]
    fn csharp_namespace_import_fan_out_narrows_to_the_callers_own_component() {
        let mut orders_order = cs_file("services/orders/Order.cs");
        orders_order.component = Some("orders".to_string());
        orders_order.extract.declared_namespace = Some("Acme.Orders".into());
        orders_order.extract.symbols = vec![sym("Order", SymKind::Class, 1, 5, true)];

        let mut orders_program = cs_file("services/orders/Program.cs");
        orders_program.component = Some("orders".to_string());
        orders_program.extract.declared_namespace = Some("Acme.App".into());
        orders_program.extract.imports = vec![RawImport::NamespaceImport {
            path: "Acme.Orders".into(),
        }];

        // A *different* service that happens to declare a same-named
        // namespace — must not receive a fan-out edge from orders'
        // `Program.cs`.
        let mut billing_order = cs_file("services/billing/Order.cs");
        billing_order.component = Some("billing".to_string());
        billing_order.extract.declared_namespace = Some("Acme.Orders".into());
        billing_order.extract.symbols = vec![sym("BillingOrder", SymKind::Class, 1, 5, true)];

        let out = resolve(
            vec![orders_program, orders_order, billing_order],
            &ContractRules::builtin(),
            &[],
        );
        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 1, "must not fan out into billing too");
        assert_eq!(imports[0].to, graph::file_id("services/orders/Order.cs"));
    }

    #[test]
    fn csharp_namespace_import_with_known_root_but_no_exact_match_produces_no_edge() {
        // `Acme` is a known root (a file declares `Acme.Orders`) but no
        // file declares exactly `Acme.Billing` — internal but
        // unresolvable, honest omission (INV-8), same as PHP outcome 2.
        let mut order = cs_file("Orders/Order.cs");
        order.extract.declared_namespace = Some("Acme.Orders".into());
        order.extract.symbols = vec![sym("Order", SymKind::Class, 1, 5, true)];

        let mut program = cs_file("Program.cs");
        program.extract.imports = vec![RawImport::NamespaceImport {
            path: "Acme.Billing".into(),
        }];

        let out = resolve(vec![program, order], &ContractRules::builtin(), &[]);
        assert!(imports_edges(&out.edges).is_empty());
        assert!(
            out.nodes
                .iter()
                .all(|n| !matches!(n.data, NodeData::Module(_)))
        );
    }

    #[test]
    fn csharp_namespace_import_with_unknown_root_keys_external_module_by_full_string() {
        // `using System.Text.Json;` — external, keyed by the full
        // namespace string, not truncated to `System` (Go-style full
        // identity, ADR-0015/0016): `System.Text.Json` is not the same
        // package as `System`.
        let mut program = cs_file("Program.cs");
        program.extract.imports = vec![
            RawImport::NamespaceImport {
                path: "System.Text.Json".into(),
            },
            RawImport::NamespaceImport {
                path: "System".into(),
            },
        ];

        let out = resolve(vec![program], &ContractRules::builtin(), &[]);
        let module_paths: Vec<&str> = out
            .nodes
            .iter()
            .filter_map(|n| match &n.data {
                NodeData::Module(m) if m.external => Some(m.path.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(module_paths, vec!["System.Text.Json", "System"]);
        assert_eq!(imports_edges(&out.edges).len(), 2);
    }

    #[test]
    fn csharp_namespace_import_never_feeds_tier_b_but_tier_c_resolves_the_call() {
        // A namespace `using` binds no symbol name (like Go's package
        // import, ADR-0015), so the cross-namespace call resolves at
        // tier (c) with "same-package" evidence — which is also why
        // `internal` counts as is_pub (ADR-0016): tier (c) only matches
        // exported symbols.
        let mut order = cs_file("Orders/Order.cs");
        order.extract.declared_namespace = Some("Acme.Orders".into());
        order.extract.symbols = vec![sym("ParseOrder", SymKind::Method, 1, 5, true)];

        let mut program = cs_file("Program.cs");
        program.extract.declared_namespace = Some("Acme.App".into());
        program.extract.imports = vec![RawImport::NamespaceImport {
            path: "Acme.Orders".into(),
        }];
        program.extract.symbols = vec![sym("Main", SymKind::Method, 1, 5, true)];
        program.extract.call_sites = vec![call("ParseOrder", 3)];

        let out = resolve(vec![program, order], &ContractRules::builtin(), &[]);
        assert!(find_symbol(&out.nodes, "Main").unresolved_calls.is_empty());
        assert_eq!(
            calls_edges(&out.edges)[0].evidence,
            vec!["same-package".to_string()]
        );
    }

    #[test]
    fn csharp_alias_using_resolves_import_and_call_with_dot_separator() {
        // `using Parser = Acme.Orders.OrderParser;` maps to
        // RawImport::Qualified with a `.`-separated FQN — the FQN index
        // must compose with the declaring file's own separator
        // (ns_separator, ADR-0016), and tier (b) must find the declared
        // name behind the alias (ADR-0013's machinery, unchanged).
        let mut parser = cs_file("Orders/OrderParser.cs");
        parser.extract.declared_namespace = Some("Acme.Orders".into());
        parser.extract.symbols = vec![sym("OrderParser", SymKind::Class, 1, 5, true)];

        let mut program = cs_file("Program.cs");
        program.extract.declared_namespace = Some("Acme.App".into());
        program.extract.imports = vec![RawImport::Qualified {
            fqn: "Acme.Orders.OrderParser".into(),
            bound_name: "Parser".into(),
        }];
        program.extract.symbols = vec![sym("Main", SymKind::Method, 1, 5, true)];
        program.extract.call_sites = vec![call("Parser", 3)];

        let out = resolve(vec![program, parser], &ContractRules::builtin(), &[]);
        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].evidence, vec!["namespace-import".to_string()]);
        assert!(find_symbol(&out.nodes, "Main").unresolved_calls.is_empty());
        assert_eq!(
            calls_edges(&out.edges)[0].evidence,
            vec!["imported".to_string()]
        );
    }

    #[test]
    fn php_and_csharp_namespaces_coexist_without_cross_matching() {
        // The separator spelling keeps the two languages' keys unequal
        // by construction: PHP's `App\Orders` and C#'s `App.Orders` are
        // different index entries, so a C# `using App.Orders;` never
        // matches the PHP file (and vice versa).
        let mut php = file("src/Orders.php");
        php.origin = "lang-php@1";
        php.extract.declared_namespace = Some("App\\Orders".into());
        php.extract.symbols = vec![sym("Order", SymKind::Class, 1, 5, true)];

        let mut cs = cs_file("Orders/Order.cs");
        cs.extract.declared_namespace = Some("App.Orders".into());
        cs.extract.symbols = vec![sym("CsOrder", SymKind::Class, 1, 5, true)];

        let mut program = cs_file("Program.cs");
        program.extract.imports = vec![RawImport::NamespaceImport {
            path: "App.Orders".into(),
        }];

        let out = resolve(vec![program, php, cs], &ContractRules::builtin(), &[]);
        let imports = imports_edges(&out.edges);
        // Exactly one edge — to the C# file, not the PHP one.
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].to, graph::file_id("Orders/Order.cs"));
    }

    #[test]
    fn namespace_import_does_not_cross_match_a_single_segment_php_namespace() {
        // Regression: a single-segment name (`App`) has no separator at
        // all, so the old bare-string `namespace_to_files` index made a
        // C# `using App;` match a PHP file's `namespace App;` — same
        // string, different language. Origin-keyed, this must instead
        // fall all the way through to outcome 3 (external, unknown
        // root — no C# file declares `App` at all).
        let mut php = file("src/App.php");
        php.origin = "lang-php@1";
        php.extract.declared_namespace = Some("App".into());
        php.extract.symbols = vec![sym("Bootstrap", SymKind::Class, 1, 5, true)];

        let mut program = cs_file("Program.cs");
        program.extract.imports = vec![RawImport::NamespaceImport { path: "App".into() }];

        let out = resolve(vec![program, php], &ContractRules::builtin(), &[]);
        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 1, "must not silently match the PHP file");
        assert_eq!(imports[0].evidence, vec!["external-package".to_string()]);
        assert!(
            out.nodes
                .iter()
                .any(|n| matches!(&n.data, NodeData::Module(m) if m.path == "App" && m.external))
        );
    }

    #[test]
    fn known_namespace_roots_does_not_cross_match_a_php_root_against_csharp() {
        // Regression: `known_namespace_roots` used to be a bare
        // `BTreeSet<&str>` of root segments — a PHP file declaring
        // `namespace System\Legacy;` (root `System`) made a C# `using
        // System.Text;` (root `System` too — root segments have no
        // separator to tell the languages apart) look like a *known
        // internal root with no exact match* (outcome 2: silent no-edge,
        // no node), when it should fall through to outcome 3 (external
        // — no C# file anywhere declares any `System`-rooted namespace).
        let mut php = file("src/Legacy.php");
        php.origin = "lang-php@1";
        php.extract.declared_namespace = Some("System\\Legacy".into());
        php.extract.symbols = vec![sym("Thing", SymKind::Class, 1, 5, true)];

        let mut program = cs_file("Program.cs");
        program.extract.imports = vec![RawImport::NamespaceImport {
            path: "System.Text".into(),
        }];

        let out = resolve(vec![program, php], &ContractRules::builtin(), &[]);
        let imports = imports_edges(&out.edges);
        assert_eq!(
            imports.len(),
            1,
            "PHP's `System` root must not suppress C#'s external edge"
        );
        assert_eq!(imports[0].evidence, vec!["external-package".to_string()]);
        assert!(out.nodes.iter().any(
            |n| matches!(&n.data, NodeData::Module(m) if m.path == "System.Text" && m.external)
        ));
    }

    #[test]
    fn csharp_qualified_external_fallback_keys_by_full_fqn_not_truncated_root() {
        // `using J = System.Text.Json.JsonSerializer;` with no internal
        // match — C#'s `qualified_external_is_full_fqn` (ADR-0016) must
        // key the external Module node by the whole FQN, matching
        // `RawImport::NamespaceImport`'s own full-identity policy, not
        // PHP's root-truncation (`System`, which would incorrectly
        // collapse distinct packages together).
        let mut program = cs_file("Program.cs");
        program.extract.imports = vec![RawImport::Qualified {
            fqn: "System.Text.Json.JsonSerializer".into(),
            bound_name: "J".into(),
        }];

        let out = resolve(vec![program], &ContractRules::builtin(), &[]);
        let module_paths: Vec<&str> = out
            .nodes
            .iter()
            .filter_map(|n| match &n.data {
                NodeData::Module(m) if m.external => Some(m.path.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(module_paths, vec!["System.Text.Json.JsonSerializer"]);
    }

    #[test]
    fn external_use_produces_one_deduped_module_node_with_edges_from_each_file() {
        let mut a = file("src/a.rs");
        a.extract.imports = vec![RawImport::Absolute {
            root: "serde".into(),
            imported_names: vec![name("Deserialize")],
        }];
        let mut b = file("src/b.rs");
        b.extract.imports = vec![RawImport::Absolute {
            root: "serde".into(),
            imported_names: vec![name("Serialize")],
        }];

        let out = resolve(vec![a, b], &ContractRules::builtin(), &[]);
        let module_nodes: Vec<&ModuleNode> = out
            .nodes
            .iter()
            .filter_map(|n| match &n.data {
                NodeData::Module(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(module_nodes.len(), 1);
        assert_eq!(module_nodes[0].path, "serde");
        assert!(module_nodes[0].external);

        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 2);
        assert!(imports.iter().all(|e| e.confidence == Confidence::Certain));
    }

    #[test]
    fn crate_prefixed_use_does_not_produce_module_node_or_edge() {
        let mut f = file("src/handlers.rs");
        f.extract.imports = vec![RawImport::Absolute {
            root: "crate".into(),
            imported_names: vec![name("parse_order")],
        }];

        let out = resolve(vec![f], &ContractRules::builtin(), &[]);
        assert!(
            out.nodes
                .iter()
                .all(|n| !matches!(n.data, NodeData::Module(_)))
        );
        assert!(imports_edges(&out.edges).is_empty());
    }

    #[test]
    fn known_local_module_use_does_not_produce_module_node() {
        let mut lib = file("src/lib.rs");
        lib.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "orders".into(),
            imported_names: vec![],
        }];
        let mut handlers = file("src/handlers.rs");
        handlers.extract.imports = vec![RawImport::Absolute {
            root: "orders".into(),
            imported_names: vec![name("parse_order")],
        }];

        let out = resolve(vec![lib, handlers], &ContractRules::builtin(), &[]);
        assert!(
            out.nodes
                .iter()
                .all(|n| !matches!(n.data, NodeData::Module(_)))
        );
    }

    /// ADR-0035: `known_modules` is a *strict* per-component partition,
    /// not component-first-with-fallback — a `mod orders;` declared in
    /// one crate/component must never make a *different* component's
    /// `use orders::x` classify as internal (a false suppression of a
    /// real external-module edge). Identical shape to
    /// `known_local_module_use_does_not_produce_module_node` above,
    /// except `lib.rs` (which declares `mod orders;`) and `handlers.rs`
    /// (which `use`s `orders`) are now in two different components —
    /// this time a real external `Module` node and edge must appear.
    #[test]
    fn a_mod_declaration_in_one_component_does_not_suppress_another_components_external_import() {
        let mut lib = file("services/orders/lib.rs");
        lib.component = Some("orders".to_string());
        lib.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "orders".into(),
            imported_names: vec![],
        }];
        let mut handlers = file("services/billing/handlers.rs");
        handlers.component = Some("billing".to_string());
        handlers.extract.imports = vec![RawImport::Absolute {
            root: "orders".into(),
            imported_names: vec![name("parse_order")],
        }];

        let out = resolve(vec![lib, handlers], &ContractRules::builtin(), &[]);
        let module = out
            .nodes
            .iter()
            .find_map(|n| match &n.data {
                NodeData::Module(m) => Some(m),
                _ => None,
            })
            .expect("`orders` must be treated as external from billing's perspective");
        assert!(module.external);
        assert_eq!(module.path, "orders");
        let edges = imports_edges(&out.edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].evidence, vec!["external-package".to_string()]);
    }

    /// The other half of the same guarantee: two components each
    /// declaring their own `mod orders;` must not cross-suppress each
    /// other's real internal resolution — a bucket-keyed set, not a
    /// single shared one that a second component's declaration could
    /// somehow interfere with.
    #[test]
    fn each_components_own_mod_declaration_still_suppresses_its_own_external_import() {
        let mut a_lib = file("services/orders/lib.rs");
        a_lib.component = Some("orders".to_string());
        a_lib.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "shared".into(),
            imported_names: vec![],
        }];
        let mut a_handlers = file("services/orders/handlers.rs");
        a_handlers.component = Some("orders".to_string());
        a_handlers.extract.imports = vec![RawImport::Absolute {
            root: "shared".into(),
            imported_names: vec![name("x")],
        }];

        let mut b_lib = file("services/billing/lib.rs");
        b_lib.component = Some("billing".to_string());
        b_lib.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "shared".into(),
            imported_names: vec![],
        }];
        let mut b_handlers = file("services/billing/handlers.rs");
        b_handlers.component = Some("billing".to_string());
        b_handlers.extract.imports = vec![RawImport::Absolute {
            root: "shared".into(),
            imported_names: vec![name("y")],
        }];

        let out = resolve(
            vec![a_lib, a_handlers, b_lib, b_handlers],
            &ContractRules::builtin(),
            &[],
        );
        assert!(
            out.nodes
                .iter()
                .all(|n| !matches!(n.data, NodeData::Module(_))),
            "each component's own `shared` module must stay internal to it"
        );
    }

    /// §ADR-0024: a bare reference (no `use`/`mod` at all) to a root
    /// that's still a genuinely local module — e.g. `orders::helper()`
    /// naming a local `mod orders;` declared elsewhere — must classify
    /// internal exactly like a real `use` to the same root already
    /// does, not fabricate an external `Module` node.
    #[test]
    fn bare_reference_to_a_known_local_module_does_not_produce_module_node() {
        let mut lib = file("src/lib.rs");
        lib.extract.imports = vec![RawImport::Relative {
            levels_up: 0,
            module_path: "orders".into(),
            imported_names: vec![],
        }];
        let mut handlers = file("src/handlers.rs");
        handlers.extract.imports = vec![RawImport::BareReference {
            root: "orders".into(),
        }];

        let out = resolve(vec![lib, handlers], &ContractRules::builtin(), &[]);
        assert!(
            out.nodes
                .iter()
                .all(|n| !matches!(n.data, NodeData::Module(_)))
        );
    }

    /// §ADR-0024: an unknown root produces an external `Module` node
    /// and a `certain`-confidence `imports` edge, same as a real `use`
    /// would — but with a distinct evidence string, so a caller can
    /// tell this heuristic detection apart from a verified declaration.
    #[test]
    fn bare_reference_to_an_unknown_root_produces_external_module_with_distinct_evidence() {
        let mut f = file("src/main.rs");
        f.extract.imports = vec![RawImport::BareReference {
            root: "carto_core".into(),
        }];

        let out = resolve(vec![f], &ContractRules::builtin(), &[]);
        let module = out
            .nodes
            .iter()
            .find_map(|n| match &n.data {
                NodeData::Module(m) if m.path == "carto_core" => Some(m),
                _ => None,
            })
            .expect("expected an external carto_core Module node");
        assert!(module.external);

        let edge = out
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Imports)
            .expect("expected one imports edge");
        assert_eq!(edge.confidence, Confidence::Certain);
        assert_eq!(edge.evidence, vec!["external-package-bare-reference"]);
    }

    #[test]
    fn every_symbol_gets_a_contains_edge_from_its_file() {
        let mut f = file("src/orders.rs");
        f.extract.symbols = vec![sym("parse_order", SymKind::Function, 1, 3, true)];

        let out = resolve(vec![f], &ContractRules::builtin(), &[]);
        let contains: Vec<&Edge> = out
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Contains)
            .collect();
        assert_eq!(contains.len(), 1);
        assert_eq!(contains[0].confidence, Confidence::Certain);
    }

    /// ADR-0023, the outbound half of §1.1's honest-absence signal (T8):
    /// a symbol's own `uncaptured_outbound_calls` counts uncaptured call
    /// sites *inside its own body* — attributed by the same
    /// innermost-containment rule `assign_calls_to_innermost_symbol`
    /// already guarantees for real calls, not a repo-wide by-name count
    /// (that's `uncaptured_inbound_calls`, a different question).
    #[test]
    fn uncaptured_call_sites_are_attributed_to_the_innermost_containing_symbol_only() {
        let mut f = file("src/indexer.rs");
        f.extract.symbols = vec![
            sym("build_and_persist", SymKind::Function, 1, 10, true),
            sym("other_fn", SymKind::Function, 20, 25, true),
        ];
        // Two uncaptured (path-qualified) call sites inside
        // build_and_persist's own range, none inside other_fn's.
        f.extract.uncaptured_call_sites = vec![call("PathGuard::new", 2), call("walk::walk", 3)];

        let out = resolve(vec![f], &ContractRules::builtin(), &[]);
        let build_and_persist = find_symbol(&out.nodes, "build_and_persist");
        let other_fn = find_symbol(&out.nodes, "other_fn");
        assert_eq!(build_and_persist.uncaptured_outbound_calls, 2);
        assert_eq!(other_fn.uncaptured_outbound_calls, 0);
        // Never surfaced as a `calls` edge or `unresolved_calls` entry —
        // a count of unattempted syntax, not a resolution attempt.
        assert!(calls_edges(&out.edges).is_empty());
        assert!(build_and_persist.unresolved_calls.is_empty());
    }

    #[test]
    fn uncaptured_outbound_and_inbound_counts_are_independent() {
        // `helper`'s own body has one uncaptured call (outbound); one
        // uncaptured call site elsewhere spells `helper`'s own bare name
        // (inbound) — the two counts must not bleed into each other.
        let mut caller_file = file("src/caller.rs");
        caller_file.extract.uncaptured_call_sites = vec![call("helper", 5)];
        let mut helper_file = file("src/helper.rs");
        helper_file.extract.symbols = vec![sym("helper", SymKind::Function, 1, 5, true)];
        helper_file.extract.uncaptured_call_sites = vec![call("Type::method", 3)];

        let out = resolve(
            vec![caller_file, helper_file],
            &ContractRules::builtin(),
            &[],
        );
        let helper = find_symbol(&out.nodes, "helper");
        assert_eq!(helper.uncaptured_inbound_calls, 1);
        assert_eq!(helper.uncaptured_outbound_calls, 1);
    }

    /// Test-only shorthand for a Go file (ADR-0015): `dir_scoped: true`,
    /// distinguishing it from every other language's `file()` helper
    /// above, which defaults `dir_scoped` to `false`.
    fn go_file(relpath: &str) -> FileExtraction {
        let mut f = file(relpath);
        f.origin = "lang-go@1";
        f.dir_scoped = true;
        // GoExtractor doesn't override `relative_import_declares_module`
        // (stays `false` — Go never emits `RawImport::Relative` at all),
        // unlike `file()`'s Rust default.
        f.declares_module = false;
        f.lang = Lang::Go;
        f
    }

    #[test]
    fn same_directory_call_resolves_an_unexported_go_callee() {
        // Go's tier (a′): `normalize` is unexported (no import needed)
        // but declared in a sibling file in the same directory.
        let mut order = go_file("internal/orders/order.go");
        order.extract.symbols = vec![sym("ParseOrder", SymKind::Function, 1, 5, true)];
        order.extract.call_sites = vec![call("normalize", 3)];

        let mut helpers = go_file("internal/orders/helpers.go");
        helpers.extract.symbols = vec![sym("normalize", SymKind::Function, 1, 2, false)];

        let out = resolve(vec![order, helpers], &ContractRules::builtin(), &[]);
        assert!(
            find_symbol(&out.nodes, "ParseOrder")
                .unresolved_calls
                .is_empty()
        );
        assert_eq!(
            calls_edges(&out.edges)[0].evidence,
            vec!["same-directory".to_string()]
        );
    }

    #[test]
    fn ambiguous_same_directory_candidates_produce_no_edge() {
        let mut caller = go_file("pkg/a.go");
        caller.extract.symbols = vec![sym("run", SymKind::Function, 1, 5, true)];
        caller.extract.call_sites = vec![call("helper", 3)];

        let mut b = go_file("pkg/b.go");
        b.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, false)];
        let mut c = go_file("pkg/c.go");
        c.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, false)];

        let out = resolve(vec![caller, b, c], &ContractRules::builtin(), &[]);
        let run = find_symbol(&out.nodes, "run");
        assert_eq!(run.unresolved_calls.len(), 1);
        assert_eq!(run.unresolved_calls[0].name, "helper");
        assert!(calls_edges(&out.edges).is_empty());
    }

    #[test]
    fn a_non_go_file_sharing_a_directory_with_a_go_file_never_gets_tier_a_prime() {
        // Guards the `dir_scoped[caller_file_idx]` gate itself: a
        // same-directory unexported symbol must NOT resolve for a caller
        // whose own extractor never opted into directory scoping, even
        // if (unrealistically) it shares a directory with a Go file.
        let mut caller = file("pkg/a.rs"); // NOT go_file — dir_scoped: false
        caller.extract.symbols = vec![sym("run", SymKind::Function, 1, 5, true)];
        caller.extract.call_sites = vec![call("helper", 3)];

        let mut sibling = go_file("pkg/b.go");
        sibling.extract.symbols = vec![sym("helper", SymKind::Function, 1, 2, false)];

        let out = resolve(vec![caller, sibling], &ContractRules::builtin(), &[]);
        let run = find_symbol(&out.nodes, "run");
        assert_eq!(run.unresolved_calls.len(), 1);
        assert_eq!(run.unresolved_calls[0].name, "helper");
    }

    #[test]
    fn go_package_path_import_fans_out_to_every_file_in_the_target_directory() {
        let mut main = go_file("cmd/server/main.go");
        main.extract.imports = vec![RawImport::PackagePath {
            path: "github.com/acme/svc/internal/orders".into(),
        }];

        let order = go_file("internal/orders/order.go");
        let helpers = go_file("internal/orders/helpers.go");

        let out = resolve(vec![main, order, helpers], &ContractRules::builtin(), &[]);
        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 2, "one edge per file in the target dir");
        assert!(imports.iter().all(|e| e.confidence == Confidence::Certain));
        assert!(
            imports
                .iter()
                .all(|e| e.evidence == vec!["package-import".to_string()])
        );
    }

    #[test]
    fn longest_suffix_match_wins_for_go_package_path() {
        // Both "pkg/orders" and "orders" would match a naive single-
        // segment suffix check; the deeper, more specific directory must
        // win over the shallower coincidental one.
        let mut main = go_file("cmd/main.go");
        main.extract.imports = vec![RawImport::PackagePath {
            path: "github.com/acme/svc/pkg/orders".into(),
        }];

        let deep = go_file("pkg/orders/order.go");
        let shallow = go_file("orders/order.go");

        let out = resolve(vec![main, deep, shallow], &ContractRules::builtin(), &[]);
        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].to, graph::file_id("pkg/orders/order.go"));
    }

    #[test]
    fn unmatched_go_package_path_produces_a_full_path_external_module_node() {
        let mut main = go_file("cmd/main.go");
        main.extract.imports = vec![RawImport::PackagePath {
            path: "github.com/lib/pq".into(),
        }];

        let out = resolve(vec![main], &ContractRules::builtin(), &[]);
        let module_nodes: Vec<&ModuleNode> = out
            .nodes
            .iter()
            .filter_map(|n| match &n.data {
                NodeData::Module(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(module_nodes.len(), 1);
        assert_eq!(module_nodes[0].path, "github.com/lib/pq");
        assert!(module_nodes[0].external);

        let imports = imports_edges(&out.edges);
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].confidence, Confidence::Certain);
        assert_eq!(imports[0].evidence, vec!["external-package".to_string()]);
    }

    #[test]
    fn blank_go_import_still_produces_a_dependency_edge() {
        // `import _ "github.com/acme/svc/internal/orders"` — the local
        // binding is discarded by Go itself, but the dependency is real.
        // `PackagePath` carries no bound-name field at all, so a blank
        // import is indistinguishable from (and handled identically to)
        // a normal one — nothing extractor-side to special-case.
        let mut main = go_file("cmd/main.go");
        main.extract.imports = vec![RawImport::PackagePath {
            path: "internal/orders".into(),
        }];
        let order = go_file("internal/orders/order.go");

        let out = resolve(vec![main, order], &ContractRules::builtin(), &[]);
        assert_eq!(imports_edges(&out.edges).len(), 1);
    }

    fn contract_nodes(nodes: &[Node]) -> Vec<&crate::graph::ContractNode> {
        nodes
            .iter()
            .filter_map(|n| match &n.data {
                NodeData::Contract(c) => Some(c),
                _ => None,
            })
            .collect()
    }

    /// ADR-0026's end-to-end shape: an HCL file consuming a metric name
    /// and a C# file producing the exact same one (same value, same
    /// qualifier) must collapse onto one `Contract` node reached by both
    /// a `produces` and a `consumes` edge — this is what makes "orphan"
    /// well-defined (a `Contract` with only one side's edges).
    #[test]
    fn matching_producer_and_consumer_share_one_contract_node() {
        let mut cs = cs_file("services/ingest/EmfMetricsExportService.cs");
        cs.extract.literals = vec![RawLiteral {
            position: "object-init:Name".to_string(),
            value: "QuoteDropsPerSecond".to_string(),
            qualifier: Some("SIDCloud/Ingest".to_string()),
            line: 40,
        }];

        let mut tf = file("infra/terraform/alarms.tf");
        tf.lang = Lang::Hcl;
        tf.extract.literals = vec![RawLiteral {
            position: "aws_cloudwatch_metric_alarm.metric_name".to_string(),
            value: "QuoteDropsPerSecond".to_string(),
            qualifier: Some("SIDCloud/Ingest".to_string()),
            line: 12,
        }];

        let out = resolve(vec![cs, tf], &ContractRules::builtin(), &[]);
        // `resolve` itself may push the same (category, qualifier,
        // value) Contract node's content more than once — real dedup
        // happens where every other producer does too, at
        // `Graph::insert_node` (indexer.rs folds `out.nodes` in the same
        // way); mirror that here rather than asserting on the raw Vec.
        let mut graph = crate::graph::Graph::new();
        for node in out.nodes.clone() {
            graph.insert_node(node);
        }
        let (deduped_nodes, _) = graph.into_sorted_parts();
        let contracts = contract_nodes(&deduped_nodes);
        assert_eq!(contracts.len(), 1, "one shared Contract node, not two");
        assert_eq!(contracts[0].category, "metric_name");

        let produces: Vec<&Edge> = out
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Produces)
            .collect();
        let consumes: Vec<&Edge> = out
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Consumes)
            .collect();
        assert_eq!(produces.len(), 1);
        assert_eq!(consumes.len(), 1);
        assert_eq!(produces[0].to, consumes[0].to, "same Contract node ID");
        assert_eq!(produces[0].confidence, Confidence::Inferred);
        assert_eq!(consumes[0].confidence, Confidence::Certain);
    }

    /// A dead alarm (ADR-0026's acceptance test shape): the HCL side
    /// consumes a name no C# file produces — a real Contract node still
    /// exists (from the consumer alone), with zero `produces` edges.
    #[test]
    fn consumer_with_no_matching_producer_still_creates_a_contract_node() {
        let mut tf = file("infra/terraform/alarms.tf");
        tf.lang = Lang::Hcl;
        tf.extract.literals = vec![RawLiteral {
            position: "aws_cloudwatch_metric_alarm.metric_name".to_string(),
            value: "SequenceGapsTotal".to_string(),
            qualifier: Some("SIDCloud/Ingest".to_string()),
            line: 5,
        }];

        let out = resolve(vec![tf], &ContractRules::builtin(), &[]);
        let contracts = contract_nodes(&out.nodes);
        assert_eq!(contracts.len(), 1);
        assert_eq!(
            out.edges
                .iter()
                .filter(|e| e.kind == EdgeKind::Produces)
                .count(),
            0
        );
        assert_eq!(
            out.edges
                .iter()
                .filter(|e| e.kind == EdgeKind::Consumes)
                .count(),
            1
        );
    }

    /// The same metric *name* emitted under two different namespaces
    /// must not collapse onto one Contract node — the qualifier is part
    /// of the node's identity (ADR-0026), guarding the exact
    /// `CUSTOMER_SUBSCRIPTIONS_TABLE`-vs-`DYNAMO_CUSTOMER_SUBSCRIPTIONS_
    /// TABLE`-style conflation the SID Cloud spec warns about.
    #[test]
    fn same_value_different_qualifier_is_two_distinct_contract_nodes() {
        let mut a = file("infra/a.tf");
        a.lang = Lang::Hcl;
        a.extract.literals = vec![RawLiteral {
            position: "aws_cloudwatch_metric_alarm.metric_name".to_string(),
            value: "Errors".to_string(),
            qualifier: Some("SIDCloud/Ingest".to_string()),
            line: 1,
        }];
        let mut b = file("infra/b.tf");
        b.lang = Lang::Hcl;
        b.extract.literals = vec![RawLiteral {
            position: "aws_cloudwatch_metric_alarm.metric_name".to_string(),
            value: "Errors".to_string(),
            qualifier: Some("SIDCloud/Streamer".to_string()),
            line: 1,
        }];

        let out = resolve(vec![a, b], &ContractRules::builtin(), &[]);
        assert_eq!(contract_nodes(&out.nodes).len(), 2);
    }

    /// A literal at a position no rule classifies (a `.cartoignore`d
    /// vocabulary miss, or simply an ordinary non-contract attribute) is
    /// dropped entirely — no `Contract` node, no edge — same "unmapped
    /// beats guessed" honesty `resolve_call` already applies to an
    /// ambiguous call.
    #[test]
    fn unclassified_position_produces_nothing() {
        let mut tf = file("infra/other.tf");
        tf.lang = Lang::Hcl;
        tf.extract.literals = vec![RawLiteral {
            position: "aws_s3_bucket.bucket".to_string(),
            value: "artifacts".to_string(),
            qualifier: None,
            line: 1,
        }];

        let out = resolve(vec![tf], &ContractRules::builtin(), &[]);
        assert!(contract_nodes(&out.nodes).is_empty());
    }

    /// A C# literal sitting inside a method body must attach its
    /// `produces` edge to the enclosing symbol, not the file — the same
    /// innermost-containment rule `assign_to_innermost_symbol` gives
    /// `calls` edges.
    #[test]
    fn literal_inside_a_method_attaches_to_the_symbol_not_the_file() {
        let mut cs = cs_file("services/ingest/Emitter.cs");
        cs.extract.symbols = vec![sym("Emit", SymKind::Method, 3, 6, true)];
        cs.extract.literals = vec![RawLiteral {
            position: "object-init:Name".to_string(),
            value: "QuoteDropsPerSecond".to_string(),
            qualifier: None,
            line: 4,
        }];

        let out = resolve(vec![cs], &ContractRules::builtin(), &[]);
        let produces = out
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Produces)
            .expect("expected a produces edge");
        let emit_id = graph::sym_id("services/ingest/Emitter.cs", "method", "Emit", 3);
        assert_eq!(produces.from, emit_id);
    }

    // ADR-0029: type refs go through the same tier ladder `calls` does,
    // and become `EdgeKind::References` edges instead — these mirror
    // the shape of the `calls`-resolution tests above, one per tier,
    // plus the self-reference-suppression case that has no `calls`
    // equivalent (a call can't target its own call site the way a
    // field can name its own enclosing type).

    #[test]
    fn same_file_type_ref_resolves_with_same_file_evidence() {
        let mut f = file("src/orders.rs");
        f.extract.symbols = vec![
            sym("Handler", SymKind::Struct, 1, 5, true),
            sym("Order", SymKind::Struct, 7, 9, false),
        ];
        f.extract.type_refs = vec![type_ref("Order", 3)];

        let out = resolve(vec![f], &ContractRules::builtin(), &[]);
        let refs = references_edges(&out.edges);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].confidence, Confidence::Inferred);
        assert_eq!(
            refs[0].evidence,
            vec!["type-reference:same-file".to_string()]
        );
    }

    #[test]
    fn imported_type_ref_resolves_via_use_when_unique() {
        let mut handlers = file("src/handlers.rs");
        handlers.extract.imports = vec![RawImport::Absolute {
            root: "crate".into(),
            imported_names: vec![name("Order")],
        }];
        handlers.extract.symbols = vec![sym("Handler", SymKind::Struct, 1, 5, true)];
        handlers.extract.type_refs = vec![type_ref("Order", 3)];

        let mut orders = file("src/orders.rs");
        orders.extract.symbols = vec![sym("Order", SymKind::Struct, 1, 3, true)];

        let out = resolve(vec![handlers, orders], &ContractRules::builtin(), &[]);
        assert_eq!(
            references_edges(&out.edges)[0].evidence,
            vec!["type-reference:imported".to_string()]
        );
    }

    #[test]
    fn same_package_type_ref_resolves_when_unimported_and_unique() {
        let mut handlers = file("src/handlers.rs");
        handlers.extract.symbols = vec![sym("Handler", SymKind::Struct, 1, 5, true)];
        handlers.extract.type_refs = vec![type_ref("Order", 3)];

        let mut orders = file("src/orders.rs");
        orders.extract.symbols = vec![sym("Order", SymKind::Struct, 1, 3, true)];

        let out = resolve(vec![handlers, orders], &ContractRules::builtin(), &[]);
        assert_eq!(
            references_edges(&out.edges)[0].evidence,
            vec!["type-reference:same-package".to_string()]
        );
    }

    #[test]
    fn ambiguous_type_ref_produces_no_reference_edge() {
        let mut caller = file("src/a.rs");
        caller.extract.symbols = vec![sym("Handler", SymKind::Struct, 1, 5, true)];
        caller.extract.type_refs = vec![type_ref("Order", 3)];

        let mut b = file("src/b.rs");
        b.extract.symbols = vec![sym("Order", SymKind::Struct, 1, 2, true)];
        let mut c = file("src/c.rs");
        c.extract.symbols = vec![sym("Order", SymKind::Struct, 1, 2, true)];

        let out = resolve(vec![caller, b, c], &ContractRules::builtin(), &[]);
        assert!(references_edges(&out.edges).is_empty());
    }

    #[test]
    fn unresolved_type_ref_is_silently_dropped_not_recorded_anywhere() {
        // No `unresolved_calls`-style list for type refs (ADR-0029,
        // user-confirmed): unlike an unresolved call, an unresolved
        // type ref is overwhelmingly stdlib/BCL noise, not a signal.
        let mut f = file("src/a.rs");
        f.extract.symbols = vec![sym("Handler", SymKind::Struct, 1, 5, true)];
        f.extract.type_refs = vec![type_ref("SomeExternalType", 3)];

        let out = resolve(vec![f], &ContractRules::builtin(), &[]);
        assert!(references_edges(&out.edges).is_empty());
        assert!(
            find_symbol(&out.nodes, "Handler")
                .unresolved_calls
                .is_empty()
        );
    }

    /// A symbol referencing its own enclosing type — a field of its
    /// own type, a recursive generic argument — must not produce a
    /// self-edge. No `calls` equivalent exists for this case (a call
    /// site can't be its own callee's definition the way a field can
    /// name its own type), so type refs need their own coverage.
    #[test]
    fn self_referencing_type_ref_produces_no_edge() {
        let mut f = file("src/tree.rs");
        f.extract.symbols = vec![sym("Node", SymKind::Struct, 1, 5, true)];
        // `struct Node { next: Option<Box<Node>> }` -- attributed to
        // Node itself by innermost containment (the struct's own range
        // contains its field declarations).
        f.extract.type_refs = vec![type_ref("Node", 2)];

        let out = resolve(vec![f], &ContractRules::builtin(), &[]);
        assert!(references_edges(&out.edges).is_empty());
    }

    /// The reported field case, reproduced directly against `resolve`:
    /// `QueryJobService` references `IQueryJobStore` (constructor
    /// injection — a type ref, tier (c) `same-package` since neither
    /// file imports a symbol name from the other, only C#'s namespace
    /// `using`), while `S3PresignedUrlProvider` — same namespace,
    /// genuinely `using`s it, but never spells `IQueryJobStore` in a
    /// type position at all — gets no `references` edge despite
    /// sharing the `imports` fan-out. This is the precision `deps
    /// IQueryJobStore --dir in --depth 1 --kinds references` needed
    /// and didn't have before ADR-0029.
    #[test]
    fn csharp_field_reference_is_precise_where_namespace_import_fan_out_was_not() {
        let mut store = cs_file("Ports/IQueryJobStore.cs");
        store.extract.declared_namespace = Some("Acme.Ports".into());
        store.extract.symbols = vec![sym("IQueryJobStore", SymKind::Interface, 1, 1, true)];

        let mut url_provider = cs_file("Ports/IPresignedUrlProvider.cs");
        url_provider.extract.declared_namespace = Some("Acme.Ports".into());
        url_provider.extract.symbols =
            vec![sym("IPresignedUrlProvider", SymKind::Interface, 1, 1, true)];

        let mut service = cs_file("Services/QueryJobService.cs");
        service.extract.declared_namespace = Some("Acme.Services".into());
        service.extract.imports = vec![RawImport::NamespaceImport {
            path: "Acme.Ports".into(),
        }];
        service.extract.symbols = vec![sym("QueryJobService", SymKind::Class, 1, 6, true)];
        service.extract.type_refs = vec![type_ref("IQueryJobStore", 3)];

        let mut s3_provider = cs_file("Services/S3PresignedUrlProvider.cs");
        s3_provider.extract.declared_namespace = Some("Acme.Services".into());
        s3_provider.extract.imports = vec![RawImport::NamespaceImport {
            path: "Acme.Ports".into(),
        }];
        s3_provider.extract.symbols =
            vec![sym("S3PresignedUrlProvider", SymKind::Class, 1, 6, true)];
        s3_provider.extract.type_refs = vec![type_ref("IPresignedUrlProvider", 3)];

        let out = resolve(
            vec![store, url_provider, service, s3_provider],
            &ContractRules::builtin(),
            &[],
        );

        // Both service files import the namespace -- the pre-existing,
        // honestly-broad signal.
        let store_file_imports: Vec<&Edge> = imports_edges(&out.edges)
            .into_iter()
            .filter(|e| e.to == graph::file_id("Ports/IQueryJobStore.cs"))
            .collect();
        assert_eq!(store_file_imports.len(), 2);

        // Only QueryJobService gets a references edge to IQueryJobStore.
        let iqueryjobstore_id =
            graph::sym_id("Ports/IQueryJobStore.cs", "interface", "IQueryJobStore", 1);
        let refs_to_store: Vec<&Edge> = references_edges(&out.edges)
            .into_iter()
            .filter(|e| e.to == iqueryjobstore_id)
            .collect();
        assert_eq!(refs_to_store.len(), 1);
        let queryjobservice_id =
            graph::sym_id("Services/QueryJobService.cs", "class", "QueryJobService", 1);
        assert_eq!(refs_to_store[0].from, queryjobservice_id);
    }

    // ADR-0031: `assign_to_innermost_symbol`'s leftover bucket, resolved
    // at file scope — the retest fix for C# top-level-statement code
    // (ASP.NET Core's minimal-API `Program.cs`, with no `Main` method
    // at all) silently dropping every call/type-ref it contains.

    #[test]
    fn unattached_call_resolves_at_file_scope() {
        // `top_level` has no symbols whatsoever — every line in it,
        // including the call site's, is outside every symbol's range
        // by construction, the same shape a real top-level-statements
        // file has (nothing in `symbols.scm` captures the top level of
        // a file as a container of anything).
        let mut top_level = cs_file("Program.cs");
        top_level.extract.call_sites = vec![call("Register", 3)];

        let mut helper = cs_file("Registrar.cs");
        helper.extract.symbols = vec![sym("Register", SymKind::Method, 1, 3, true)];

        let out = resolve(vec![top_level, helper], &ContractRules::builtin(), &[]);
        let edges = calls_edges(&out.edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, graph::file_id("Program.cs"));
        assert_eq!(edges[0].confidence, Confidence::Inferred);
        assert_eq!(edges[0].evidence, vec!["same-package".to_string()]);
    }

    #[test]
    fn unattached_type_ref_resolves_at_file_scope() {
        let mut top_level = cs_file("Program.cs");
        top_level.extract.type_refs = vec![type_ref("IQueryJobStore", 3)];

        let mut store = cs_file("IQueryJobStore.cs");
        store.extract.symbols = vec![sym("IQueryJobStore", SymKind::Interface, 1, 1, true)];

        let out = resolve(vec![top_level, store], &ContractRules::builtin(), &[]);
        let refs = references_edges(&out.edges);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].from, graph::file_id("Program.cs"));
        assert_eq!(refs[0].confidence, Confidence::Inferred);
        assert_eq!(
            refs[0].evidence,
            vec!["type-reference:same-package".to_string()]
        );
    }

    /// A file mixing top-level-statement code with an ordinary
    /// function: the top-level call must resolve at file scope, the
    /// in-function call must resolve at symbol scope as before, and
    /// neither is double-counted or misattributed to the other.
    #[test]
    fn attached_and_unattached_calls_in_the_same_file_each_resolve_exactly_once() {
        let mut f = file("src/mixed.rs");
        f.extract.symbols = vec![sym("handle", SymKind::Function, 5, 8, true)];
        f.extract.call_sites = vec![
            call("top_level_helper", 1), // outside handle's range
            call("inner_helper", 6),     // inside handle's range
        ];

        let mut helpers = file("src/helpers.rs");
        helpers.extract.symbols = vec![
            sym("top_level_helper", SymKind::Function, 1, 2, true),
            sym("inner_helper", SymKind::Function, 1, 2, true),
        ];

        let out = resolve(vec![f, helpers], &ContractRules::builtin(), &[]);
        let edges = calls_edges(&out.edges);
        assert_eq!(edges.len(), 2);

        let file_sourced = edges
            .iter()
            .filter(|e| e.from == graph::file_id("src/mixed.rs"))
            .count();
        assert_eq!(file_sourced, 1);

        let handle_id = graph::sym_id("src/mixed.rs", "function", "handle", 5);
        let symbol_sourced = edges.iter().filter(|e| e.from == handle_id).count();
        assert_eq!(symbol_sourced, 1);

        assert!(
            find_symbol(&out.nodes, "handle")
                .unresolved_calls
                .is_empty()
        );
    }

    /// User-confirmed scope: an unattached call/type-ref that fails to
    /// resolve stays fully invisible, the same as before ADR-0031 —
    /// there's no `FileNode` equivalent of `unresolved_calls` to record
    /// it in, and adding one is a separate, unscoped decision.
    #[test]
    fn unattached_call_that_fails_to_resolve_produces_no_edge_and_stays_invisible() {
        let mut top_level = file("src/program.rs");
        top_level.extract.call_sites = vec![call("mystery", 1)];

        let out = resolve(vec![top_level], &ContractRules::builtin(), &[]);
        assert!(calls_edges(&out.edges).is_empty());
        // No symbol exists in this file at all, so there's nowhere an
        // `unresolved_calls` entry even could live for this miss.
        assert!(
            out.nodes
                .iter()
                .all(|n| !matches!(n.data, NodeData::Symbol(_)))
        );
    }

    #[test]
    fn unattached_type_ref_that_fails_to_resolve_produces_no_edge() {
        let mut top_level = file("src/program.rs");
        top_level.extract.type_refs = vec![type_ref("SomeExternalType", 1)];

        let out = resolve(vec![top_level], &ContractRules::builtin(), &[]);
        assert!(references_edges(&out.edges).is_empty());
    }
}
