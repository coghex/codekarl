//! Disk-owned archive and output logs for a running game.
use super::GameState;
use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);

pub(super) struct Storage {
    pub(super) path: PathBuf,
}
impl Storage {
    pub(super) fn fresh_path(parent: &Path) -> io::Result<PathBuf> {
        std::fs::create_dir_all(parent)?;
        for _ in 0..128 {
            let path = parent.join(format!(
                ".characters-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            if !path.exists() {
                return Ok(path);
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "cannot reserve character storage",
        ))
    }
    pub(super) fn empty(parent: &Path) -> io::Result<Self> {
        let path = Self::fresh_path(parent)?;
        std::fs::create_dir(&path)?;
        let result = (|| {
            ck_characters::Archive::open(path.join("ARCH"))?;
            ck_characters::LifeLog::open(path.join("LOGS"))?;
            Ok(Self { path: path.clone() })
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(path);
        }
        result
    }
    pub(super) fn validate(path: &Path, state: &GameState) -> io::Result<()> {
        let archive = ck_characters::Archive::open(path.join("ARCH"))
            .map_err(|e| io::Error::other(format!("ARCH section: {e}")))?;
        state.characters.validate_archive(&archive)?;
        state
            .characters
            .validate_pops(|locator| {
                let place = state
                    .economy
                    .places
                    .iter()
                    .find(|p| p.id == locator.place)?;
                let pop = place.pops.get(locator.slot as usize)?;
                pop.workers.checked_add(pop.dependents)
            })
            .map_err(|e| io::Error::other(format!("STAT section: {e}")))?;
        let logs = ck_characters::LifeLog::open(path.join("LOGS"))?;
        logs.visit(|line| {
            if state.characters.get(line.id).is_some() || archive.contains(line.id) {
                Ok(())
            } else {
                Err(io::Error::other("unknown character"))
            }
        })
        .map_err(|e| io::Error::other(format!("LOGS section: {e}")))
    }
}
impl Drop for Storage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
