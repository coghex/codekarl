//! Build script for `ck-gpu-vulkan`: compiles the quad shaders to SPIR-V with `glslc`, and on macOS links the
//! windowed tests' keyboard filter.
//!
//! **The shaders.** `shaders/quad.vert` and `shaders/quad.frag` (GLSL 450) are compiled by `glslc` into
//! `OUT_DIR`, where the library includes them with `include_bytes!`. `glslc` is found by an explicit path, never
//! by `PATH` alone (the project's rule for every external tool and library, so a build does not depend on how
//! its parent was started): in order `$VULKAN_SDK/bin` (a `LunarG` SDK, when the variable is set), `/usr/local/bin`
//! (the SDK's usual install) and `/opt/homebrew/bin` (Homebrew's `shaderc`). The first directory that has an
//! executable `glslc` wins; the order is the same as the graphics doctor's (`tools/check.py`,
//! `GLSLC_DIRECTORIES`, which a catalog test compares with the list below). A missing `glslc`, or a shader it
//! refuses, fails the build with a message that names the directories searched and the install command.
//!
//! **The keyboard filter.** The library contains no native code and no macOS-only code. The windowed tests
//! (`tests/window.rs`) must not be disturbed by the owner's typing, so they use `ck-window-glfw`'s test-only
//! filter (a local event monitor that drops every keyboard event without the tests' mark): its Objective-C source
//! is compiled here into a separate archive that only the `window` test executable links
//! (`rustc-link-arg-tests` names the test targets only, and the `gpu` test and the library carry none of it).
//! This crate does not change `ck-window-glfw`; it reads that crate's test file by path.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

/// The directories searched for `glslc`, after `$VULKAN_SDK/bin` when that variable is set, in order.
const GLSLC_DIRECTORIES: [&str; 2] = ["/usr/local/bin", "/opt/homebrew/bin"];
/// What to run to get `glslc` when none of the directories has it.
const GLSLC_INSTALL: &str = "run tools/bootstrap (it installs Homebrew's `shaderc`), or install the LunarG Vulkan SDK \
                             for macOS and set VULKAN_SDK";
/// The shaders, by file name in `shaders/`; each compiles to `<name>.spv` in `OUT_DIR`.
const SHADERS: [&str; 6] = [
    "quad.vert",
    "quad.frag",
    "scene.vert",
    "scene.frag",
    "scene_bindless.vert",
    "scene_bindless.frag",
];

/// The filter's source, in `ck-window-glfw`'s tests.
const TESTING_SOURCE: &str = "../window-glfw/tests/native/testing.m";
/// Its header, which the source includes and which declares the info structure the tests' Rust binding must
/// match: a change to it rebuilds this archive as well.
const TESTING_HEADER: &str = "../window-glfw/tests/native/testing.h";

/// The directories searched for `glslc` (`$VULKAN_SDK/bin` first, when set) and the first `glslc` in them.
fn find_glslc() -> (Vec<PathBuf>, Option<PathBuf>) {
    let mut searched = Vec::new();
    if let Some(sdk) = env::var_os("VULKAN_SDK").filter(|sdk| !sdk.is_empty()) {
        searched.push(Path::new(&sdk).join("bin"));
    }
    searched.extend(GLSLC_DIRECTORIES.iter().map(PathBuf::from));
    let found = searched
        .iter()
        .map(|directory| directory.join("glslc"))
        .find(|candidate| candidate.is_file());
    (searched, found)
}

/// What every `glslc` failure says besides its own cause: where the compiler was looked for, and how to get one.
fn where_to_look(searched: &[PathBuf]) -> String {
    let searched: Vec<String> = searched
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    format!(
        "glslc was searched for in {}; {GLSLC_INSTALL}",
        searched.join(", ")
    )
}

/// Compiles every shader to SPIR-V in `out`, naming what failed, the compiler's path and where it was looked for.
fn compile_shaders(out: &Path) {
    for shader in SHADERS {
        println!("cargo::rerun-if-changed=shaders/{shader}");
    }
    println!("cargo::rerun-if-env-changed=VULKAN_SDK");
    let (searched, found) = find_glslc();
    let Some(glslc) = found else {
        panic!(
            "glslc (the GLSL to SPIR-V compiler) was not found: {}",
            where_to_look(&searched)
        );
    };
    for shader in SHADERS {
        let output = out.join(format!("{shader}.spv"));
        println!(
            "ck-gpu-vulkan: {} shaders/{shader} -o {}",
            glslc.display(),
            output.display()
        );
        let result = Command::new(&glslc)
            .args(["--target-env=vulkan1.0", "-std=450", "-Werror"])
            .arg(format!("shaders/{shader}"))
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap_or_else(|error| {
                panic!(
                    "could not run {} on shaders/{shader}: {error}; {}",
                    glslc.display(),
                    where_to_look(&searched)
                )
            });
        assert!(
            result.status.success(),
            "{} refused shaders/{shader} ({}):\n{}{}\n{}",
            glslc.display(),
            result.status,
            String::from_utf8_lossy(&result.stderr),
            String::from_utf8_lossy(&result.stdout),
            where_to_look(&searched)
        );
    }
}

fn main() {
    compile_shaders(&PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")));
    println!("cargo::rerun-if-changed={TESTING_SOURCE}");
    println!("cargo::rerun-if-changed={TESTING_HEADER}");
    println!("cargo::rerun-if-changed=../window-cocoa/tests/native/activation_watch.inc");
    println!("cargo::rerun-if-changed=build.rs");
    for key in ["DEVELOPER_DIR", "SDKROOT", "MACOSX_DEPLOYMENT_TARGET"] {
        println!("cargo::rerun-if-env-changed={key}");
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let architecture = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        _ => panic!("unsupported macOS architecture"),
    };
    let target = env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| "26.0".to_owned());
    let object = out.join("testing.o");
    let archive = out.join("libck_gpu_vulkan_testing.a");
    run(
        "xcrun",
        &[
            "--sdk",
            "macosx",
            "clang",
            "-arch",
            architecture,
            &format!("-mmacosx-version-min={target}"),
            "-fobjc-arc",
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-c",
            TESTING_SOURCE,
            "-o",
            object.to_str().expect("a UTF-8 path"),
        ],
    );
    let _ = std::fs::remove_file(&archive);
    run(
        "xcrun",
        &[
            "ar",
            "rcs",
            archive.to_str().expect("a UTF-8 path"),
            object.to_str().expect("a UTF-8 path"),
        ],
    );
    println!("cargo::rustc-link-arg-tests={}", archive.display());
    for framework in ["AppKit", "CoreGraphics"] {
        println!("cargo::rustc-link-arg-tests=-framework");
        println!("cargo::rustc-link-arg-tests={framework}");
    }
}

fn run(program: &str, arguments: &[&str]) {
    println!("ck-gpu-vulkan: {program} {}", arguments.join(" "));
    let status = Command::new(program)
        .args(arguments)
        .status()
        .unwrap_or_else(|error| panic!("could not run {program}: {error}"));
    assert!(status.success(), "{program} failed; full Xcode is required");
}
