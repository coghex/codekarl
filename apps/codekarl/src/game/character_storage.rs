//! Disk-owned archive and output logs for a running game.
use super::GameState;
use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT: AtomicU64 = AtomicU64::new(0);

/// Where the running game's character storage is, for the scheduler's daily system (CK-363): a load replaces the directory, so
/// the system reads the current path from this shared cell each time it needs the archive and the life logs.
pub(crate) type Handle = Arc<Mutex<Option<PathBuf>>>;

pub(super) struct Storage {
    pub(super) path: PathBuf,
    /// The cell this storage's path is published to, if the game's scheduler reads it.
    pub(super) handle: Option<Handle>,
}
impl Storage {
    /// Publish this storage's path to its handle.
    pub(super) fn publish(&self) {
        if let Some(handle) = &self.handle
            && let Ok(mut cell) = handle.lock()
        {
            *cell = Some(self.path.clone());
        }
    }
    /// Share `handle` with the scheduler and publish the path.
    pub(super) fn attach(&mut self, handle: &Handle) {
        self.handle = Some(Arc::clone(handle));
        self.publish();
    }
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
            Ok(Self {
                path: path.clone(),
                handle: None,
            })
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
