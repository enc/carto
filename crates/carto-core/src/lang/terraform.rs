//! Terraform reference resolution (ADR-0041) — a dedicated pass, not
//! `resolve.rs`'s generic call/type-ref tier ladder. That ladder ends in
//! a repo-wide fallback, which is wrong by construction here: Terraform
//! scope is exactly one directory (a module), so a `var.region` in
//! `modules/a` must never match the `variable "region"` in `modules/b`.
//!
//! **Scope.** A module is the set of `*.tf` files in one directory plus
//! its environment-variant files (`locals.tf.simu`, `locals.tf.prod` —
//! the per-environment pattern ADR-0025 documents, where the real
//! `locals_env.tf` is generated and gitignored).
//!
//! **Which definition a reference means** (file F, reference name N):
//!
//! 1. Candidates: definitions of N in F's directory that live in a plain
//!    `*.tf` file or in a variant file with F's *own* variant suffix.
//!    If any candidate is not an `override.tf`, override candidates are
//!    dropped (Terraform merges overrides over the base declaration).
//! 2. Exactly one → a `references` edge, `inferred`,
//!    `tf-ref:same-module`.
//! 3. None, but other variant files define N → one `inferred` edge per
//!    such definition, evidence `tf-ref:env-variant` + `variant:<suffix>`
//!    (each variant *is* the definition for its own environment; this is
//!    honest, not a guess).
//! 4. Two or more real candidates, or none at all, for `var`/`local`/
//!    `module`/`data`: no edge; the miss is recorded in the enclosing
//!    symbol's `unresolved_calls` ("not found among parsed files" — a
//!    gitignored generated file or a `*.tf.json` is invisible to us).
//! 5. None at all for a resource-shaped `<type>.<name>`: silently
//!    dropped (ADR-0029's policy for type refs) — `each.value`, `count`,
//!    `for`/`dynamic` iterators would otherwise flood the list.
//!
//! Edges are always `inferred` (spec §4.2 allows nothing higher for
//! `references`; source-level scope only approximates what Terraform
//! really loads).

use super::extractor::RawTfRef;
use super::resolve::{FileExtraction, relpath_dir, smallest_containing_symbol};
use crate::graph::{Confidence, Edge, EdgeKind, NodeId, UnresolvedCall};
use std::collections::BTreeMap;

pub(super) struct TerraformResolved {
    pub edges: Vec<Edge>,
    /// `(file index, symbol index)` → misses to append to that symbol's
    /// `unresolved_calls`.
    pub unresolved: BTreeMap<(usize, usize), Vec<UnresolvedCall>>,
}

/// Roots that are never a resource type: Terraform's own namespaces.
const BUILTIN_ROOTS: &[&str] = &["path", "terraform", "count", "each", "self"];

/// What kind of name a reference spells — decides whether a miss is
/// reported (`Strict`) or silently dropped (`ResourceShaped`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Strict,
    ResourceShaped,
}

/// The symbol name (Terraform address) a reference points at, if it is
/// one of the shapes this pass understands.
fn target_name(segments: &[String]) -> Option<(String, Shape)> {
    let root = segments.first()?.as_str();
    if BUILTIN_ROOTS.contains(&root) {
        return None;
    }
    match root {
        "var" | "local" | "module" => {
            let n = segments.get(1)?;
            Some((format!("{root}.{n}"), Shape::Strict))
        }
        "data" => {
            let (t, n) = (segments.get(1)?, segments.get(2)?);
            Some((format!("data.{t}.{n}"), Shape::Strict))
        }
        _ => {
            let n = segments.get(1)?;
            Some((format!("{root}.{n}"), Shape::ResourceShaped))
        }
    }
}

struct Def<'a> {
    fi: usize,
    si: usize,
    variant: Option<&'a str>,
    is_override: bool,
}

