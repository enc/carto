//! Shared repo-path -> out-dir resolution (every command in spec §7.1
//! takes "repo path" [+ optional `--out`] as its first input). `index`
//! additionally needs a [`carto_core::pathguard::PathGuard`] (it's the
//! one command that writes); `where`/`deps`/`map` only ever read
//! `<out>/graph.json`, but computing the default out-dir still needs the
//! same canonicalized `repo_root` `index` uses — so that half is shared
//! here rather than kept as four inline copies.

use carto_core::error::{Error, ErrorKind, Result};
use carto_core::outdir;
use std::path::{Path, PathBuf};

pub struct Target {
    pub repo_root: PathBuf,
    pub out_root: PathBuf,
}

pub fn resolve(path: &Path, out: &Option<PathBuf>) -> Result<Target> {
    let repo_root = path.canonicalize().map_err(|e| {
        Error::with_source(
            ErrorKind::UserError,
            format!("cannot resolve repo path `{}`", path.display()),
            e,
        )
    })?;

    let out_root = match out {
        Some(p) => p.clone(),
        None => outdir::default_out_root(&repo_root)?,
    };

    Ok(Target {
        repo_root,
        out_root,
    })
}
