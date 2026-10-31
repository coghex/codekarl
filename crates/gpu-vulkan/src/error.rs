//! The crate's one error type: a message that names what failed.

use std::fmt;

/// What failed, in words: a missing loader, layer or extension is named, a Vulkan error carries its code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub(crate) String);

impl Error {
    /// The message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// An error from a Vulkan call, naming the call.
pub(crate) fn vk_error(call: &str, code: ash::vk::Result) -> Error {
    Error(format!("{call} failed: {code:?}"))
}
