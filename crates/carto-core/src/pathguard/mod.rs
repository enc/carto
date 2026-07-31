//! `pathguard` — the write guard enforcing INV-3 ("repo is read-only;
//! output goes to one directory") and INV-4 ("no agent-config or VCS
//! mutation"). All fs writes anywhere in carto go through
//! [`PathGuard::writer`]. Spec §3.1, §7.4.

mod atomic;
mod denylist;

pub use atomic::AtomicFile;
pub use denylist::digest as denylist_digest;

use crate::error::{Error, Result};
use std::path::{Component, Path, PathBuf};

/// Guards writes to a single output root. Constructed once per invocation
/// from the resolved `--out` (or the XDG-cache default from
/// [`crate::outdir`]) and the repo root being indexed.
pub struct PathGuard {
    out_root: PathBuf,
    repo_root: PathBuf,
    allow_writes_in_repo: bool,
}

impl PathGuard {
    /// `out_root` is created if it doesn't exist (that's the one directory
    /// carto is allowed to create) — but only *after* the INV-4 denylist
    /// has cleared it, both lexically and (via the longest existing
    /// ancestor, same trick [`Self::writer`] uses) on its canonical form.
    /// Checking before creating matters: without this, `carto index --out
    /// ~/.claude/carto` would create a directory inside `.claude` and only
    /// refuse on the first actual write, in `writer()` — a real (if
    /// short-lived) INV-4 violation, not just a reported one.
    ///
    /// `allow_writes_in_repo` is true only when the user passed `--out`
    /// pointing explicitly inside the repo — spec §7.4: "any path under
    /// repo root unless `--out` inside repo was explicit".
    pub fn new(repo_root: &Path, out_root: &Path, allow_writes_in_repo: bool) -> Result<Self> {
        denylist::check(out_root)?;
        let canonical_out_root = canonicalize_via_longest_existing_ancestor(out_root)?;
        denylist::check(&canonical_out_root)?;

        std::fs::create_dir_all(out_root)?;
        let repo_root = repo_root.canonicalize()?;
        let out_root = out_root.canonicalize()?;
        Ok(PathGuard {
            out_root,
            repo_root,
            allow_writes_in_repo,
        })
    }

    /// Opens an atomic writer for `relative`, a path relative to `out_root`.
    /// Refuses (hard error, `ErrorKind::InvariantRefusal`, never a warning)
    /// per spec §7.4:
    ///
    /// 1. `relative` is actually relative and contains no `..` / root
    ///    component (checked lexically, before any fs access).
    /// 2. The INV-4 hard denylist (not configurable — [`denylist::check`]),
    ///    checked once lexically here and again on the canonical path
    ///    below, since a symlink can only introduce *new* path segments,
    ///    never remove the ones already checked.
    /// 3. Symlink escape: the longest existing ancestor of the joined path
    ///    is canonicalized and the result re-checked against `out_root`.
    /// 4. The canonical target must not be under `repo_root`, unless
    ///    `allow_writes_in_repo` was set.
    pub fn writer(&self, relative: &Path) -> Result<AtomicFile> {
        if relative.is_absolute() {
            return Err(Error::invariant_refusal(format!(
                "refusing to write: `{}` is not relative to the out directory",
                relative.display()
            )));
        }
        if relative
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::RootDir))
        {
            return Err(Error::invariant_refusal(format!(
                "refusing to write: `{}` escapes its parent via `..`",
                relative.display()
            )));
        }

        denylist::check(relative)?;

        let joined = self.out_root.join(relative);
        let canonical = canonicalize_via_longest_existing_ancestor(&joined)?;

        denylist::check(&canonical)?;

        if !canonical.starts_with(&self.out_root) {
            return Err(Error::invariant_refusal(format!(
                "refusing to write: `{}` resolves outside the out directory (symlink escape?)",
                relative.display()
            )));
        }

        if canonical.starts_with(&self.repo_root) && !self.allow_writes_in_repo {
            return Err(Error::invariant_refusal(format!(
                "refusing to write: `{}` resolves inside the repository root; \
                 pass an explicit --out inside the repo to allow this",
                relative.display()
            )));
        }

        if let Some(parent) = canonical.parent() {
            std::fs::create_dir_all(parent)?;
        }
        AtomicFile::create(canonical)
    }

    pub fn out_root(&self) -> &Path {
        &self.out_root
    }
}

