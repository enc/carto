//! `AtomicFile`: tmp-file-then-rename writer used by every write path in
//! carto (spec §6.5 serialisation choke-point, §7.4 pathguard). Hand-rolled
//! instead of pulling in `tempfile` — see
//! docs/adr/0001-hand-rolled-error-atomicfile.md.

use crate::error::{Error, ErrorKind, Result};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

/// A file opened for atomic replacement of `target`: writes land in a
/// sibling temp file first, and only `commit()` makes them visible at
/// `target` via `rename`. Dropping without calling `commit()` removes the
/// temp file, leaving no partial output behind.
///
/// Determinism note (INV-7): the temp file name is built from the process
/// ID plus a small retry counter, not a random suffix, so repeated runs
/// don't introduce nondeterminism into anything that might (incorrectly)
/// observe directory listings during a write. The *committed* file name is
/// always exactly `target`'s file name.
pub struct AtomicFile {
    file: Option<File>,
    tmp_path: PathBuf,
    target: PathBuf,
    committed: bool,
}

impl AtomicFile {
    /// Creates the backing temp file next to `target`. `target`'s parent
    /// directory must already exist (pathguard callers create `out_root`
    /// up front; this type does not create directories itself, to keep its
    /// failure modes narrow).
    pub(super) fn create(target: PathBuf) -> Result<Self> {
        let parent = target.parent().ok_or_else(|| {
            Error::new(
                ErrorKind::UserError,
                format!("target path `{}` has no parent directory", target.display()),
            )
        })?;
        let file_name = target.file_name().ok_or_else(|| {
            Error::new(
                ErrorKind::UserError,
                format!("target path `{}` has no file name", target.display()),
            )
        })?;

        let pid = std::process::id();
        let mut last_err = None;
        for seq in 0..1000u32 {
            let tmp_name = format!("{}.carto-tmp.{pid}.{seq}", file_name.to_string_lossy());
            let tmp_path = parent.join(tmp_name);
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_path)
            {
                Ok(file) => {
                    return Ok(AtomicFile {
                        file: Some(file),
                        tmp_path,
                        target,
                        committed: false,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    last_err = Some(e);
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(Error::with_source(
            ErrorKind::DataError,
            "exhausted temp-file name attempts",
            last_err.unwrap_or_else(|| std::io::Error::other("no attempts made")),
        ))
    }

    /// The path writes should go to. Same as calling [`std::io::Write`] on
    /// this `AtomicFile` directly, exposed for callers that need the path
    /// (e.g. to hand to a library that writes by path).
    pub fn tmp_path(&self) -> &Path {
        &self.tmp_path
    }

    /// Flushes and syncs the temp file to durable storage, then renames it
    /// onto `target`. Consumes `self`; after this call the write is
    /// externally visible at `target`.
    pub fn commit(mut self) -> Result<()> {
        if let Some(file) = self.file.take() {
            file.sync_all()?;
        }
        std::fs::rename(&self.tmp_path, &self.target)?;
        self.committed = true;
        Ok(())
    }
}

impl Write for AtomicFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file
            .as_mut()
            .expect("file handle taken only by commit(), which consumes self")
            .write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file
            .as_mut()
            .expect("file handle taken only by commit(), which consumes self")
            .flush()
    }
}

impl Drop for AtomicFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.tmp_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_makes_content_visible_at_target() {
        let dir = std::env::temp_dir().join(format!("carto-atomic-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("out.txt");

        let mut f = AtomicFile::create(target.clone()).unwrap();
        f.write_all(b"hello").unwrap();
        f.commit().unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn drop_without_commit_leaves_no_tmp_file() {
        let dir =
            std::env::temp_dir().join(format!("carto-atomic-test-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("out.txt");

        let tmp_path;
        {
            let mut f = AtomicFile::create(target.clone()).unwrap();
            f.write_all(b"never committed").unwrap();
            tmp_path = f.tmp_path().to_path_buf();
            // f dropped here without commit()
        }

        assert!(!tmp_path.exists());
        assert!(!target.exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
