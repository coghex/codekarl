//! Build script for `ck-gpu-metal`: compiles the Objective-C bridge and the Metal shaders with the
//! one macOS deployment target, keeps the shader's debug information in step with Cargo's debug-info
//! setting, and optionally runs the Clang static analyzer over the bridge.
use std::{
    env,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Command,
};

fn execute(program: &str, arguments: &[&OsStr]) {
    // A plain stdout line (not a directive): the build script's `output` file records each invocation.
    let shown: Vec<_> = arguments.iter().map(|a| a.to_string_lossy()).collect();
    println!("ck-gpu-metal: {program} {}", shown.join(" "));
    let status = Command::new(program)
        .args(arguments)
        .status()
        .unwrap_or_else(|error| panic!("could not run {program}: {error}"));
    assert!(
        status.success(),
        "{program} failed; full Xcode and its Metal Toolchain are required"
    );
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
fn analyze(program: &str, arguments: &[&OsStr]) {
    let shown: Vec<_> = arguments.iter().map(|a| a.to_string_lossy()).collect();
    println!("ck-gpu-metal: {program} {}", shown.join(" "));
    let output = Command::new(program)
        .args(arguments)
        .output()
        .unwrap_or_else(|error| panic!("could not run {program}: {error}"));
    assert!(
        output.status.success(),
        "the Clang static analyzer reported findings in native/bridge.m (CK_NATIVE_ANALYZE=1):\n{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}

/// Whether the shaders carry debug information: Cargo's `DEBUG` build-script variable, which is
/// `true` for the dev, test and profiling profiles and `false` for release. Anything else is an
/// error rather than a silently chosen variant.
fn shader_debug() -> bool {
    match env::var("DEBUG").as_deref() {
        Ok("true") => true,
        Ok("false") => false,
        other => panic!("DEBUG must be true or false, not {other:?}"),
    }
}

/// The one macOS deployment target for native code, shaders and the Rust link: the environment
/// override when the variable is present, otherwise the value rustc itself uses for this target
/// (never guessed from the host or the SDK). A present but empty, non-Unicode or unparseable
/// override is an error, not a reason to fall back.
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
    // A major-only target (`11`) is valid for rustc, clang and metal; spell it `11.0` everywhere.
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

/// Compiles the Metal source `source` (under `shaders/`) to `ir` and links it to `library`. With debug
/// information the compile keeps the sources and line tables and the link keeps the sources (the
/// metallib's `SOURCES` section); without it neither is passed.
fn build_shader(version_min: &OsStr, debug: bool, source: &str, ir: &Path, library: &Path) {
    let (compile_debug, link_debug): (&[&OsStr], &[&OsStr]) = if debug {
        (
            &["-frecord-sources".as_ref(), "-gline-tables-only".as_ref()],
            &["-frecord-sources".as_ref()],
        )
    } else {
        (&[], &[])
    };
    let mut compile: Vec<&OsStr> = vec![
        "--sdk".as_ref(),
        "macosx".as_ref(),
        "metal".as_ref(),
        version_min,
        "-Werror".as_ref(),
    ];
    compile.extend(compile_debug);
    compile.extend([
        "-c".as_ref(),
        source.as_ref(),
        "-o".as_ref(),
        ir.as_os_str(),
    ]);
    execute("xcrun", &compile);
    let mut link: Vec<&OsStr> = vec![
        "--sdk".as_ref(),
        "macosx".as_ref(),
        "metal".as_ref(),
        version_min,
    ];
    link.extend(link_debug);
    link.extend([ir.as_os_str(), "-o".as_ref(), library.as_os_str()]);
    execute("xcrun", &link);
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
            analyze("xcrun", &arguments);
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
    assert_eq!(
        env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("macos"),
        "ck-gpu-metal requires macOS"
    );
    for input in [
        "native/bridge.h",
        "native/bridge.m",
        "native/presenter.h",
        "native/presenter.m",
        "native/textures.h",
        "native/textures.m",
        "tests/native/testing.m",
        "../window-cocoa/tests/native/activation_watch.inc",
        "tests/native/bindless_probe.m",
        "tests/native/bindless_probe.h",
        "shaders/probe.metal",
        "shaders/quad.metal",
        "shaders/bindless_probe.metal",
    ] {
        println!("cargo::rerun-if-changed={input}");
    }
    for key in [
        "DEVELOPER_DIR",
        "SDKROOT",
        "MACOSX_DEPLOYMENT_TARGET",
        "CK_NATIVE_ANALYZE",
        "DEBUG",
    ] {
        println!("cargo::rerun-if-env-changed={key}");
    }
    let target = deployment_target();
    let debug = shader_debug();
    // Recorded for the shader-debug test, which still reads the metallib itself.
    println!("cargo::rustc-env=CK_SHADER_DEBUG={}", u8::from(debug));
    let version_min = OsString::from(format!("-mmacosx-version-min={target}"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let architecture = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        _ => panic!("unsupported macOS architecture"),
    };
    // One argument vector for every real compile and analysis, so they cannot drift apart: the SDK,
    // architecture, S1's deployment target, ARC, language level and warnings. `-c` and `-o` differ
    // (`-c` is rejected next to `--analyze` under `-Werror`), so they follow the source.
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
    // The library: the probe's bridge and the presenter. The test support is a separate archive that only
    // the integration tests link (see below), so the library carries none of it.
    let archive = build_archive(
        &["native/bridge.m", "native/presenter.m", "native/textures.m"],
        "libck_metal.a",
        &out,
        &common,
        analyzing,
    );
    let testing = build_archive(
        &["tests/native/testing.m", "tests/native/bindless_probe.m"],
        "libck_metal_testing.a",
        &out,
        &common,
        analyzing,
    );
    println!("cargo::rustc-link-arg-tests={}", testing.display());
    // The tests check the two archives' symbols (the isolation of the test support).
    println!("cargo::rustc-env=CK_METAL_ARCHIVE={}", archive.display());
    println!(
        "cargo::rustc-env=CK_METAL_TESTING_ARCHIVE={}",
        testing.display()
    );
    println!("ck-gpu-metal: deployment target {target}");
    // `probe.metallib` is the bootstrap probe's kernel (embedded by `Device`); `quad.metallib` holds the
    // textured quad's vertex and fragment functions (embedded by the presenter).
    for shader in ["probe", "quad"] {
        build_shader(
            &version_min,
            debug,
            &format!("shaders/{shader}.metal"),
            &out.join(format!("{shader}.ir")),
            &out.join(format!("{shader}.metallib")),
        );
    }
    build_shader(
        &version_min,
        debug,
        "shaders/bindless_probe.metal",
        &out.join("bindless_probe.ir"),
        &out.join("bindless_probe.metallib"),
    );
    println!("cargo::rustc-link-search=native={}", out.display());
    println!("cargo::rustc-link-lib=static=ck_metal");
    for framework in ["Metal", "Foundation", "CoreGraphics", "QuartzCore"] {
        println!("cargo::rustc-link-lib=framework={framework}");
    }
}