/// Canonicalizes `path` by resolving the longest prefix of it that already
/// exists on disk, then re-joining the remaining (not-yet-created)
/// components. This lets pathguard catch a symlink escape introduced by an
/// existing ancestor directory even though the final file itself doesn't
/// exist yet (it's about to be created).
fn canonicalize_via_longest_existing_ancestor(path: &Path) -> Result<PathBuf> {
    let mut existing = path;
    let mut remainder: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if existing.exists() {
            break;
        }
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                remainder.push(name);
                existing = parent;
            }
            _ => break, // reached the root without finding an existing ancestor
        }
    }

    let mut canonical = existing.canonicalize()?;
    for name in remainder.into_iter().rev() {
        canonical.push(name);
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "carto-pathguard-test-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn setup(tag: &str) -> (TempDir, PathGuard) {
        let base = TempDir::new(tag);
        let repo_root = base.path().join("repo");
        let out_root = base.path().join("out");
        std::fs::create_dir_all(&repo_root).unwrap();
        std::fs::create_dir_all(&out_root).unwrap();
        let guard = PathGuard::new(&repo_root, &out_root, false).unwrap();
        (base, guard)
    }

    #[test]
    fn writes_graph_json_atomically() {
        let (_base, guard) = setup("basic");
        let mut w = guard.writer(Path::new("graph.json")).unwrap();
        w.write_all(b"{}").unwrap();
        w.commit().unwrap();
        assert_eq!(
            std::fs::read_to_string(guard.out_root().join("graph.json")).unwrap(),
            "{}"
        );
    }

    #[test]
    fn refuses_parent_dir_escape() {
        let (_base, guard) = setup("dotdot");
        assert!(guard.writer(Path::new("../escape.json")).is_err());
    }

    #[test]
    fn refuses_absolute_path() {
        let (_base, guard) = setup("absolute");
        assert!(guard.writer(Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn refuses_denylisted_relative_paths() {
        let (_base, guard) = setup("denylist");
        assert!(guard.writer(Path::new(".claude/settings.json")).is_err());
        assert!(guard.writer(Path::new("CLAUDE.md")).is_err());
        assert!(guard.writer(Path::new(".git/hooks/pre-commit")).is_err());
        assert!(guard.writer(Path::new(".zshrc")).is_err());
    }

    #[test]
    fn refuses_a_denylisted_out_root_without_creating_it() {
        let base = TempDir::new("denylisted-out-root");
        let repo_root = base.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();
        let out_root = base.path().join(".claude").join("carto");

        assert!(PathGuard::new(&repo_root, &out_root, false).is_err());
        assert!(
            !out_root.exists(),
            "a refused out-root must never be created, even partially"
        );
        assert!(!base.path().join(".claude").exists());
    }

    #[test]
    fn refuses_symlink_escape_from_out_root() {
        let (base, guard) = setup("symlink");
        let outside = base.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let link = guard.out_root().join("escape-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        assert!(guard.writer(Path::new("escape-link/pwned.json")).is_err());
    }

    #[test]
    fn refuses_writes_inside_repo_root_by_default() {
        let base = TempDir::new("inrepo");
        let repo_root = base.path().join("repo");
        let out_root = repo_root.join(".carto-out");
        std::fs::create_dir_all(&repo_root).unwrap();
        let guard = PathGuard::new(&repo_root, &out_root, false).unwrap();
        assert!(guard.writer(Path::new("graph.json")).is_err());
    }

    #[test]
    fn allows_writes_inside_repo_root_when_explicit() {
        let base = TempDir::new("inrepo-allowed");
        let repo_root = base.path().join("repo");
        let out_root = repo_root.join(".carto-out");
        std::fs::create_dir_all(&repo_root).unwrap();
        let guard = PathGuard::new(&repo_root, &out_root, true).unwrap();
        let mut w = guard.writer(Path::new("graph.json")).unwrap();
        w.write_all(b"{}").unwrap();
        w.commit().unwrap();
        assert!(out_root.join("graph.json").exists());
    }
}
