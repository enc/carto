//! The INV-4 hard denylist. Not configurable by design — see spec §7.4:
//! "Refuses (hard error, not warning): any path matching the INV-4 denylist
//! ... symlink escape (canonicalize and re-check)."

use crate::error::{Error, Result};
use std::path::{Component, Path};

/// Directory-name components that make a path unwritable no matter where
/// they appear. Deliberately broader than the spec's literal minimum
/// (`**/.git/hooks/**`): carto is read-only over the whole repo (INV-3) and
/// never needs to write inside `.git` or `.ssh` at all, so denying the
/// entire directory is strictly safer and no more restrictive in practice.
const DENIED_DIR_COMPONENTS: &[&str] = &[".claude", ".git", ".ssh", "node_modules"];

/// Exact file names that are never writable, anywhere.
const DENIED_FILE_NAMES: &[&str] = &[
    "CLAUDE.md",
    ".bashrc",
    ".zshrc",
    ".zshenv",
    ".profile",
    ".bash_profile",
    ".gitconfig",
];

/// Refuses `path` if it matches the INV-4 denylist. Checked twice by
/// [`super::PathGuard::writer`]: once on the caller-supplied relative path
/// (fails fast, no fs touched) and once on the fully resolved canonical
/// path (catches a denied component contributed by `out_root` itself, or
/// surfaced only after resolving a symlink).
pub fn check(path: &Path) -> Result<()> {
    for comp in path.components() {
        if let Component::Normal(os) = comp {
            if let Some(s) = os.to_str() {
                if DENIED_DIR_COMPONENTS.contains(&s) {
                    return Err(Error::invariant_refusal(format!(
                        "refusing to write: path contains denylisted component `{s}`"
                    )));
                }
            }
        }
    }

    if let Some(file_name) = path.file_name().and_then(|f| f.to_str()) {
        if DENIED_FILE_NAMES.contains(&file_name) {
            return Err(Error::invariant_refusal(format!(
                "refusing to write: `{file_name}` is on the INV-4 denylist"
            )));
        }
    }

    if is_under_disallowed_config_dir(path) {
        return Err(Error::invariant_refusal(
            "refusing to write under ~/.config, except carto's own subdirectory",
        ));
    }

    Ok(())
}

/// `~/.config/**` is denied except carto's own subdirectory
/// (`~/.config/carto/`), per spec §7.4.
fn is_under_disallowed_config_dir(path: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let config_dir = std::path::PathBuf::from(home).join(".config");
    if !path.starts_with(&config_dir) {
        return false;
    }
    let carto_config = config_dir.join(crate::consts::BIN_NAME);
    !path.starts_with(&carto_config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn denies_dot_claude_anywhere_in_path() {
        assert!(check(&PathBuf::from("out/.claude/settings.json")).is_err());
        assert!(check(&PathBuf::from(".claude/foo")).is_err());
    }

    #[test]
    fn denies_claude_md_filename() {
        assert!(check(&PathBuf::from("some/nested/CLAUDE.md")).is_err());
    }

    #[test]
    fn denies_git_hooks() {
        assert!(check(&PathBuf::from("repo/.git/hooks/pre-commit")).is_err());
    }

    #[test]
    fn denies_shell_rc_files() {
        for name in [".bashrc", ".zshrc", ".zshenv", ".profile", ".gitconfig"] {
            assert!(
                check(&PathBuf::from(name)).is_err(),
                "{name} should be denied"
            );
        }
    }

    #[test]
    fn allows_ordinary_out_dir_paths() {
        assert!(check(&PathBuf::from("/home/user/.cache/carto/abc123/graph.json")).is_ok());
    }
}