pub(super) fn resolve(
    extractions: &[FileExtraction],
    symbol_ids: &[Vec<NodeId>],
) -> TerraformResolved {
    // (directory, address) -> every definition, in file order.
    let mut defs: BTreeMap<(&str, &str), Vec<Def<'_>>> = BTreeMap::new();
    for (fi, fe) in extractions.iter().enumerate() {
        let Some(tf) = &fe.extract.terraform else {
            continue;
        };
        for (si, sym) in fe.extract.symbols.iter().enumerate() {
            defs.entry((relpath_dir(&fe.relpath), sym.name.as_str()))
                .or_default()
                .push(Def {
                    fi,
                    si,
                    variant: tf.variant.as_deref(),
                    is_override: tf.is_override,
                });
        }
    }

    let mut out = TerraformResolved {
        edges: Vec::new(),
        unresolved: BTreeMap::new(),
    };

    for (fi, fe) in extractions.iter().enumerate() {
        let Some(tf) = &fe.extract.terraform else {
            continue;
        };
        let dir = relpath_dir(&fe.relpath);
        for r in &tf.refs {
            resolve_ref(
                &mut out,
                extractions,
                symbol_ids,
                &defs,
                fi,
                dir,
                tf.variant.as_deref(),
                r,
            );
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn resolve_ref(
    out: &mut TerraformResolved,
    extractions: &[FileExtraction],
    symbol_ids: &[Vec<NodeId>],
    defs: &BTreeMap<(&str, &str), Vec<Def<'_>>>,
    fi: usize,
    dir: &str,
    variant: Option<&str>,
    r: &RawTfRef,
) {
    let Some((name, shape)) = target_name(&r.segments) else {
        return;
    };
    let fe = &extractions[fi];
    let enclosing = smallest_containing_symbol(&fe.extract.symbols, r.line);
    let from_id = match enclosing {
        Some(si) => symbol_ids[fi][si].clone(),
        None => fe.file_id.clone(),
    };

    let all: &[Def<'_>] = defs
        .get(&(dir, name.as_str()))
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    let mut live: Vec<&Def<'_>> = all
        .iter()
        .filter(|d| d.variant.is_none() || d.variant == variant)
        .collect();
    if live.iter().any(|d| !d.is_override) {
        live.retain(|d| !d.is_override);
    }

    let mut push_edge = |d: &Def<'_>, evidence: Vec<String>| {
        let to_id = symbol_ids[d.fi][d.si].clone();
        if to_id == from_id {
            return; // a declaration naming itself is not a dependency
        }
        let mut it = evidence.into_iter();
        let mut edge = Edge::new(
            EdgeKind::References,
            from_id.clone(),
            to_id,
            Confidence::Inferred,
            it.next().expect("at least one evidence entry"),
        );
        edge.evidence.extend(it);
        out.edges.push(edge);
    };

    match live.as_slice() {
        [one] => push_edge(one, vec!["tf-ref:same-module".to_string()]),
        [] if !all.is_empty() => {
            // Defined only in other environment variants.
            for d in all {
                let suffix = d.variant.unwrap_or_default();
                push_edge(
                    d,
                    vec![
                        "tf-ref:env-variant".to_string(),
                        format!("variant:{suffix}"),
                    ],
                );
            }
        }
        [] if shape == Shape::ResourceShaped => {}
        // Genuinely undefined, or a real duplicate: no edge, and say so
        // on the enclosing symbol (no enclosing symbol: nowhere to say it).
        _ => {
            if let Some(si) = enclosing {
                out.unresolved
                    .entry((fi, si))
                    .or_default()
                    .push(UnresolvedCall { name, line: r.line });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::extractor::LangExtractor;
    use super::super::hcl::HclExtractor;
    use super::super::resolve::resolve as resolve_all;
    use super::*;
    use crate::contracts::ContractRules;
    use crate::graph::{self, Node, NodeData};
    use crate::lang::Lang;

    fn tf_file(relpath: &str, src: &str) -> FileExtraction {
        FileExtraction {
            file_id: graph::file_id(relpath),
            relpath: relpath.to_string(),
            extract: HclExtractor.extract(src.as_bytes(), relpath),
            origin: "lang-hcl@1",
            dir_scoped: false,
            ns_separator: "\\",
            qualified_external_is_full_fqn: false,
            declares_module: false,
            lang: Lang::Hcl,
            component: None,
        }
    }

    /// `(from, to, evidence)` for every `references` edge, by symbol
    /// name (`<file>` for a file-scope source).
    fn refs(nodes: &[Node], edges: &[Edge]) -> Vec<(String, String, Vec<String>)> {
        let names: BTreeMap<&NodeId, String> = nodes
            .iter()
            .filter_map(|n| match &n.data {
                NodeData::Symbol(s) => Some((&n.id, format!("{}@{}", s.name, s.start_line))),
                _ => None,
            })
            .collect();
        let mut v: Vec<_> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::References)
            .map(|e| {
                (
                    names.get(&e.from).cloned().unwrap_or("<file>".into()),
                    names.get(&e.to).cloned().unwrap_or("<file>".into()),
                    e.evidence.clone(),
                )
            })
            .collect();
        v.sort();
        v
    }

    fn unresolved(nodes: &[Node], name: &str) -> Vec<String> {
        nodes
            .iter()
            .find_map(|n| match &n.data {
                NodeData::Symbol(s) if s.name == name => {
                    Some(s.unresolved_calls.iter().map(|u| u.name.clone()).collect())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no symbol {name}"))
    }

    fn run(files: Vec<FileExtraction>) -> (Vec<Node>, Vec<Edge>) {
        let out = resolve_all(files, &ContractRules::builtin(), &[]);
        (out.nodes, out.edges)
    }

    #[test]
    fn same_module_reference_resolves_inferred() {
        let (nodes, edges) = run(vec![
            tf_file("m/variables.tf", "variable \"region\" {}\n"),
            tf_file(
                "m/main.tf",
                "resource \"aws_s3_bucket\" \"b\" {\n  bucket = var.region\n}\n",
            ),
        ]);
        assert_eq!(
            refs(&nodes, &edges),
            vec![(
                "aws_s3_bucket.b@1".into(),
                "var.region@1".into(),
                vec!["tf-ref:same-module".into()]
            )]
        );
        let e = edges
            .iter()
            .find(|e| e.kind == EdgeKind::References)
            .unwrap();
        assert_eq!(e.confidence, Confidence::Inferred);
    }

    #[test]
    fn a_variable_in_another_directory_is_never_matched() {
        let (nodes, edges) = run(vec![
            tf_file("a/variables.tf", "variable \"region\" {}\n"),
            tf_file(
                "b/main.tf",
                "resource \"aws_s3_bucket\" \"x\" {\n  bucket = var.region\n}\n",
            ),
        ]);
        assert!(refs(&nodes, &edges).is_empty());
        assert_eq!(unresolved(&nodes, "aws_s3_bucket.x"), vec!["var.region"]);
    }

    #[test]
    fn same_named_variables_in_two_modules_each_resolve_locally() {
        let (nodes, edges) = run(vec![
            tf_file("a/v.tf", "variable \"region\" {}\n"),
            tf_file("a/m.tf", "output \"o\" {\n  value = var.region\n}\n"),
            tf_file("b/v.tf", "variable \"region\" {}\n"),
            tf_file("b/m.tf", "output \"o\" {\n  value = var.region\n}\n"),
        ]);
        let r = refs(&nodes, &edges);
        assert_eq!(r.len(), 2);
        let ids: Vec<_> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::References)
            .map(|e| e.to.clone())
            .collect();
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn env_variant_definitions_fan_out_with_variant_evidence() {
        let (nodes, edges) = run(vec![
            tf_file("env/locals.tf.simu", "locals {\n  prefix = \"a\"\n}\n"),
            tf_file("env/locals.tf.prod", "locals {\n  prefix = \"b\"\n}\n"),
            tf_file("env/main.tf", "output \"o\" {\n  value = local.prefix\n}\n"),
        ]);
        let r = refs(&nodes, &edges);
        assert_eq!(r.len(), 2);
        let ev: Vec<_> = r.iter().map(|x| x.2.clone()).collect();
        assert!(ev.contains(&vec!["tf-ref:env-variant".into(), "variant:simu".into()]));
        assert!(ev.contains(&vec!["tf-ref:env-variant".into(), "variant:prod".into()]));
        assert!(unresolved(&nodes, "output.o").is_empty());
    }

    #[test]
    fn a_variant_file_prefers_its_own_variant_and_plain_files() {
        let (nodes, edges) = run(vec![
            tf_file("env/locals.tf.simu", "locals {\n  prefix = \"a\"\n}\n"),
            tf_file("env/locals.tf.prod", "locals {\n  prefix = \"b\"\n}\n"),
            tf_file(
                "env/extra.tf.simu",
                "output \"o\" {\n  value = local.prefix\n}\n",
            ),
        ]);
        let r = refs(&nodes, &edges);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].2, vec!["tf-ref:same-module".to_string()]);
    }

    #[test]
    fn override_file_yields_to_the_base_declaration() {
        let (nodes, edges) = run(vec![
            tf_file("m/v.tf", "variable \"x\" {}\n"),
            tf_file("m/override.tf", "variable \"x\" {}\n"),
            tf_file("m/main.tf", "output \"o\" {\n  value = var.x\n}\n"),
        ]);
        let r = refs(&nodes, &edges);
        assert_eq!(r.len(), 1);
        assert!(unresolved(&nodes, "output.o").is_empty());
    }

    #[test]
    fn real_duplicate_produces_no_edge_and_is_reported() {
        let (nodes, edges) = run(vec![
            tf_file("m/a.tf", "variable \"x\" {}\n"),
            tf_file("m/b.tf", "variable \"x\" {}\n"),
            tf_file("m/main.tf", "output \"o\" {\n  value = var.x\n}\n"),
        ]);
        assert!(refs(&nodes, &edges).is_empty());
        assert_eq!(unresolved(&nodes, "output.o"), vec!["var.x"]);
    }

    #[test]
    fn undefined_local_is_reported_but_unknown_resource_shape_is_not() {
        let (nodes, edges) = run(vec![tf_file(
            "m/main.tf",
            "output \"o\" {\n  value = \"${local.missing}${each.value}${s.id}\"\n}\n",
        )]);
        assert!(refs(&nodes, &edges).is_empty());
        assert_eq!(unresolved(&nodes, "output.o"), vec!["local.missing"]);
    }

    #[test]
    fn resources_and_data_and_modules_resolve() {
        let (nodes, edges) = run(vec![tf_file(
            "m/main.tf",
            "resource \"aws_vpc\" \"main\" {}\n\
             data \"aws_caller_identity\" \"me\" {}\n\
             module \"child\" {}\n\
             output \"o\" {\n  value = [aws_vpc.main.id, data.aws_caller_identity.me.arn, module.child.x]\n}\n",
        )]);
        let r = refs(&nodes, &edges);
        let targets: Vec<_> = r.iter().map(|x| x.1.clone()).collect();
        assert_eq!(targets.len(), 3);
        assert!(targets.iter().any(|t| t.starts_with("aws_vpc.main")));
        assert!(
            targets
                .iter()
                .any(|t| t.starts_with("data.aws_caller_identity.me"))
        );
        assert!(targets.iter().any(|t| t.starts_with("module.child")));
    }

    #[test]
    fn a_local_naming_itself_is_not_an_edge() {
        let (nodes, edges) = run(vec![tf_file("m/main.tf", "locals {\n  a = local.a\n}\n")]);
        assert!(refs(&nodes, &edges).is_empty());
    }

    /// The reason HCL symbols are never `is_pub`: a Terraform
    /// `variable "timeout"` must not make a unique Python `timeout()`
    /// call ambiguous.
    #[test]
    fn terraform_symbols_do_not_disturb_other_languages_call_resolution() {
        use crate::graph::SymKind;
        use crate::lang::extractor::{RawCallSite, RawSymbol};
        let mut py_lib = tf_file("lib.py", "");
        py_lib.origin = "lang-python@1";
        py_lib.lang = Lang::Python;
        py_lib.extract.symbols.push(RawSymbol {
            name: "timeout".into(),
            qualified_name: "timeout".into(),
            sym_kind: SymKind::Function,
            start_line: 1,
            end_line: 2,
            signature: "def timeout()".into(),
            is_pub: true,
            owner: None,
        });
        let mut py_app = tf_file("app.py", "");
        py_app.origin = "lang-python@1";
        py_app.lang = Lang::Python;
        py_app.extract.symbols.push(RawSymbol {
            name: "run".into(),
            qualified_name: "run".into(),
            sym_kind: SymKind::Function,
            start_line: 1,
            end_line: 3,
            signature: "def run()".into(),
            is_pub: true,
            owner: None,
        });
        py_app.extract.call_sites.push(RawCallSite {
            callee_name: "timeout".into(),
            line: 2,
        });
        let (nodes, edges) = run(vec![
            py_lib,
            py_app,
            tf_file("infra/v.tf", "variable \"timeout\" {}\n"),
        ]);
        assert!(
            edges.iter().any(|e| e.kind == EdgeKind::Calls),
            "the Python call must still resolve: {nodes:?}"
        );
    }
}
