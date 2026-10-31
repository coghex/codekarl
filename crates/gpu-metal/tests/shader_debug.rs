//! The metallib this build embedded (`$OUT_DIR/probe.metallib`) carries shader debug information
//! exactly when Cargo's debug-info setting asks for it, and it targets the project's macOS 26.0.
//!
//! The build script records which variant it produced (`CK_SHADER_DEBUG`, derived from Cargo's
//! `DEBUG`); this test does not trust that value alone, it reads the artifact itself with
//! `xcrun metal-objdump`: a `SOURCES` section and `LineTablesOnly` line tables when the variant is
//! `1`, none of them when it is `0`. A missing `metal-objdump` fails the test (the Metal toolchain
//! is already mandatory); it is never a skip.

use std::process::Command;

const LIBRARY: &str = concat!(env!("OUT_DIR"), "/probe.metallib");
const VARIANT: &str = env!("CK_SHADER_DEBUG");

/// The output of `xcrun metal-objdump <arguments> <metallib>`, or why it could not be read.
fn objdump(arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("xcrun")
        .arg("metal-objdump")
        .args(arguments)
        .arg(LIBRARY)
        .output()
        .map_err(|error| format!("could not run xcrun metal-objdump: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "xcrun metal-objdump {arguments:?} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[test]
fn the_metallib_carries_debug_information_exactly_when_cargo_debug_info_is_on() -> Result<(), String>
{
    let sections = objdump(&["--section-headers"])?;
    let disassembly = objdump(&["-d"])?;
    let has_sources = sections.lines().any(|line| line.contains(" SOURCES "));
    let has_compile_unit = disassembly.contains("DICompileUnit")
        && disassembly.contains("emissionKind: LineTablesOnly");
    let has_locations = disassembly.contains("!DILocation(");
    match VARIANT {
        "1" => {
            assert!(
                has_sources,
                "no SOURCES section in the debug metallib:\n{sections}"
            );
            assert!(
                has_compile_unit,
                "no DICompileUnit with emissionKind: LineTablesOnly in the debug metallib"
            );
            assert!(has_locations, "no DILocation record in the debug metallib");
        }
        "0" => {
            assert!(
                !has_sources,
                "a SOURCES section in the no-debug metallib:\n{sections}"
            );
            assert!(
                !disassembly.contains("DICompileUnit"),
                "a DICompileUnit in the no-debug metallib"
            );
            assert!(
                !disassembly.contains("DILocation"),
                "a DILocation in the no-debug metallib"
            );
        }
        other => return Err(format!("CK_SHADER_DEBUG must be 1 or 0, not {other:?}")),
    }
    Ok(())
}

#[test]
fn the_metallib_targets_the_forced_deployment_target() -> Result<(), String> {
    let disassembly = objdump(&["-d"])?;
    let triple = disassembly
        .lines()
        .find(|line| line.starts_with("target triple"))
        .ok_or("no target triple in the metallib disassembly")?;
    assert!(
        triple.contains("-apple-macosx26.0.0"),
        "the metallib's AIR triple does not name macosx26.0.0: {triple}"
    );
    Ok(())
}
