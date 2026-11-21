//! A shared, read-only, incremental reader of the life-log file for the UI's views (CK-378 §2).
//!
//! The simulation owns the life log and appends to it; the views it publishes must read **that world's log as it was at the publication** and
//! keep reading it for as long as any view of it lives, however far the game has moved on (or been replaced by a load). A [`LogSource`] is the
//! one reader of one log file: it keeps, for each character, the offsets of that character's records, extended from where it last stopped
//! (never rescanning), and hands out [`LogView`]s, each fixed at the **published length** the file had when it was made. The log is
//! append-only, so a view's records are exactly the records whose offsets lie before that length: a record appended later is invisible to it
//! even when its date is the same or earlier, and a read decodes only the one character's own indexed records.
//!
//! **A write is complete** when its length and its whole body are on disk. The published length ends at the last complete record: a record
//! still being written (or one a failed append is about to truncate) lies beyond it and is read by no view, and the next publication reads it
//! once it is whole.
//!
//! A source also holds what keeps the log's directory alive (the game removes a storage directory when its last owner lets go), so a view of
//! an old world still reads its own files after a load has replaced them.

use std::{
    any::Any,
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{Arc, RwLock},
};

use ck_sim::Canonical;

use crate::{LogLine, read_record};

/// The offsets of each character's records, and how far the file has been read.
#[derive(Debug, Default)]
struct Index {
    /// For each character, the offsets of its records in the order written.
    by_id: BTreeMap<u64, Vec<u64>>,
    /// The length of the complete records read so far: where the next scan starts.
    scanned: u64,
}

/// The one reader of one life-log file. See the module documentation.
pub struct LogSource {
    path: PathBuf,
    index: RwLock<Index>,
    /// What keeps the log's directory alive.
    _keep: Arc<dyn Any + Send + Sync>,
}

impl std::fmt::Debug for LogSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogSource")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl LogSource {
    /// A reader of the log file at `path`. Nothing is read until [`view`](Self::view). `keep` is held for as long as the source lives:
    /// whatever owns the file's directory.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, keep: Arc<dyn Any + Send + Sync>) -> Arc<Self> {
        Arc::new(Self {
            path: path.into(),
            index: RwLock::new(Index::default()),
            _keep: keep,
        })
    }

    /// Read what has been completely written to the log since the last call and return a view fixed at the length now published. Only the
    /// new records are read.
    ///
    /// # Errors
    /// Returns an I/O or decoding error; the index is left as it was before the failing scan.
    pub fn view(self: &Arc<Self>) -> std::io::Result<LogView> {
        let mut index = self
            .index
            .write()
            .map_err(|_| std::io::Error::other("log index poisoned"))?;
        let mut file = std::fs::File::open(&self.path)?;
        let size = file.metadata()?.len();
        if size < index.scanned {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the life log is shorter than it was published",
            ));
        }
        let mut found: Vec<(u64, u64)> = Vec::new();
        let mut position = index.scanned;
        // Whatever does not hold a whole record (its length cut short, or its body not all there) is a write in progress: not published.
        while size - position >= 4 {
            file.seek(SeekFrom::Start(position))?;
            let mut length = [0; 4];
            file.read_exact(&mut length)?;
            let length = u32::from_le_bytes(length);
            if size - position - 4 < u64::from(length) {
                break;
            }
            let bytes = read_record(&mut file, length)?;
            let line = LogLine::from_bytes(&bytes).map_err(std::io::Error::other)?;
            found.push((line.id, position));
            position += 4 + u64::from(length);
        }
        for (id, offset) in found {
            index.by_id.entry(id).or_default().push(offset);
        }
        index.scanned = position;
        Ok(LogView {
            source: Arc::clone(self),
            limit: position,
        })
    }
}

/// A life log as it was when a view was published: the records whose offsets lie before `limit`.
#[derive(Clone, Debug)]
pub struct LogView {
    source: Arc<LogSource>,
    limit: u64,
}

impl PartialEq for LogView {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.source, &other.source) && self.limit == other.limit
    }
}
impl Eq for LogView {}

impl LogView {
    /// The length of the published bytes this view reads.
    #[must_use]
    pub const fn limit(&self) -> u64 {
        self.limit
    }

    /// The offsets of `id`'s records inside this view, in the order written. This is all a read of that character touches.
    #[must_use]
    pub fn offsets(&self, id: u64) -> Vec<u64> {
        let Ok(index) = self.source.index.read() else {
            return Vec::new();
        };
        // A character's offsets only grow, so what lies before the limit is a prefix: later records are never visited.
        index.by_id.get(&id).map_or_else(Vec::new, |list| {
            let end = list.partition_point(|offset| *offset < self.limit);
            list[..end].to_vec()
        })
    }

    /// The lines of `id` inside this view, in date order with the order written for equal dates. Only that character's records are read.
    ///
    /// # Errors
    /// Returns an I/O or decoding error.
    pub fn lines(&self, id: u64) -> std::io::Result<Vec<LogLine>> {
        let offsets = self.offsets(id);
        let mut file = std::fs::File::open(&self.source.path)?;
        let mut lines = Vec::with_capacity(offsets.len());
        for offset in offsets {
            file.seek(SeekFrom::Start(offset))?;
            let mut length = [0; 4];
            file.read_exact(&mut length)?;
            let bytes = read_record(&mut file, u32::from_le_bytes(length))?;
            lines.push(LogLine::from_bytes(&bytes).map_err(std::io::Error::other)?);
        }
        // A stable sort keeps the order written for equal dates.
        lines.sort_by_key(|line| line.date);
        Ok(lines)
    }
}
