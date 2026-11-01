//! Build script for `ck-window-cocoa`: compiles the Objective-C window backend with the one macOS
//! deployment target and optionally runs the Clang static analyzer over it, as `ck-gpu-metal`'s does.
//! The test support (`tests/native/`) is compiled into a **separate** archive that only the integration
//! test executable links (`rustc-link-arg-tests`), so the library and everything that depends on it carry
//! none of it. On other targets the script does nothing: the crate is empty there.
use std::{
    env,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Command,
};

/// The library's native sources and the archive they make.
const LIBRARY: (&[&str], &str) = (&["native/window.m"], "libck_window_cocoa.a");
/// The test support's native sources and its archive, linked into the integration test only.
const TESTING: (&[&str], &str) = (&["tests/native/testing.m"], "libck_window_cocoa_testing.a");

fn execute(program: &str, arguments: &[&OsStr]) {
    // A plain stdout line (not a directive): the build script's `output` file records each invocation.
    let shown: Vec<_> = arguments.iter().map(|a| a.to_string_lossy()).collect();
    println!("ck-window-cocoa: {program} {}", shown.join(" "));
    let status = Command::new(program)
        .args(arguments)
        .status()
        .unwrap_or_else(|error| panic!("could not run {program}: {error}"));
    assert!(status.success(), "{program} failed; full Xcode is required");
}

/// Whether the Clang static analyzer runs: `CK_NATIVE_ANALYZE=1` turns it on, unset or `0` leave it
/// off, and anything else is an error rather than a silently skipped analysis.
fn analyzer_requested() -> bool {
    match env::var("CK_NATIVE_ANALYZE").as_deref() {
        Ok("1") => true,
        Err(env::VarError::NotPresent) | Ok("0") => false,
        other => panic!("CK_NATIVE_ANALYZE must be 1 or 0, not {other:?}"),
    }
}

