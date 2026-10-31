//! The command line: which command was asked for, or why the arguments are not understood.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use crate::mode::Backend;

/// The usage line printed by `--help`.
pub(crate) const USAGE: &str = "Usage: codekarl [--config PATH] [--headless | --backend NAME] | --help | --version | --metal-smoke";

/// What the runtime command asks for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RunOptions {
    /// The configuration file, or `None` for the built-in defaults.
    pub(crate) config: Option<PathBuf>,
    /// `--headless`: no window, whatever the configuration says.
    pub(crate) headless: bool,
    /// `--backend NAME`: the window backend, overriding the configuration's.
    pub(crate) backend: Option<Backend>,
}

/// What the arguments ask for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    /// Run the runtime.
    Run(RunOptions),
    /// Print the usage line.
    Help,
    /// Print the version.
    Version,
    /// Run the Metal smoke test.
    MetalSmoke,
}

fn is(argument: &OsStr, flag: &str) -> bool {
    argument.to_str() == Some(flag)
}

/// Whether `argument` is exactly one of the option words. As the operand of an option it is not a value
/// (`./--version` and `--version.toml` are paths; the bare word is a usage error).
fn is_option_word(argument: &OsStr) -> bool {
    [
        "--config",
        "--headless",
        "--backend",
        "--help",
        "--version",
        "--metal-smoke",
    ]
    .iter()
    .any(|word| is(argument, word))
}

const UNKNOWN: &str = "unknown arguments; use --help";

