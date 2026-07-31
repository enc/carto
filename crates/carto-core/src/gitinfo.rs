//! Reads the current commit SHA directly from `.git`, no `git` subprocess
//! (INV-2) — same no-subprocess constraint as `crates/carto-cli/build.rs`
//! (ADR-0003), reused here for `manifest.json`'s `commit_sha` field (spec
//! §4.4). Deliberately not shared code with build.rs: a build script runs
//! in a separate compilation context and can't cleanly depend on the
//! crate it's building for. Unlike build.rs's cosmetic version string,
//! `commit_sha` is manifest data an operator might rely on, so this
//! handles two cases build.rs's ADR explicitly skips: `packed-refs` and
//! (partially) git worktrees. See docs/adr/0006-manifest-git-provenance.md
//! for why `dirty` still isn't attempted.

use std::path::{Path, PathBuf};

/// The current commit SHA for the repo at `repo_root`, or `None` if
/// `repo_root` isn't inside a git working tree, or anything about its
/// `.git` metadata can't be read or doesn't parse as expected. Never
/// errors — spec's honesty principle (INV-8's spirit, applied to
/// manifest data) means an unknown SHA is recorded as `null`, not
/// guessed at or defaulted to something misleading.
pub fn head_sha(repo_root: &Path) -> Option<String> {
    let git_dir = find_git_dir(repo_root)?;
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();

    if let Some(ref_name) = head.strip_prefix("ref: ") {
        resolve_ref(&git_dir, ref_name.trim())
    } else if is_sha(head) {
        // Detached HEAD: HEAD itself contains the SHA.
        Some(head.to_string())
    } else {
        None
    }
}

/// Resolves `ref_name` (e.g. `refs/heads/main`) to a commit SHA: a loose
/// ref file under `git_dir`, then (for worktrees) the shared refs under
/// `commondir`, then `packed-refs` in each of those, in that order.
fn resolve_ref(git_dir: &Path, ref_name: &str) -> Option<String> {
    if let Some(sha) = read_loose_ref(git_dir, ref_name) {
        return Some(sha);
    }
    if let Ok(commondir) = std::fs::read_to_string(git_dir.join("commondir")) {
        let common = git_dir.join(commondir.trim());
        if let Some(sha) = read_loose_ref(&common, ref_name) {
            return Some(sha);
        }
        if let Some(sha) = read_packed_ref(&common, ref_name) {
            return Some(sha);
        }
    }
    read_packed_ref(git_dir, ref_name)
}

fn read_loose_ref(git_dir: &Path, ref_name: &str) -> Option<String> {
    let sha = std::fs::read_to_string(git_dir.join(ref_name)).ok()?;
    let sha = sha.trim();
    is_sha(sha).then(|| sha.to_string())
}

fn read_packed_ref(git_dir: &Path, ref_name: &str) -> Option<String> {
    let packed = std::fs::read_to_string(git_dir.join("packed-refs")).ok()?;
    for line in packed.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        if let Some((sha, name)) = line.split_once(' ') {
            if name == ref_name && is_sha(sha) {
                return Some(sha.to_string());
            }
        }
    }
    None
}

/// Finds `.git`, walking up from `start`. `.git` may be a directory
/// (normal checkout) or a file containing `gitdir: <path>` (worktrees,
/// submodules) — build.rs's ADR-0003 only handles the directory form;
/// this resolves the file form's target directory too (though ref
/// resolution beyond that directory and its `commondir` is not attempted
/// further — nested worktree/submodule chains are not resolved).
fn find_git_dir(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join(".git");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if candidate.is_file() {
            if let Ok(contents) = std::fs::read_to_string(&candidate) {
                if let Some(path) = contents.trim().strip_prefix("gitdir: ") {
                    let resolved = dir.join(path);
                    if resolved.is_dir() {
                        return Some(resolved);
                    }
                }
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn is_sha(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "carto-gitinfo-test-{tag}-{}-{:?}",
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

    const FAKE_SHA: &str = "abc123def456abc123def456abc123def456abc";

    #[test]
    fn resolves_via_loose_ref() {
        let dir = TempDir::new("loose");
        let git = dir.path().join(".git");
        std::fs::create_dir_all(git.join("refs/heads")).unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(git.join("refs/heads/main"), format!("{FAKE_SHA}\n")).unwrap();

        assert_eq!(head_sha(dir.path()), Some(FAKE_SHA.to_string()));
    }

    #[test]
    fn resolves_detached_head() {
        let dir = TempDir::new("detached");
        let git = dir.path().join(".git");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::write(git.join("HEAD"), format!("{FAKE_SHA}\n")).unwrap();

        assert_eq!(head_sha(dir.path()), Some(FAKE_SHA.to_string()));
    }

    #[test]
    fn resolves_via_packed_refs_when_loose_ref_missing() {
        let dir = TempDir::new("packed");
        let git = dir.path().join(".git");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(
            git.join("packed-refs"),
            format!("# pack-refs with: peeled fully-peeled sorted\n{FAKE_SHA} refs/heads/main\n"),
        )
        .unwrap();

        assert_eq!(head_sha(dir.path()), Some(FAKE_SHA.to_string()));
    }

    #[test]
    fn finds_git_dir_from_a_nested_subdirectory() {
        let dir = TempDir::new("nested");
        let git = dir.path().join(".git");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::write(git.join("HEAD"), format!("{FAKE_SHA}\n")).unwrap();
        let nested = dir.path().join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();

        assert_eq!(head_sha(&nested), Some(FAKE_SHA.to_string()));
    }

    #[test]
    fn resolves_worktree_gitdir_file_and_shared_refs_via_commondir() {
        let dir = TempDir::new("worktree");
        let main_git = dir.path().join("main-repo/.git");
        std::fs::create_dir_all(main_git.join("refs/heads")).unwrap();
        std::fs::write(main_git.join("refs/heads/main"), format!("{FAKE_SHA}\n")).unwrap();

        let wt_git_dir = main_git.join("worktrees/wt1");
        std::fs::create_dir_all(&wt_git_dir).unwrap();
        std::fs::write(wt_git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(wt_git_dir.join("commondir"), "../..\n").unwrap();

        let worktree_checkout = dir.path().join("worktree-checkout");
        std::fs::create_dir_all(&worktree_checkout).unwrap();
        std::fs::write(
            worktree_checkout.join(".git"),
            format!("gitdir: {}\n", wt_git_dir.display()),
        )
        .unwrap();

        assert_eq!(head_sha(&worktree_checkout), Some(FAKE_SHA.to_string()));
    }

    #[test]
    fn returns_none_when_not_a_git_repo() {
        let dir = TempDir::new("notgit");
        assert_eq!(head_sha(dir.path()), None);
    }

    #[test]
    fn returns_none_for_malformed_head() {
        let dir = TempDir::new("malformed");
        let git = dir.path().join(".git");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::write(git.join("HEAD"), "not a valid HEAD line\n").unwrap();

        assert_eq!(head_sha(dir.path()), None);
    }
}
