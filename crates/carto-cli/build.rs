//! Emits `CARTO_GIT_SHA` for `selfcheck`/`--version` by reading `.git/HEAD`
//! directly with `std::fs` — no `git` subprocess, keeping INV-2 ("no
//! foreign code execution") true at build time too. See
//! docs/adr/0003-buildrs-no-subprocess.md.
//!
//! Falls back to `"unknown"` (never fails the build) when `.git` is absent,
//! e.g. building from a source tarball with no VCS metadata.

use std::path::{Path, PathBuf};

fn main() {
    let sha = git_sha().unwrap_or_else(|| "unknown".to_string());
    println!("cargo::rustc-env=CARTO_GIT_SHA={sha}");
    // Re-run only when HEAD or the ref it points at changes, not on every
    // build.
    if let Some(git_dir) = find_git_dir() {
        println!("cargo::rerun-if-changed={}", git_dir.join("HEAD").display());
        if let Some(ref_path) = head_ref_path(&git_dir) {
            println!("cargo::rerun-if-changed={}", ref_path.display());
        }
    }

    // Cargo already sets TARGET for build scripts; re-expose it under
    // carto's own env var name so selfcheck.rs doesn't depend on Cargo's
    // naming. Deliberately no `rustc --version` subprocess call here —
    // build.rs stays subprocess-free (ADR-0003); CARGO_PKG_RUST_VERSION
    // (the declared MSRV from Cargo.toml's `rust-version`) is reported
    // instead via `env!()` directly in selfcheck.rs, no build.rs plumbing
    // needed.
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo::rustc-env=CARTO_TARGET_TRIPLE={target}");
}

fn git_sha() -> Option<String> {
    let git_dir = find_git_dir()?;
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();

    if let Some(ref_name) = head.strip_prefix("ref: ") {
        let sha = std::fs::read_to_string(git_dir.join(ref_name)).ok()?;
        Some(sha.trim().to_string())
    } else {
        // Detached HEAD: HEAD itself contains the SHA.
        Some(head.to_string())
    }
}

fn head_ref_path(git_dir: &Path) -> Option<PathBuf> {
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let ref_name = head.trim().strip_prefix("ref: ")?;
    Some(git_dir.join(ref_name))
}

/// Walks up from `CARGO_MANIFEST_DIR` looking for a `.git` entry (directory
/// for a normal checkout, or a `.git` *file* pointing elsewhere for a
/// worktree/submodule — the latter is not resolved further since carto
/// doesn't need worktree support for a version string).
fn find_git_dir() -> Option<PathBuf> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let mut dir = PathBuf::from(manifest_dir);
    loop {
        let candidate = dir.join(".git");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}
