//! Windowed or headless: which window backends exist, which this build can open, and how a run chooses.
//!
//! `native-metal` is available in builds with the `metal` feature on macOS; `glfw-vulkan` in builds with the
//! `vulkan` feature. A windowed run whose backend is not in [`AVAILABLE`] is a configuration error. The rules
//! are here so that adding a backend only means adding it to [`AVAILABLE`].

use std::fmt;

/// A window backend, named as in the configuration and on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Backend {
    /// The native macOS path: `AppKit` and Metal.
    NativeMetal,
    /// GLFW and Vulkan.
    GlfwVulkan,
}

impl Backend {
    /// The names, for messages.
    pub(crate) const NAMES: &'static str = "native-metal or glfw-vulkan";

    /// The backend called `name`, if there is one.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "native-metal" => Some(Self::NativeMetal),
            "glfw-vulkan" => Some(Self::GlfwVulkan),
            _ => None,
        }
    }

    /// The name used in the configuration, on the command line and in records.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::NativeMetal => "native-metal",
            Self::GlfwVulkan => "glfw-vulkan",
        }
    }
}

/// The backends this build can open: `native-metal` with the `metal` feature on macOS, `glfw-vulkan` with
/// the `vulkan` feature. The two features are independent: a build may have both, one or none.
pub(crate) const AVAILABLE: &[Backend] = &[
    #[cfg(all(feature = "metal", target_os = "macos"))]
    Backend::NativeMetal,
    #[cfg(feature = "vulkan")]
    Backend::GlfwVulkan,
];

/// How a run presents itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// No window.
    Headless,
    /// A window opened by this backend.
    Windowed(Backend),
}

/// Refuses explicit frame settings that the compatibility backend does not implement yet.
pub(crate) fn validate_settings(
    mode: Mode,
    _input_bindings_present: bool,
    frame_present: bool,
) -> Result<(), String> {
    if mode != Mode::Windowed(Backend::GlfwVulkan) {
        return Ok(());
    }
    if frame_present {
        return Err("[frame] is not supported by glfw-vulkan until milestone 5.5".to_owned());
    }
    Ok(())
}

impl fmt::Display for Mode {
    /// The text of the `window: <mode>` record: `none (headless)` or the backend name.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Headless => formatter.write_str("none (headless)"),
            Self::Windowed(backend) => formatter.write_str(backend.name()),
        }
    }
}

/// Chooses the mode of a run (the message of an error is the text of the `config` record).
///
/// - `--backend` (`requested`) needs a configuration that has a `[window]` backend (`configured`);
/// - a run is **headless** when `headless` is given or when no backend is configured (no `--config`, no
///   `[window]` table, or one without `backend`);
/// - otherwise it is windowed with the requested backend, else the configured one, which must be in
///   `available`: availability is checked only for windowed runs.
pub(crate) fn select(
    headless: bool,
    requested: Option<Backend>,
    configured: Option<Backend>,
    available: &[Backend],
) -> Result<Mode, String> {
    if requested.is_some() && configured.is_none() {
        return Err("--backend given but the configuration has no [window] backend".to_owned());
    }
    if headless {
        return Ok(Mode::Headless);
    }
    match requested.or(configured) {
        None => Ok(Mode::Headless),
        Some(backend) if available.contains(&backend) => Ok(Mode::Windowed(backend)),
        Some(backend) => Err(format!(
            "backend {} is not available in this build",
            backend.name()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: &[Backend] = &[Backend::NativeMetal, Backend::GlfwVulkan];

    #[test]
    fn backends_have_the_names_the_configuration_and_the_command_line_use() {
        for backend in [Backend::NativeMetal, Backend::GlfwVulkan] {
            assert_eq!(Backend::from_name(backend.name()), Some(backend));
        }
        for wrong in [
            "",
            "Native-Metal",
            "metal",
            "glfw",
            "glfw-vulkan ",
            "vulkan",
        ] {
            assert_eq!(Backend::from_name(wrong), None, "{wrong:?}");
        }
        assert_eq!(Backend::NAMES, "native-metal or glfw-vulkan");
    }

    #[test]
    fn each_backend_is_available_exactly_with_its_own_feature() {
        assert_eq!(
            AVAILABLE.contains(&Backend::NativeMetal),
            cfg!(all(feature = "metal", target_os = "macos"))
        );
        assert_eq!(
            AVAILABLE.contains(&Backend::GlfwVulkan),
            cfg!(feature = "vulkan"),
            "glfw-vulkan is available exactly in builds with the `vulkan` feature"
        );
        assert_eq!(
            AVAILABLE.len(),
            usize::from(AVAILABLE.contains(&Backend::NativeMetal))
                + usize::from(AVAILABLE.contains(&Backend::GlfwVulkan))
        );
    }

    #[test]
    fn the_mode_record_text() {
        assert_eq!(Mode::Headless.to_string(), "none (headless)");
        assert_eq!(
            Mode::Windowed(Backend::GlfwVulkan).to_string(),
            "glfw-vulkan"
        );
    }

    #[test]
    fn no_configured_backend_or_headless_is_headless_and_checks_no_availability() {
        assert_eq!(select(false, None, None, &[]), Ok(Mode::Headless));
        assert_eq!(
            select(true, None, Some(Backend::NativeMetal), &[]),
            Ok(Mode::Headless),
            "forced headless with a configured backend that is not available"
        );
    }

    #[test]
    fn a_windowed_run_needs_its_backend_to_be_available() {
        assert_eq!(
            select(false, None, Some(Backend::NativeMetal), BOTH),
            Ok(Mode::Windowed(Backend::NativeMetal))
        );
        assert_eq!(
            select(false, None, Some(Backend::NativeMetal), &[]),
            Err("backend native-metal is not available in this build".to_owned())
        );
        // The requested backend overrides the configured one, and is the one checked.
        assert_eq!(
            select(
                false,
                Some(Backend::GlfwVulkan),
                Some(Backend::NativeMetal),
                &[Backend::NativeMetal]
            ),
            Err("backend glfw-vulkan is not available in this build".to_owned())
        );
        assert_eq!(
            select(
                false,
                Some(Backend::NativeMetal),
                Some(Backend::GlfwVulkan),
                &[Backend::NativeMetal]
            ),
            Ok(Mode::Windowed(Backend::NativeMetal))
        );
    }

    #[test]
    fn a_requested_backend_needs_a_configured_one() {
        assert_eq!(
            select(false, Some(Backend::NativeMetal), None, BOTH),
            Err("--backend given but the configuration has no [window] backend".to_owned())
        );
    }

    #[test]
    fn glfw_vulkan_accepts_explicit_bindings_but_still_refuses_frame() {
        assert_eq!(
            validate_settings(Mode::Windowed(Backend::GlfwVulkan), true, false),
            Ok(())
        );
        assert_eq!(
            validate_settings(Mode::Windowed(Backend::GlfwVulkan), false, true),
            Err("[frame] is not supported by glfw-vulkan until milestone 5.5".to_owned())
        );
        assert_eq!(
            validate_settings(Mode::Windowed(Backend::GlfwVulkan), false, false),
            Ok(())
        );
        assert_eq!(validate_settings(Mode::Headless, true, true), Ok(()));
        assert_eq!(
            validate_settings(Mode::Windowed(Backend::NativeMetal), true, true),
            Ok(())
        );
    }
}
