//! The assets rule: which files the render API opens.
//!
//! [`Assets`] holds a configured root directory and decodes images by name:
//!
//! - The root is resolved at the **first** [`Assets::open`], not at construction, so a configuration that
//!   never renders needs no assets directory. The canonicalised root, or the error, is kept and reused: a
//!   missing or unusable root is an error naming it, on that call and on every later one.
//! - A name must be relative, non-empty and made of normal components only: no `..`, no `.`, no root or prefix.
//! - The joined path is canonicalised, which resolves links, and must lie beneath the canonical root. A link
//!   whose target stays beneath the root is allowed; a file or a directory on the way that leads outside is
//!   refused.
//! - The file is opened once, by that canonical path, and decoded from the opened handle. It is never
//!   reopened by name.
//!
//! **The limit, stated plainly.** This decides which files *this API* opens. It is not a sandbox: it does
//! not protect against a hostile process replacing paths inside the assets directory between the check and
//! the open (the directory is trusted local files, and race-safe confinement waits until modding needs it),
//! and it does not confine a script's own use of Lua's `io`.

use std::fmt;
use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use crate::budget::DecodeLimits;
use crate::image::{DecodeError, Image, ReserveError};

/// Why an image could not be opened by name.
#[derive(Debug)]
pub enum AssetsError {
    /// The root cannot be used.
    Root {
        /// The configured root.
        root: PathBuf,
        /// Why it cannot be used.
        reason: String,
    },
    /// The name is not allowed.
    Name {
        /// The name.
        name: String,
        /// Why not.
        reason: &'static str,
    },
    /// The name leads outside the root.
    Outside {
        /// The name.
        name: String,
        /// Where it resolved to.
        resolved: PathBuf,
    },
    /// The file could not be resolved or opened.
    Open {
        /// The name.
        name: String,
        /// The error.
        source: io::Error,
    },
    /// The file is not a regular file.
    NotAFile {
        /// The name.
        name: String,
    },
    /// The file did not decode.
    Decode {
        /// The name.
        name: String,
        /// Why.
        source: DecodeError,
    },
}

impl fmt::Display for AssetsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root { root, reason } => {
                write!(
                    formatter,
                    "the assets directory {} cannot be used: {reason}",
                    root.display()
                )
            }
            Self::Name { name, reason } => {
                write!(formatter, "image name {name:?} is not allowed: {reason}")
            }
            Self::Outside { name, resolved } => write!(
                formatter,
                "image {name:?} resolves to {}, outside the assets directory",
                resolved.display()
            ),
            Self::Open { name, source } => {
                write!(formatter, "cannot open image {name:?}: {source}")
            }
            Self::NotAFile { name } => write!(formatter, "image {name:?} is not a regular file"),
            Self::Decode { name, source } => write!(formatter, "image {name:?}: {source}"),
        }
    }
}

