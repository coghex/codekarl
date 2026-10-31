//! Build script for `ck-window-glfw`: finds GLFW and, on macOS, compiles the test support.
//!
//! **The one rule for finding GLFW.** The library is linked from the Homebrew keg that `tools/bootstrap`
//! installs and `tools/ck doctor` checks: `$HOMEBREW_PREFIX/opt/glfw` (`/opt/homebrew/opt/glfw` when
//! `HOMEBREW_PREFIX` is not set). It is an error, naming the install command, when the keg has no
//! `libglfw`; nothing is searched for elsewhere and no global shell setting is changed. The keg's `opt`
//! path is stable across `brew upgrade`, which is also the install name of the library.
//!
//! On macOS the library also carries a small Objective-C source (`native/activation.m`: the regular
//! activation policy and the request to come to the front), compiled into its own archive that the library
//! links (`rustc-link-lib`), with the same compiler flags, deployment target and analyzer rule as the test
//! support. The two archives are separate.
//!
//! The test support (`tests/native/`) is compiled into a **separate** archive that only the integration
//! test executable links (`rustc-link-arg-tests`), so the library and everything that depends on it carry
//! none of it. The same archive's compile and the optional Clang static analyzer follow
//! `ck-window-cocoa`'s build script (one deployment-target rule, `CK_NATIVE_ANALYZE`).
use std::{
    env,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Command,
};

/// The install command named when GLFW is missing.
const INSTALL: &str = "tools/bootstrap";
/// The library's native source and its archive, linked into the library (and so into whatever depends on it).
const ACTIVATION: (&[&str], &str) = (&["native/activation.m"], "libck_window_glfw_activation.a");
/// The test support's native source and its archive, linked into the integration test only.
const TESTING: (&[&str], &str) = (&["tests/native/testing.m"], "libck_window_glfw_testing.a");

fn execute(program: &str, arguments: &[&OsStr]) {
    // A plain stdout line (not a directive): the build script's `output` file records each invocation.
    let shown: Vec<_> = arguments.iter().map(|a| a.to_string_lossy()).collect();
    println!("ck-window-glfw: {program} {}", shown.join(" "));
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
    println!("ck-window-glfw: {program} {}", shown.join(" "));
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
/// the variable is present, otherwise the value rustc itself uses for this target. A present but empty,
/// non-Unicode or unparseable override is an error, not a reason to fall back.
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
        // One argument vector for the real compile and the analysis, so they cannot drift apart.
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

/// The Homebrew keg GLFW is linked from (see the module documentation).
fn glfw_keg() -> PathBuf {
    let prefix = env::var_os("HOMEBREW_PREFIX").unwrap_or_else(|| OsString::from("/opt/homebrew"));
    Path::new(&prefix).join("opt/glfw")
}

fn main() {
    println!("cargo::rerun-if-env-changed=HOMEBREW_PREFIX");
    let keg = glfw_keg();
    let library = keg.join("lib");
    assert!(
        library.join("libglfw.dylib").exists(),
        "GLFW is not installed ({} has no libglfw.dylib); install it with: {INSTALL}",
        keg.display()
    );
    println!(
        "cargo::rerun-if-changed={}",
        library.join("libglfw.dylib").display()
    );
    println!("cargo::rustc-link-search=native={}", library.display());
    println!("cargo::rustc-link-lib=dylib=glfw");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    for input in [
        "native/activation.h",
        "native/activation.m",
        "tests/native/testing.h",
        "tests/native/testing.m",
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
    ];
    let analyzing = analyzer_requested();
    let activation = build_archive(ACTIVATION.0, ACTIVATION.1, &out, &common, analyzing);
    println!("cargo::rustc-link-search=native={}", out.display());
    println!("cargo::rustc-link-lib=static=ck_window_glfw_activation");
    println!("cargo::rustc-link-lib=framework=AppKit");
    println!(
        "ck-window-glfw: {} is linked into the library",
        activation.display()
    );
    let testing = build_archive(TESTING.0, TESTING.1, &out, &common, analyzing);
    println!("ck-window-glfw: deployment target {target}");
    // The test support's archive is handed to the integration test's link only, so it is never part of the
    // library or of anything that depends on it. The test also checks the library's rlib for its symbols.
    println!("cargo::rustc-link-arg-tests={}", testing.display());
    for framework in ["AppKit", "CoreGraphics"] {
        println!("cargo::rustc-link-arg-tests=-framework");
        println!("cargo::rustc-link-arg-tests={framework}");
    }
}
