//! A shared, read-only, incremental reader of an archive file for the UI's views (CK-377 §1).
//!
//! The simulation owns the archive and appends to it; the views it publishes must read **that world's archive as it was at the
//! publication** and keep reading it for as long as any view of it lives, however far the game has moved on (or been replaced by a load). An
//! [`ArchiveSource`] is the one reader of one archive file: it keeps an identifier-to-offset index and a parent-to-children index in memory,
//! extended from where it last stopped (never rescanning), and hands out [`ArchiveView`]s, each fixed at the published length the file had
//! when it was made. The archive is append-only, so a view's record is exactly a record whose offset lies before that length: a record
//! archived later is invisible to it, and a lookup is one seek and one read.
//!
//! A source also holds what keeps the archive's directory alive (the game removes a storage directory when its last owner lets go), so a
//! view of an old world still reads its own files after a load has replaced them.

use std::{
    any::Any,
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use ck_sim::Canonical;

use crate::{Character, UNKNOWN, read_record, scan_archive};

/// The two indexes and how far the file has been read.
#[derive(Debug, Default)]
struct Index {
    /// Each archived identifier's record offset.
    by_id: BTreeMap<u64, u64>,
    /// The archived identifiers of each known parent, each with its record offset, in the order archived.
    by_parent: BTreeMap<u64, Vec<(u64, u64)>>,
    /// The length of the published bytes read so far: where the next scan starts.
    scanned: u64,
}

/// The one reader of one archive file. See the module documentation.
pub struct ArchiveSource {
    path: PathBuf,
    index: RwLock<Index>,
    /// What keeps the archive's directory alive.
    _keep: Arc<dyn Any + Send + Sync>,
}

impl std::fmt::Debug for ArchiveSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArchiveSource")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl ArchiveSource {
    /// A reader of the archive file at `path`. Nothing is read until [`view`](Self::view). `keep` is held for as long as the source lives:
    /// whatever owns the file's directory.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, keep: Arc<dyn Any + Send + Sync>) -> Arc<Self> {
        Arc::new(Self {
            path: path.into(),
            index: RwLock::new(Index::default()),
            _keep: keep,
        })
    }

    /// Read what has been published to the archive since the last call and return a view fixed at the length now published. Only the new
    /// records are read.
    ///
    /// # Errors
    /// Returns an I/O or decoding error; the index is left as it was before the failing scan.
    pub fn view(self: &Arc<Self>) -> std::io::Result<ArchiveView> {
        let mut index = self
            .index
            .write()
            .map_err(|_| std::io::Error::other("archive index poisoned"))?;
        let mut file = std::fs::File::open(&self.path)?;
        let size = file.metadata()?.len();
        if size < index.scanned {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the archive is shorter than it was published",
            ));
        }
        let mut found: Vec<(u64, u64, u64, u64)> = Vec::new();
        let scanned = scan_archive(&mut file, index.scanned, size, |record, offset| {
            found.push((record.id, offset, record.father, record.mother));
            Ok(())
        })?;
        for (id, offset, father, mother) in found {
            if id == UNKNOWN || index.by_id.contains_key(&id) {
                continue;
            }
            index.by_id.insert(id, offset);
            for parent in [father, mother] {
                if parent != UNKNOWN {
                    index
                        .by_parent
                        .entry(parent)
                        .or_default()
                        .push((id, offset));
                }
            }
        }
        index.scanned = scanned;
        Ok(ArchiveView {
            source: Arc::clone(self),
            limit: scanned,
        })
    }
}

/// An archive as it was when a view was published: the records whose offsets lie before `limit`.
#[derive(Clone, Debug)]
pub struct ArchiveView {
    source: Arc<ArchiveSource>,
    limit: u64,
}

impl PartialEq for ArchiveView {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.source, &other.source) && self.limit == other.limit
    }
}
impl Eq for ArchiveView {}

impl ArchiveView {
    /// The length of the published bytes this view reads.
    #[must_use]
    pub const fn limit(&self) -> u64 {
        self.limit
    }

    /// Whether `id` is archived in this view.
    #[must_use]
    pub fn contains(&self, id: u64) -> bool {
        self.offset(id).is_some()
    }

    /// The archived identifiers whose recorded father or mother is `parent`, in the order archived.
    #[must_use]
    pub fn children_of(&self, parent: u64) -> Vec<u64> {
        let Ok(index) = self.source.index.read() else {
            return Vec::new();
        };
        // The offsets of a parent's list only grow, so what lies before the limit is a prefix: the later records are never visited.
        index.by_parent.get(&parent).map_or_else(Vec::new, |list| {
            let end = list.partition_point(|(_, offset)| *offset < self.limit);
            list[..end].iter().map(|(id, _)| *id).collect()
        })
    }

    /// The archived record of `id`, read from disk, or `None` when this view does not hold it.
    ///
    /// # Errors
    /// Returns an I/O or decoding error.
    pub fn get(&self, id: u64) -> std::io::Result<Option<Character>> {
        use std::io::{Read, Seek, SeekFrom};
        let Some(offset) = self.offset(id) else {
            return Ok(None);
        };
        let mut file = std::fs::File::open(&self.source.path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut length = [0; 4];
        file.read_exact(&mut length)?;
        let bytes = read_record(&mut file, u32::from_le_bytes(length))?;
        Character::from_bytes(&bytes)
            .map(Some)
            .map_err(std::io::Error::other)
    }

    fn offset(&self, id: u64) -> Option<u64> {
        let index = self.source.index.read().ok()?;
        index
            .by_id
            .get(&id)
            .copied()
            .filter(|offset| *offset < self.limit)
    }
}