/// Runs the analyzer and panics with its diagnostics when it reports a finding (or cannot run).
fn analyze(source: &str, program: &str, arguments: &[&OsStr]) {
    let shown: Vec<_> = arguments.iter().map(|a| a.to_string_lossy()).collect();
    println!("ck-window-cocoa: {program} {}", shown.join(" "));
    let output = Command::new(program)
        .args(arguments)
        .output()
        .unwrap_or_else(|error| panic!("could not run {program}: {error}"));
    assert!(
        output.status.success(),
        "the Clang static analyzer reported findings in {source} (CK_NATIVE_ANALYZE=1):\n{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}

/// The one macOS deployment target for native code and the Rust link: the environment override when
/// the variable is present, otherwise the value rustc itself uses for this target (never guessed from
/// the host or the SDK). A present but empty, non-Unicode or unparseable override is an error, not a
/// reason to fall back.
fn deployment_target() -> String {
    let (value, source) = match env::var("MACOSX_DEPLOYMENT_TARGET") {
        Ok(value) => (value, "MACOSX_DEPLOYMENT_TARGET"),
        Err(env::VarError::NotPresent) => (
            rustc_deployment_target(),
            "`rustc --print deployment-target`",
        ),
        Err(env::VarError::NotUnicode(value)) => {
            panic!(
                "MACOSX_DEPLOYMENT_TARGET is not valid Unicode: {}",
                value.display()
            )
        }
    };
    let parts: Vec<&str> = value.split('.').collect();
    let well_formed = (1..=3).contains(&parts.len())
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    assert!(
        well_formed,
        "cannot parse macOS deployment target {value:?} from {source}; expected MAJOR[.MINOR[.PATCH]]"
    );
    // A major-only target (`11`) is valid for rustc and clang; spell it `11.0` everywhere.
    if parts.len() == 1 {
        return format!("{value}.0");
    }
    value
}

/// What rustc uses for `$TARGET` (`MACOSX_DEPLOYMENT_TARGET=11.0` on Rust 1.91.1).
fn rustc_deployment_target() -> String {
    let rustc = env::var_os("RUSTC").expect("RUSTC");
    let target = env::var_os("TARGET").expect("TARGET");
    let output = Command::new(&rustc)
        .args(["--print", "deployment-target", "--target"])
        .arg(&target)
        .output()
        .unwrap_or_else(|error| panic!("could not run {}: {error}", rustc.display()));
    assert!(
        output.status.success(),
        "{} --print deployment-target --target {} failed",
        rustc.display(),
        target.display()
    );
    let printed = String::from_utf8_lossy(&output.stdout).into_owned();
    printed
        .trim()
        .strip_prefix("MACOSX_DEPLOYMENT_TARGET=")
        .unwrap_or_else(|| {
            panic!("unexpected `rustc --print deployment-target` output: {printed:?}")
        })
        .to_owned()
}

/// Compiles `sources` (analysing each first when asked) and archives the objects as `archive` in `out`.
fn build_archive(
    sources: &[&str],
    archive: &str,
    out: &Path,
    common: &[&OsStr],
    analyzing: bool,
) -> PathBuf {
    let mut objects = Vec::new();
    for source in sources {
        let name = Path::new(source)
            .file_stem()
            .and_then(OsStr::to_str)
            .expect("a source file name");
        let object = out.join(format!("{name}.o"));
        // One argument vector for the real compile and the analysis, so they cannot drift apart: the
        // SDK, architecture, the deployment target, ARC, language level and warnings, then the source.
        // `-c` and `-o` differ (`-c` is rejected next to `--analyze` under `-Werror`), so they follow.
        let mut shared: Vec<&OsStr> = common.to_vec();
        shared.push(source.as_ref());
        if analyzing {
            let analysis = out.join(format!("{name}.analysis"));
            let mut arguments = shared.clone();
            arguments.extend([
                "--analyze".as_ref(),
                "-Xclang".as_ref(),
                "-analyzer-werror".as_ref(),
                "-o".as_ref(),
                analysis.as_os_str(),
            ]);
            analyze(source, "xcrun", &arguments);
        }
        let mut arguments = shared;
        arguments.extend(["-c".as_ref(), "-o".as_ref(), object.as_os_str()]);
        execute("xcrun", &arguments);
        objects.push(object);
    }
    let path = out.join(archive);
    // `ar rcs` adds to an existing archive; start from nothing so no stale member can linger.
    let _ = std::fs::remove_file(&path);
    let mut archiver: Vec<&OsStr> = vec!["ar".as_ref(), "rcs".as_ref(), path.as_os_str()];
    archiver.extend(objects.iter().map(|object| object.as_os_str()));
    execute("xcrun", &archiver);
    path
}

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    for input in [
        "native/window.h",
        "native/internal.h",
        "native/window.m",
        "tests/native/testing.h",
        "tests/native/testing.m",
        "tests/native/activation_watch.inc",
    ] {
        println!("cargo::rerun-if-changed={input}");
    }
    for key in [
        "DEVELOPER_DIR",
        "SDKROOT",
        "MACOSX_DEPLOYMENT_TARGET",
        "CK_NATIVE_ANALYZE",
    ] {
        println!("cargo::rerun-if-env-changed={key}");
    }
    let target = deployment_target();
    let version_min = OsString::from(format!("-mmacosx-version-min={target}"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let architecture = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        _ => panic!("unsupported macOS architecture"),
    };
    let common: Vec<&OsStr> = vec![
        "--sdk".as_ref(),
        "macosx".as_ref(),
        "clang".as_ref(),
        "-arch".as_ref(),
        architecture.as_ref(),
        version_min.as_ref(),
        "-fobjc-arc".as_ref(),
        "-std=c11".as_ref(),
        "-Wall".as_ref(),
        "-Wextra".as_ref(),
        "-Werror".as_ref(),
        "-I".as_ref(),
        "native".as_ref(),
    ];
    let analyzing = analyzer_requested();
    let library = build_archive(LIBRARY.0, LIBRARY.1, &out, &common, analyzing);
    let testing = build_archive(TESTING.0, TESTING.1, &out, &common, analyzing);
    println!("ck-window-cocoa: deployment target {target}");
    // The library links its own archive. The test support's archive is handed to the integration test's
    // link only, so it is never part of the library or of anything that depends on it.
    println!("cargo::rustc-link-search=native={}", out.display());
    println!("cargo::rustc-link-lib=static=ck_window_cocoa");
    println!("cargo::rustc-link-arg-tests={}", testing.display());
    // The tests check the two archives' symbols (the isolation of the test support).
    println!(
        "cargo::rustc-env=CK_WINDOW_COCOA_ARCHIVE={}",
        library.display()
    );
    println!(
        "cargo::rustc-env=CK_WINDOW_COCOA_TESTING_ARCHIVE={}",
        testing.display()
    );
    for framework in ["AppKit", "QuartzCore"] {
        println!("cargo::rustc-link-lib=framework={framework}");
    }
}
