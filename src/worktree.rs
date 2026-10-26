// Open choice: canonical session cwd; the alternative is the Git worktree root.
use crate::{Error, failure};
use std::path::PathBuf;

pub(crate) fn resolve(worktree: PathBuf) -> Result<PathBuf, Error> {
    if !worktree.is_absolute() || !worktree.is_dir() {
        return Err(failure("worktree must be an existing absolute directory"));
    }
    let worktree = worktree
        .canonicalize()
        .map_err(|_| failure("worktree is unavailable"))?;
    Ok(worktree)
}
