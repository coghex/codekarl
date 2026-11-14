//! What identifies a game in a save: the build id, the scenario and the content hashes; and the quicksave file.

use std::{
    fs, io,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use ck_sim::{Date, Expected, SaveHeader, SaveMeta};

/// The quicksave's file name inside the saves directory.
pub(crate) const QUICKSAVE: &str = "quicksave.cksave";
static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fit {
    Unreadable,
    Version,
    Content,
    Fits,
}

impl Fit {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unreadable => "unreadable",
            Self::Version => "version",
            Self::Content => "content",
            Self::Fits => "fits",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) filename: String,
    pub(crate) name: String,
    pub(crate) written: Option<String>,
    pub(crate) date: Option<String>,
    pub(crate) this_build: Option<bool>,
    pub(crate) fit: Fit,
}

pub(crate) fn valid_name(name: &str) -> Result<String, String> {
    if name.chars().any(char::is_control) {
        return Err("name must not contain control characters".to_owned());
    }
    let trimmed = name.trim_matches(' ');
    let length = trimmed.chars().count();
    if !(1..=48).contains(&length) {
        return Err("name must be 1 to 48 characters and contain no control characters".to_owned());
    }
    Ok(trimmed.to_owned())
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut separator = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !out.is_empty() {
                out.push('-');
            }
            separator = false;
            out.push(character.to_ascii_lowercase());
        } else {
            separator = true;
        }
    }
    if out.len() > 48 {
        out.truncate(48);
    }
    out.trim_matches('-').to_owned()
}

fn entry_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub(crate) fn new_save_path(saves: &Path, name: &str) -> Result<(String, PathBuf), String> {
    let name = valid_name(name)?;
    fs::create_dir_all(saves).map_err(|error| format!("cannot create saves directory: {error}"))?;
    let base = match slug(&name).as_str() {
        "" => "save".to_owned(),
        value => value.to_owned(),
    };
    let first = if base == "quicksave" { 2 } else { 1 };
    for number in first..=u32::MAX {
        let filename = if number == 1 {
            format!("{base}.cksave")
        } else {
            format!("{base}-{number}.cksave")
        };
        let path = saves.join(&filename);
        if !entry_exists(&path).map_err(|error| format!("cannot inspect save path: {error}"))? {
            return Ok((filename, path));
        }
    }
    Err("no free save file name remains".to_owned())
}

pub(crate) fn valid_filename(filename: &str) -> Result<(), String> {
    if filename.is_empty()
        || filename == "."
        || filename == ".."
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains('\0')
        || !filename.ends_with(".cksave")
        || filename == ".cksave"
    {
        return Err("not a save file name".to_owned());
    }
    Ok(())
}

pub(crate) fn regular_save(saves: &Path, filename: &str) -> Result<PathBuf, String> {
    valid_filename(filename)?;
    let path = saves.join(filename);
    let metadata = fs::symlink_metadata(&path).map_err(|_| "not a save".to_owned())?;
    if !metadata.file_type().is_file() {
        return Err("not a regular save".to_owned());
    }
    Ok(path)
}

pub(crate) fn open_regular(path: &Path) -> io::Result<fs::File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(file)
}

pub(crate) fn read_header(path: &Path) -> Result<SaveHeader, String> {
    let mut file = open_regular(path).map_err(|error| error.to_string())?;
    // The parser itself reads only the 12-byte prefix and the bounded header.
    SaveHeader::read_from(&mut file).map_err(|error| error.to_string())
}

pub(crate) fn list(saves: &Path, identity: &Identity) -> Result<Vec<Entry>, String> {
    let directory = match fs::read_dir(saves) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot list saves: {error}")),
    };
    let mut entries = Vec::new();
    for item in directory {
        let item = item.map_err(|error| format!("cannot list saves: {error}"))?;
        let filename = match item.file_name().into_string() {
            Ok(filename) if valid_filename(&filename).is_ok() => filename,
            _ => continue,
        };
        let path = item.path();
        if !fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_file()) {
            continue;
        }
        let header = read_header(&path).ok();
        let fit = header.as_ref().map_or(Fit::Unreadable, |header| {
            let (Some(format), Some(build)) = (header.format, header.build.as_ref()) else {
                return Fit::Unreadable;
            };
            if format != 2 || build != &identity.build {
                return Fit::Version;
            }
            let expected = [
                &identity.scenario_hash,
                &identity.map_hash,
                &identity.scripts_hash,
                &identity.rules_hash,
            ];
            if header
                .content_hashes
                .iter()
                .zip(expected)
                .any(|(found, wanted)| found.as_ref() != Some(wanted))
            {
                Fit::Content
            } else {
                Fit::Fits
            }
        });
        let this_build = header
            .as_ref()
            .and_then(|header| header.build.as_ref())
            .map(|build| build == &identity.build);
        let name = header
            .as_ref()
            .and_then(|header| header.name.clone())
            .unwrap_or_else(|| filename.trim_end_matches(".cksave").to_owned());
        entries.push(Entry {
            filename,
            name,
            written: header.as_ref().and_then(|header| header.written.clone()),
            date: header.as_ref().and_then(|header| header.date.clone()),
            this_build,
            fit,
        });
    }
    entries.sort_by(|left, right| left.filename.cmp(&right.filename));
    Ok(entries)
}

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
    /// Shared economy and terrain rules hash.
    pub(crate) rules_hash: String,
    /// The scenario's start date.
    pub(crate) start: Date,
}

