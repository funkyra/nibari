use std::fs::{File, OpenOptions, TryLockError};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

pub fn acquire() -> Result<File> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .context("XDG_RUNTIME_DIR must be set to lock the bar instance")?;
    let path = PathBuf::from(runtime).join("nibari.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("failed to open instance lock {}", path.display()))?;

    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => bail!("nibari is already running"),
        Err(TryLockError::Error(error)) => Err(error).context("failed to lock the bar instance"),
    }
    // Keep the file open for the bar's lifetime. Never unlink it: other processes
    // must lock the same inode. The OS releases the lock even after a crash.
}
