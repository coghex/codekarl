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

/// Where the running game's archive reader is, for the view builder (CK-377 §1): like [`Handle`], a load replaces what it holds, and the
/// builder reads the current reader each time it builds a view.
pub(crate) type SourceCell = Arc<Mutex<Option<Arc<ck_characters::ArchiveSource>>>>;

/// A storage directory, removed when its last owner lets go: the storage itself, and every archive reader (so every published view) that
/// still reads its files after a load has replaced the storage (CK-377 §1).
#[derive(Debug)]
pub(super) struct Directory(pub(super) PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) struct Storage {
    pub(super) path: PathBuf,
    /// The cell this storage's path is published to, if the game's scheduler reads it.
    pub(super) handle: Option<Handle>,
    /// The cell this storage's archive reader is published to, if the game builds views.
    pub(super) cell: Option<SourceCell>,
    /// The directory's owner, shared with the archive reader.
    directory: Arc<Directory>,
    /// The archive reader of this storage's `ARCH`, made on first use.
    source: std::sync::OnceLock<Arc<ck_characters::ArchiveSource>>,
}
impl Storage {
    /// A storage at `path`, which it owns: the directory goes when the storage and every reader of its archive have.
    pub(super) fn at(path: PathBuf, handle: Option<Handle>, cell: Option<SourceCell>) -> Self {
        Self {
            directory: Arc::new(Directory(path.clone())),
            path,
            handle,
            cell,
            source: std::sync::OnceLock::new(),
        }
    }
    /// The reader of this storage's archive.
    pub(super) fn source(&self) -> &Arc<ck_characters::ArchiveSource> {
        self.source.get_or_init(|| {
            ck_characters::ArchiveSource::new(self.path.join("ARCH"), self.directory.clone())
        })
    }
    /// Publish this storage's path and archive reader to their cells.
    pub(super) fn publish(&self) {
        if let Some(handle) = &self.handle
            && let Ok(mut cell) = handle.lock()
        {
            *cell = Some(self.path.clone());
        }
        if let Some(cell) = &self.cell
            && let Ok(mut held) = cell.lock()
        {
            *held = Some(Arc::clone(self.source()));
        }
    }
    /// Share `handle` with the scheduler and `cell` with the view builder, and publish both.
    pub(super) fn attach(&mut self, handle: &Handle, cell: &SourceCell) {
        self.handle = Some(Arc::clone(handle));
        self.cell = Some(Arc::clone(cell));
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
            Ok(Self::at(path.clone(), None, None))
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
