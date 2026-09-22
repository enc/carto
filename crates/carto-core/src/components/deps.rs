//! Component dependency graph (ADR-0039): resolves each [`Component`]'s
//! own manifest-declared dependencies to *other components in this
//! walked tree*, populating [`Component::depends_on`]. Runs once, at
//! the end of [`super::ComponentSet::discover`], after every
//! component's `path`/`kind` is final — dependency resolution needs
//! the whole set to match a declaration against.
//!
//! Per-`kind` parsing, dependency-free (no `toml`/XML crate — spec §13 and
//! `deny.toml`'s `multiple-versions = "deny"`; every format here is either
//! JSON, already a `serde_json` dependency, or simple enough for a line
//! scan, the same discipline `is_aggregator` (ADR-0037) already uses):
//!
//! - **go**: `go.mod`'s `require` block/lines, matched against every
//!   other go component's own `module` declaration; a local `replace`
//!   directive (`old/path => ../relative/dir`) resolved by path instead.
//! - **node**: `package.json`'s `dependencies`/`devDependencies`, via
//!   a `file:`/`workspace:` path reference or a name match against
//!   another component's own declared `"name"`.
//! - **rust**: `Cargo.toml`'s `[dependencies]`/`[dev-dependencies]`/
//!   `[build-dependencies]` tables (and their `[section.name]`
//!   sub-table form), any entry naming a `path = "…"`.
//! - **dotnet**: every `*.csproj`/`*.fsproj` directly in the
//!   component's own directory, each `<ProjectReference Include="…">`.
//! - **php**: `composer.json`'s `repositories` entries of
//!   `"type": "path"`, plus `require` entries matching another
//!   component's own declared `"name"`.
//!
//! `terraform`/`custom`-kind components have no manifest identity
//! concept carto knows how to read here, so `depends_on` stays empty
//! for them — a real, if narrower, scope limit (see
//! `docs/adr/0039-component-dependency-graph.md`), not a claim that
//! such a component provably has no dependencies.
//!
//! A read/parse failure for any one manifest is treated the same
//! best-effort way `is_aggregator` treats one (ADR-0034/0037): that
//! component's `depends_on` simply stays empty rather than failing the
//! whole index.

use super::Component;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub(super) fn resolve(repo_root: &Path, components: &mut [Component]) {
    // Snapshot path -> name before any mutation below — every kind's
    // path-based resolution (Go `replace`, Rust `path =`, dotnet
    // `ProjectReference`, PHP path `repositories`) needs the *whole*
    // set, including components of other kinds, to resolve against.
    let components_by_path: BTreeMap<String, String> = components
        .iter()
        .map(|c| (c.path.clone(), c.name.clone()))
        .collect();

    resolve_go(repo_root, components, &components_by_path);
    resolve_node(repo_root, components, &components_by_path);
    resolve_rust(repo_root, components, &components_by_path);
    resolve_dotnet(repo_root, components, &components_by_path);
    resolve_php(repo_root, components, &components_by_path);
}

