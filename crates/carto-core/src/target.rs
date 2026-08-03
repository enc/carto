//! Shared repo-path -> out-dir resolution. Every read/write command in
//! spec §7.1 (`index`, `where`, `deps`, `map`, and the MCP tools mirroring
//! them, spec §7.3) takes "repo path" [+ optional `--out`] as its first
//! input; this used to live in `carto-cli/src/target.rs` as a CLI-private
//! helper, but `carto-mcp` needs the exact same resolution (same default
//! out-dir computation via [`crate::outdir`]) rather than a second,
//! possibly-drifting copy — moved here so both front ends share one
//! implementation.

use crate::error::{Error, ErrorKind, Result};
use crate::outdir;
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