/// Parses the arguments after the program name. The error is the one-line usage message that the
/// executable prints after `codekarl: `.
pub(crate) fn parse(args: &[OsString]) -> Result<Command, String> {
    for option in ["--config", "--headless", "--backend"] {
        if args.iter().filter(|argument| is(argument, option)).count() > 1 {
            return Err(format!("{option} given more than once; use --help"));
        }
    }
    let mut options = RunOptions {
        config: None,
        headless: false,
        backend: None,
    };
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        if is(argument, "--config") {
            match arguments.next() {
                None => return Err("--config needs a path; use --help".into()),
                // An option word after `--config` is another option, not a file name.
                Some(path) if is_option_word(path) => return Err(UNKNOWN.into()),
                Some(path) => options.config = Some(PathBuf::from(path)),
            }
        } else if is(argument, "--headless") {
            options.headless = true;
        } else if is(argument, "--backend") {
            let Some(name) = arguments.next().filter(|name| !is_option_word(name)) else {
                return Err(format!(
                    "--backend needs a backend name ({}); use --help",
                    Backend::NAMES
                ));
            };
            let Some(backend) = name.to_str().and_then(Backend::from_name) else {
                return Err(format!(
                    "unknown backend {}: expected {}; use --help",
                    name.to_string_lossy(),
                    Backend::NAMES
                ));
            };
            options.backend = Some(backend);
        } else if args.len() == 1 && is(argument, "--help") {
            return Ok(Command::Help);
        } else if args.len() == 1 && is(argument, "--version") {
            return Ok(Command::Version);
        } else if args.len() == 1 && is(argument, "--metal-smoke") {
            return Ok(Command::MetalSmoke);
        } else {
            return Err(UNKNOWN.into());
        }
    }
    if options.headless && options.backend.is_some() {
        return Err("--headless and --backend cannot be combined; use --help".into());
    }
    if options.backend.is_some() && options.config.is_none() {
        return Err("--backend needs --config with a [window] backend; use --help".into());
    }
    Ok(Command::Run(options))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strs(args: &[&str]) -> Result<Command, String> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        parse(&args)
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "it builds the expected `Result` of `parse`"
    )]
    fn run(
        config: Option<&str>,
        headless: bool,
        backend: Option<Backend>,
    ) -> Result<Command, String> {
        Ok(Command::Run(RunOptions {
            config: config.map(PathBuf::from),
            headless,
            backend,
        }))
    }

    fn unknown() -> Result<Command, String> {
        Err("unknown arguments; use --help".to_owned())
    }

    #[test]
    fn no_arguments_run_with_the_defaults_and_a_path_runs_with_that_file() {
        assert_eq!(parse_strs(&[]), run(None, false, None));
        assert_eq!(
            parse_strs(&["--config", "/some/where.toml"]),
            run(Some("/some/where.toml"), false, None)
        );
    }

    #[test]
    fn the_flags_that_do_not_start_the_runtime() {
        assert_eq!(parse_strs(&["--help"]), Ok(Command::Help));
        assert_eq!(parse_strs(&["--version"]), Ok(Command::Version));
        assert_eq!(parse_strs(&["--metal-smoke"]), Ok(Command::MetalSmoke));
    }

    #[test]
    fn the_usage_line_is_the_one_the_specification_expects() {
        assert_eq!(
            USAGE,
            "Usage: codekarl [--config PATH] [--headless | --backend NAME] | --help | --version | --metal-smoke"
        );
    }

    #[test]
    fn config_headless_and_backend_are_accepted_in_every_combination_and_order() {
        let metal = Some(Backend::NativeMetal);
        let vulkan = Some(Backend::GlfwVulkan);
        assert_eq!(parse_strs(&["--headless"]), run(None, true, None));
        assert_eq!(
            parse_strs(&["--config", "c.toml", "--headless"]),
            run(Some("c.toml"), true, None)
        );
        assert_eq!(
            parse_strs(&["--headless", "--config", "c.toml"]),
            run(Some("c.toml"), true, None)
        );
        assert_eq!(
            parse_strs(&["--config", "c.toml", "--backend", "native-metal"]),
            run(Some("c.toml"), false, metal)
        );
        assert_eq!(
            parse_strs(&["--backend", "glfw-vulkan", "--config", "c.toml"]),
            run(Some("c.toml"), false, vulkan)
        );
    }

    #[test]
    fn config_without_a_value_or_given_twice_has_its_own_message() {
        assert_eq!(
            parse_strs(&["--config"]),
            Err("--config needs a path; use --help".to_owned())
        );
        assert_eq!(
            parse_strs(&["--headless", "--config"]),
            Err("--config needs a path; use --help".to_owned())
        );
        for args in [
            &["--config", "a", "--config", "b"][..],
            &["--config", "a", "--config"][..],
            &["--config", "--config"][..],
        ] {
            assert_eq!(
                parse_strs(args),
                Err("--config given more than once; use --help".to_owned()),
                "{args:?}"
            );
        }
    }

    #[test]
    fn an_option_given_twice_is_its_own_error() {
        assert_eq!(
            parse_strs(&["--headless", "--headless"]),
            Err("--headless given more than once; use --help".to_owned())
        );
        assert_eq!(
            parse_strs(&[
                "--config",
                "a",
                "--backend",
                "glfw-vulkan",
                "--backend",
                "native-metal"
            ]),
            Err("--backend given more than once; use --help".to_owned())
        );
    }

    #[test]
    fn headless_and_backend_cannot_be_combined_and_backend_needs_config() {
        for args in [
            &["--config", "c", "--headless", "--backend", "native-metal"][..],
            &["--backend", "native-metal", "--config", "c", "--headless"][..],
        ] {
            assert_eq!(
                parse_strs(args),
                Err("--headless and --backend cannot be combined; use --help".to_owned()),
                "{args:?}"
            );
        }
        assert_eq!(
            parse_strs(&["--backend", "native-metal"]),
            Err("--backend needs --config with a [window] backend; use --help".to_owned())
        );
        assert_eq!(
            parse_strs(&["--backend", "glfw-vulkan", "--headless"]),
            Err("--headless and --backend cannot be combined; use --help".to_owned()),
            "the combination is reported before the missing configuration"
        );
    }

    #[test]
    fn a_backend_needs_a_known_name() {
        let missing = Err(
            "--backend needs a backend name (native-metal or glfw-vulkan); use --help".to_owned(),
        );
        assert_eq!(parse_strs(&["--backend"]), missing);
        assert_eq!(parse_strs(&["--config", "c", "--backend"]), missing);
        for word in [
            "--config",
            "--headless",
            "--help",
            "--version",
            "--metal-smoke",
        ] {
            // An option word is not a name: `--backend` has no value.
            assert_eq!(parse_strs(&["--backend", word]), missing, "{word}");
        }
        assert_eq!(
            parse_strs(&["--config", "c", "--backend", "metal"]),
            Err(
                "unknown backend metal: expected native-metal or glfw-vulkan; use --help"
                    .to_owned()
            )
        );
        assert_eq!(
            parse_strs(&["--config", "c", "--backend", ""]),
            Err("unknown backend : expected native-metal or glfw-vulkan; use --help".to_owned())
        );
    }

    #[test]
    fn everything_else_is_unknown_arguments() {
        for args in [
            &["--unknown"][..],
            &["--help", "extra"][..],
            &["--config", "x", "--version"][..],
            &["--version", "--config", "x"][..],
            &["--config", "a", "b"][..],
            &["x", "--config"][..],
            &["--metal-smoke", "--help"][..],
            &["--help", "--headless"][..],
            &["--headless", "--version"][..],
            &["--headless", "extra"][..],
            &["--metal-smoke", "--backend", "native-metal"][..],
            &[""][..],
        ] {
            assert_eq!(parse_strs(args), unknown(), "{args:?}");
        }
    }

    #[test]
    fn an_option_word_after_config_is_unknown_arguments_not_a_path() {
        for word in [
            "--help",
            "--version",
            "--metal-smoke",
            "--headless",
            "--backend",
        ] {
            assert_eq!(parse_strs(&["--config", word]), unknown(), "{word}");
        }
        // An explicit path that merely looks like an option stays a path.
        for path in [
            "./--version",
            "./--help",
            "dir/--metal-smoke",
            "./--headless",
            "dir/--backend",
            "--headless.toml",
            "--backend.toml",
            "--version.toml",
            "-x",
        ] {
            assert_eq!(
                parse_strs(&["--config", path]),
                run(Some(path), false, None),
                "{path}"
            );
        }
    }

    #[test]
    fn a_path_need_not_be_utf8() {
        use std::os::unix::ffi::OsStringExt;
        let path = OsString::from_vec(vec![b'/', 0xff, b'.', b't']);
        let args = [OsString::from("--config"), path.clone()];
        assert_eq!(
            parse(&args),
            Ok(Command::Run(RunOptions {
                config: Some(PathBuf::from(path)),
                headless: false,
                backend: None,
            }))
        );
        assert_eq!(parse(&[OsString::from_vec(vec![0xff])]), unknown());
        // A backend name that is not UTF-8 is an unknown backend, not a panic.
        let bad = OsString::from_vec(vec![b'x', 0xff]);
        assert!(
            parse(&[
                OsString::from("--config"),
                OsString::from("c"),
                OsString::from("--backend"),
                bad
            ])
            .expect_err("an unknown backend")
            .starts_with("unknown backend x")
        );
    }
}
