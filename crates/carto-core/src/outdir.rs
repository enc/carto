//! Resolves the default output root (INV-3): `${XDG_CACHE_HOME:-~/.cache}/carto/<repo-hash>/`.
//! Spec §2 (INV-3), §4.3 (stable-ID hash recipe — reused here for the
//! repo-hash component so there's one blake3-hex-prefix helper in the
//! codebase, not two: [`crate::graph::id::blake3_hex_prefix`]).

use crate::error::{Error, ErrorKind, Result};
use std::path::{Path, PathBuf};

/// Computes `${XDG_CACHE_HOME:-~/.cache}/carto/<repo-hash>/` for
/// `repo_root`. `repo_root` is canonicalized first so that `.`, symlinks,
/// and relative invocations of carto against the same repo all resolve to
/// the same out-dir.
pub fn default_out_root(repo_root: &Path) -> Result<PathBuf> {
    let canonical = repo_root.canonicalize().map_err(|e| {
        Error::with_source(
            ErrorKind::UserError,
            format!("cannot resolve repo path `{}`", repo_root.display()),
            e,
        )
    })?;
    let hash = repo_hash(&canonical);
    Ok(cache_home()?.join(crate::consts::BIN_NAME).join(hash))
}

/// `${XDG_CACHE_HOME:-~/.cache}`.
fn cache_home() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    let home = std::env::var_os("HOME").ok_or_else(|| {
        Error::new(
            ErrorKind::UserError,
            "cannot determine cache directory: neither XDG_CACHE_HOME nor HOME is set",
        )
    })?;
    Ok(PathBuf::from(home).join(".cache"))
}

fn repo_hash(canonical_repo_path: &Path) -> String {
    crate::graph::id::blake3_hex_prefix(canonical_repo_path.to_string_lossy().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_repo_path_yields_same_hash() {
        let a = repo_hash(Path::new("/some/repo"));
        let b = repo_hash(Path::new("/some/repo"));
        assert_eq!(a, b);
    }

    #[test]
    fn different_repo_paths_yield_different_hashes() {
        let a = repo_hash(Path::new("/some/repo"));
        let b = repo_hash(Path::new("/some/other-repo"));
        assert_ne!(a, b);
    }

    #[test]
    fn hash_is_expected_length() {
        assert_eq!(
            repo_hash(Path::new("/x")).len(),
            crate::graph::id::ID_HEX_LEN
        );
    }

    #[test]
    fn default_out_root_ends_in_bin_name_and_hash() {
        let dir = std::env::temp_dir().join(format!("carto-outdir-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = default_out_root(&dir).unwrap();
        assert!(out.ends_with(repo_hash(&dir.canonicalize().unwrap())));
        assert!(out.to_string_lossy().contains("carto"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