impl Identity {
    /// The save header for a game under `seed`.
    pub(crate) fn meta(&self, seed: u64) -> SaveMeta {
        SaveMeta {
            name: "quicksave".to_owned(),
            written: utc_now_rfc3339(),
            build: self.build.clone(),
            scenario: self.scenario.clone(),
            scenario_hash: self.scenario_hash.clone(),
            map_hash: self.map_hash.clone(),
            scripts_hash: self.scripts_hash.clone(),
            rules_hash: self.rules_hash.clone(),
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
            rules_hash: self.rules_hash.clone(),
        }
    }
}

pub(crate) fn utc_now_rfc3339() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
    let second_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_unix_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        second_of_day / 3600,
        second_of_day / 60 % 60,
        second_of_day % 60
    )
}

fn civil_from_unix_days(days: i64) -> (i64, u8, u8) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (
        year,
        u8::try_from(month).expect("Gregorian month"),
        u8::try_from(day).expect("day"),
    )
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
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Writes a complete new file, then links it into place without replacing any entry already there.
pub(crate) fn write_new_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(directory)?;
    let (temporary, mut file) = create_temporary(directory, path.file_name().unwrap_or_default())?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::hard_link(&temporary, path)?;
        let _ = fs::remove_file(&temporary);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn create_temporary(directory: &Path, name: &std::ffi::OsStr) -> io::Result<(PathBuf, fs::File)> {
    for _ in 0..128 {
        let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = name.to_os_string();
        temporary_name.push(format!(".tmp-{}-{id}", std::process::id()));
        let temporary = directory.join(temporary_name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a temporary save file",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_bounded_and_collisions_include_directories_and_reserved_quicksave() {
        let directory = crate::testing::Dir::new();
        let saves = directory.0.join("saves");
        let (empty_slug_name, empty_slug) = new_save_path(&saves, "!!!").unwrap();
        assert_eq!(empty_slug_name, "save.cksave");
        fs::create_dir(&empty_slug).unwrap();

        let (collision_name, _) = new_save_path(&saves, "!!!").unwrap();
        assert_eq!(collision_name, "save-2.cksave");
        let (reserved_name, _) = new_save_path(&saves, "quicksave").unwrap();
        assert_eq!(reserved_name, "quicksave-2.cksave");

        let long = "a".repeat(48);
        let (long_name, path) = new_save_path(&saves, &long).unwrap();
        assert_eq!(long_name, format!("{long}.cksave"));
        fs::write(path, b"occupied").unwrap();
        let (collision, _) = new_save_path(&saves, &long).unwrap();
        assert_eq!(collision, format!("{long}-2.cksave"));
    }

    #[test]
    fn display_names_are_trimmed_and_reject_empty_long_or_control_text() {
        assert_eq!(valid_name("  Éire  ").unwrap(), "Éire");
        let long = "a".repeat(49);
        for invalid in [" ", long.as_str(), "line\nbreak", "\nvalid"] {
            assert!(valid_name(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn publishing_a_new_save_never_replaces_an_existing_entry() {
        let directory = crate::testing::Dir::new();
        let path = directory.0.join("existing.cksave");
        fs::write(&path, b"first save").unwrap();
        assert!(write_new_atomically(&path, b"replacement").is_err());
        assert_eq!(fs::read(path).unwrap(), b"first save");
    }

    #[test]
    fn utc_written_time_and_gregorian_day_conversion_are_canonical() {
        assert_eq!(civil_from_unix_days(0), (1970, 1, 1));
        let timestamp = utc_now_rfc3339();
        assert_eq!(timestamp.len(), 20);
        assert!(timestamp.ends_with('Z'));
        assert!(
            timestamp.as_bytes()[..19]
                .iter()
                .all(|byte| byte.is_ascii_digit() || matches!(*byte, b'-' | b'T' | b':'))
        );
    }
}
