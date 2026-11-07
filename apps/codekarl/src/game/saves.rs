//! What identifies a game in a save: the build id, the scenario and the content hashes; and the quicksave file.

use std::{
    fs, io,
    io::Write,
    path::{Path, PathBuf},
};

use ck_sim::{Date, Expected, SaveMeta};

/// The quicksave's file name inside the saves directory.
pub(crate) const QUICKSAVE: &str = "quicksave.cksave";

/// The build id a save carries: `<CARGO_PKG_VERSION>+<h>`, where `h` is the **simulation source hash** `build.rs` computed.
///
/// Every artifact built from the same sources has the same id, whichever backends it has, in debug or release and on any
/// supported machine. A change to any source that can change a simulation result changes it, which refuses older saves.
pub(crate) fn build_id() -> String {
    format!("{}+{}", env!("CARGO_PKG_VERSION"), env!("CK_SIM_SOURCES"))
}

/// Everything about a running game that a save header records or a load must match, apart from the seed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    /// The build id.
    pub(crate) build: String,
    /// The scenario's id.
    pub(crate) scenario: String,
    /// The scenario content hash.
    pub(crate) scenario_hash: String,
    /// The map content hash.
    pub(crate) map_hash: String,
    /// The gameplay scripts content hash.
    pub(crate) scripts_hash: String,
    /// The scenario's start date.
    pub(crate) start: Date,
}

impl Identity {
    /// The save header for a game under `seed`.
    pub(crate) fn meta(&self, seed: u64) -> SaveMeta {
        SaveMeta {
            build: self.build.clone(),
            scenario: self.scenario.clone(),
            scenario_hash: self.scenario_hash.clone(),
            map_hash: self.map_hash.clone(),
            scripts_hash: self.scripts_hash.clone(),
            start: self.start,
            seed,
        }
    }

    /// What a save must match to load into this game.
    pub(crate) fn expected(&self) -> Expected {
        Expected {
            build: self.build.clone(),
            scenario_hash: self.scenario_hash.clone(),
            map_hash: self.map_hash.clone(),
            scripts_hash: self.scripts_hash.clone(),
        }
    }
}

/// The quicksave's path in `saves`.
pub(crate) fn quicksave_path(saves: &Path) -> PathBuf {
    saves.join(QUICKSAVE)
}

/// Writes `bytes` to `path` through a temporary file in the same directory and a rename, creating the directory if needed,
/// so a failed write never leaves a partial file at `path`.
///
/// # Errors
/// Any I/O failure; the temporary file is removed.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(directory)?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let written = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}