/// `base` (a component's own repo-relative `path`) with `rel`'s `.`/`..`
/// segments applied — pure path-segment arithmetic, no filesystem
/// access, since the result is only ever looked up in an
/// already-known `components_by_path` map, never read from disk
/// itself.
fn normalize_relative(base: &str, rel: &str) -> String {
    let mut segments: Vec<&str> = if rel.starts_with('/') {
        Vec::new()
    } else {
        base.split('/').filter(|s| !s.is_empty()).collect()
    };
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

fn add_dep(deps: &mut BTreeSet<String>, self_name: &str, target_name: &str) {
    if target_name != self_name {
        deps.insert(target_name.to_string());
    }
}

// --- Go ----------------------------------------------------------------

struct GoModInfo {
    module: Option<String>,
    requires: Vec<String>,
    /// Only `replace X => <local path>` directives — a replacement
    /// naming another *module* (not a local path) carries no
    /// component-dependency signal this function can use.
    local_replaces: Vec<(String, String)>,
}

fn parse_go_mod(content: &str) -> GoModInfo {
    let mut module = None;
    let mut requires = Vec::new();
    let mut local_replaces = Vec::new();
    let mut in_require_block = false;
    let mut in_replace_block = false;

    for raw_line in content.lines() {
        let line = raw_line.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("module ") {
            module = Some(rest.trim().to_string());
            continue;
        }
        if line == "require (" {
            in_require_block = true;
            continue;
        }
        if line == "replace (" {
            in_replace_block = true;
            continue;
        }
        if line == ")" {
            in_require_block = false;
            in_replace_block = false;
            continue;
        }
        if in_require_block {
            if let Some(path) = line.split_whitespace().next() {
                requires.push(path.to_string());
            }
            continue;
        }
        if in_replace_block {
            if let Some(pair) = parse_go_replace(line) {
                local_replaces.push(pair);
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("require ") {
            if let Some(path) = rest.split_whitespace().next() {
                requires.push(path.to_string());
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("replace ") {
            if let Some(pair) = parse_go_replace(rest) {
                local_replaces.push(pair);
            }
        }
    }

    GoModInfo {
        module,
        requires,
        local_replaces,
    }
}

fn parse_go_replace(line: &str) -> Option<(String, String)> {
    let (lhs, rhs) = line.split_once("=>")?;
    let from = lhs.split_whitespace().next()?.to_string();
    let to = rhs.trim();
    if to.starts_with("./") || to.starts_with("../") {
        Some((from, to.to_string()))
    } else {
        None
    }
}

fn resolve_go(repo_root: &Path, components: &mut [Component], by_path: &BTreeMap<String, String>) {
    let mut module_to_name: BTreeMap<String, String> = BTreeMap::new();
    let mut info_by_name: BTreeMap<String, GoModInfo> = BTreeMap::new();
    for c in components.iter().filter(|c| c.kind == "go") {
        let Ok(content) = std::fs::read_to_string(repo_root.join(&c.path).join("go.mod")) else {
            continue;
        };
        let info = parse_go_mod(&content);
        if let Some(m) = &info.module {
            module_to_name.insert(m.clone(), c.name.clone());
        }
        info_by_name.insert(c.name.clone(), info);
    }
    for c in components.iter_mut().filter(|c| c.kind == "go") {
        let Some(info) = info_by_name.get(&c.name) else {
            continue;
        };
        let mut deps = BTreeSet::new();
        for req in &info.requires {
            if let Some(name) = module_to_name.get(req) {
                add_dep(&mut deps, &c.name, name);
            }
        }
        for (_, rel) in &info.local_replaces {
            let resolved = normalize_relative(&c.path, rel);
            if let Some(name) = by_path.get(&resolved) {
                add_dep(&mut deps, &c.name, name);
            }
        }
        c.depends_on = deps.into_iter().collect();
    }
}

// --- Node ----------------------------------------------------------------

fn resolve_node(
    repo_root: &Path,
    components: &mut [Component],
    by_path: &BTreeMap<String, String>,
) {
    let mut name_to_component: BTreeMap<String, String> = BTreeMap::new();
    let mut json_by_name: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for c in components.iter().filter(|c| c.kind == "node") {
        let Ok(content) = std::fs::read_to_string(repo_root.join(&c.path).join("package.json"))
        else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) else {
            continue;
        };
        if let Some(pkg_name) = v.get("name").and_then(|n| n.as_str()) {
            name_to_component.insert(pkg_name.to_string(), c.name.clone());
        }
        json_by_name.insert(c.name.clone(), v);
    }
    for c in components.iter_mut().filter(|c| c.kind == "node") {
        let Some(v) = json_by_name.get(&c.name) else {
            continue;
        };
        let mut deps = BTreeSet::new();
        for field in ["dependencies", "devDependencies"] {
            let Some(obj) = v.get(field).and_then(|d| d.as_object()) else {
                continue;
            };
            for (pkg_name, spec) in obj {
                let spec_str = spec.as_str().unwrap_or("");
                let path_ref = spec_str
                    .strip_prefix("file:")
                    .or_else(|| spec_str.strip_prefix("workspace:"))
                    .filter(|rel| rel.starts_with('.') || rel.starts_with('/'));
                if let Some(rel) = path_ref {
                    let resolved = normalize_relative(&c.path, rel);
                    if let Some(name) = by_path.get(&resolved) {
                        add_dep(&mut deps, &c.name, name);
                        continue;
                    }
                }
                if let Some(name) = name_to_component.get(pkg_name) {
                    add_dep(&mut deps, &c.name, name);
                }
            }
        }
        c.depends_on = deps.into_iter().collect();
    }
}

// --- Rust ----------------------------------------------------------------

fn parse_cargo_path_deps(content: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut in_deps_section = false;
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if let Some(header) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let head = header.split('.').next().unwrap_or(header);
            in_deps_section = matches!(
                head,
                "dependencies" | "dev-dependencies" | "build-dependencies"
            );
            continue;
        }
        if !in_deps_section {
            continue;
        }
        if let Some(path) = extract_quoted_after(line, "path") {
            paths.push(path);
        }
    }
    paths
}

/// Finds `key`, then `=`, then a quoted string — anywhere in `line`, so
/// this matches both `name = { path = "../x" }` (an inline table on one
/// line, inside `[dependencies]`) and a bare `path = "../x"` line
/// inside a `[dependencies.name]` sub-table. Tries every occurrence of
/// `key` in the line, left to right, not just the first — a dependency
/// whose own name happens to contain `key` as a substring (a crate
/// named `path_two`, say: `path_two = { path = "../path_two" }`) would
/// otherwise have its real `path = "…"` attribute hidden behind an
/// earlier, non-matching occurrence of the word inside the name itself.
fn extract_quoted_after(line: &str, key: &str) -> Option<String> {
    let mut search_from = 0;
    while let Some(rel_idx) = line[search_from..].find(key) {
        let idx = search_from + rel_idx;
        search_from = idx + key.len();
        let Some(rest) = line[search_from..].trim_start().strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(quote) = rest.chars().next() else {
            continue;
        };
        if quote != '"' && quote != '\'' {
            continue;
        }
        let inner = &rest[1..];
        if let Some(end) = inner.find(quote) {
            return Some(inner[..end].to_string());
        }
    }
    None
}

fn resolve_rust(
    repo_root: &Path,
    components: &mut [Component],
    by_path: &BTreeMap<String, String>,
) {
    for c in components.iter_mut().filter(|c| c.kind == "rust") {
        let Ok(content) = std::fs::read_to_string(repo_root.join(&c.path).join("Cargo.toml"))
        else {
            continue;
        };
        let mut deps = BTreeSet::new();
        for rel in parse_cargo_path_deps(&content) {
            let resolved = normalize_relative(&c.path, &rel);
            if let Some(name) = by_path.get(&resolved) {
                add_dep(&mut deps, &c.name, name);
            }
        }
        c.depends_on = deps.into_iter().collect();
    }
}

// --- .NET ----------------------------------------------------------------

fn extract_project_references(content: &str) -> Vec<String> {
    let mut refs = Vec::new();
    for line in content.lines() {
        if !line.contains("<ProjectReference") {
            continue;
        }
        if let Some(r) = extract_quoted_after(line, "Include") {
            refs.push(r);
        }
    }
    refs
}

fn resolve_dotnet(
    repo_root: &Path,
    components: &mut [Component],
    by_path: &BTreeMap<String, String>,
) {
    for c in components.iter_mut().filter(|c| c.kind == "dotnet") {
        let dir = repo_root.join(&c.path);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut deps = BTreeSet::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !(name.ends_with(".csproj") || name.ends_with(".fsproj")) {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            for raw_ref in extract_project_references(&content) {
                let normalized = raw_ref.replace('\\', "/");
                let ref_dir = normalized.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                let resolved = normalize_relative(&c.path, ref_dir);
                if let Some(target) = by_path.get(&resolved) {
                    add_dep(&mut deps, &c.name, target);
                }
            }
        }
        c.depends_on = deps.into_iter().collect();
    }
}

// --- PHP -----------------------------------------------------------------

fn resolve_php(repo_root: &Path, components: &mut [Component], by_path: &BTreeMap<String, String>) {
    let mut name_to_component: BTreeMap<String, String> = BTreeMap::new();
    let mut json_by_name: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for c in components.iter().filter(|c| c.kind == "php") {
        let Ok(content) = std::fs::read_to_string(repo_root.join(&c.path).join("composer.json"))
        else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) else {
            continue;
        };
        if let Some(pkg_name) = v.get("name").and_then(|n| n.as_str()) {
            name_to_component.insert(pkg_name.to_string(), c.name.clone());
        }
        json_by_name.insert(c.name.clone(), v);
    }
    for c in components.iter_mut().filter(|c| c.kind == "php") {
        let Some(v) = json_by_name.get(&c.name) else {
            continue;
        };
        let mut deps = BTreeSet::new();
        if let Some(repos) = v.get("repositories").and_then(|r| r.as_array()) {
            for repo in repos {
                if repo.get("type").and_then(|t| t.as_str()) != Some("path") {
                    continue;
                }
                if let Some(rel) = repo.get("url").and_then(|u| u.as_str()) {
                    let resolved = normalize_relative(&c.path, rel);
                    if let Some(name) = by_path.get(&resolved) {
                        add_dep(&mut deps, &c.name, name);
                    }
                }
            }
        }
        if let Some(reqs) = v.get("require").and_then(|r| r.as_object()) {
            for pkg_name in reqs.keys() {
                if let Some(name) = name_to_component.get(pkg_name) {
                    add_dep(&mut deps, &c.name, name);
                }
            }
        }
        c.depends_on = deps.into_iter().collect();
    }
}

#[cfg(test)]
mod tests;