impl std::error::Error for AssetsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Open { source, .. } => Some(source),
            Self::Decode { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Why an image could not be opened by name with a reservation: the usual reasons ([`AssetsError`]), or the reservation
/// was refused (decoding stopped there, with nothing image-sized allocated).
#[derive(Debug)]
pub enum OpenReserveError<E> {
    /// The image could not be opened or decoded.
    Assets(AssetsError),
    /// The reservation was refused.
    Refused(E),
}

impl<E: fmt::Display> fmt::Display for OpenReserveError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Assets(error) => error.fmt(formatter),
            Self::Refused(error) => error.fmt(formatter),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for OpenReserveError<E> {}

/// The images below one root directory.
#[derive(Debug)]
pub struct Assets {
    root: PathBuf,
    /// The canonical root, or the reason it cannot be used, decided once at the first `open`.
    resolved: OnceLock<Result<PathBuf, String>>,
}

impl Assets {
    /// Assets below `root`. Nothing is looked at yet.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            resolved: OnceLock::new(),
        }
    }

    /// The configured root, as given.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn canonical_root(&self) -> Result<&Path, AssetsError> {
        let resolved = self.resolved.get_or_init(|| {
            self.root
                .canonicalize()
                .map_err(|error| error.to_string())
                .and_then(|canonical| {
                    if canonical.is_dir() {
                        Ok(canonical)
                    } else {
                        Err("it is not a directory".to_owned())
                    }
                })
        });
        match resolved {
            Ok(canonical) => Ok(canonical),
            Err(reason) => Err(AssetsError::Root {
                root: self.root.clone(),
                reason: reason.clone(),
            }),
        }
    }

    /// Checks the rules for a name alone: non-empty, no `.` or `..` segment, relative. It is pure: it looks at no root
    /// and touches no file system, so a name can be refused before anything is queued. [`open`](Self::open) calls it,
    /// so the two cannot drift.
    ///
    /// # Errors
    ///
    /// [`AssetsError::Name`], naming the reason.
    pub fn check_name(name: &str) -> Result<(), AssetsError> {
        let refuse = |reason| AssetsError::Name {
            name: name.to_owned(),
            reason,
        };
        if name.is_empty() {
            return Err(refuse("it is empty"));
        }
        // `Path::components` folds an inner `.` away, so the segments are looked at directly.
        for segment in name.split(std::path::is_separator) {
            match segment {
                "." => return Err(refuse("it has a `.`")),
                ".." => return Err(refuse("it has a `..`")),
                _ => {}
            }
        }
        for component in Path::new(name).components() {
            if matches!(component, Component::RootDir | Component::Prefix(_)) {
                return Err(refuse("it is not relative"));
            }
        }
        Ok(())
    }

    /// Resolves and opens the file called `name`: the root first (an unusable root is reported before a refused name),
    /// then the name's rules, then the canonical path beneath the root, then the regular file.
    fn open_file(&self, name: &str) -> Result<File, AssetsError> {
        let root = self.canonical_root()?;
        Self::check_name(name)?;
        let open_error = |source| AssetsError::Open {
            name: name.to_owned(),
            source,
        };
        let resolved = root.join(name).canonicalize().map_err(open_error)?;
        if !resolved.starts_with(root) {
            return Err(AssetsError::Outside {
                name: name.to_owned(),
                resolved,
            });
        }
        let file = File::open(&resolved).map_err(open_error)?;
        let metadata = file.metadata().map_err(open_error)?;
        if !metadata.is_file() {
            return Err(AssetsError::NotAFile {
                name: name.to_owned(),
            });
        }
        Ok(file)
    }

    /// Opens and decodes the image called `name`.
    ///
    /// # Errors
    ///
    /// [`AssetsError`]: the root is unusable, the name is not allowed or leads outside the root, the file
    /// cannot be opened, or it does not decode.
    pub fn open(&self, name: &str) -> Result<Image, AssetsError> {
        let file = self.open_file(name)?;
        Image::decode(file).map_err(|source| AssetsError::Decode {
            name: name.to_owned(),
            source,
        })
    }

    /// [`open`](Self::open) with a reservation: the image is decoded under `limits`, and `reserve` is asked for the
    /// decoded size (`width × height × 4` bytes) after the header checks and before the image-sized allocation (see
    /// [`Image::decode_reserving`]). The root is resolved, as for `open`, at the first call, on the calling thread.
    ///
    /// # Errors
    ///
    /// [`OpenReserveError::Assets`] as for `open`; [`OpenReserveError::Refused`] with `reserve`'s refusal.
    pub fn open_reserving<Q, E>(
        &self,
        name: &str,
        limits: &DecodeLimits,
        reserve: impl FnOnce(u64) -> Result<Q, E>,
    ) -> Result<(Image, Q), OpenReserveError<E>> {
        let file = self.open_file(name).map_err(OpenReserveError::Assets)?;
        Image::decode_reserving(file, limits, reserve).map_err(|error| match error {
            ReserveError::Decode(source) => OpenReserveError::Assets(AssetsError::Decode {
                name: name.to_owned(),
                source,
            }),
            ReserveError::Refused(refusal) => OpenReserveError::Refused(refusal),
        })
    }
}
