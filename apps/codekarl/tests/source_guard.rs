//! The source guard (design §3): no source of the application names a GPU crate, except the backend modules.
//!
//! The application reaches a GPU only through `ck-render-2d` (`Renderer`, `MetalBackend`, `VulkanSinkSide`) and the
//! two modules that choose and build a backend: `src/native.rs`, the `native-metal` path, and `src/vulkan.rs`
//! with `src/vulkan/`, the `glfw-vulkan` path, and their own tests. Everything else (the configuration, the
//! Lua APIs, the windowed run, the runtime wiring, the tests of the application as a whole) works with
//! `ck-window`'s and `ck-render-2d`'s types and never names `ck_gpu_metal` or `ck_gpu_vulkan`, so no GPU
//! detail can leak into the Lua surface or the run logic. The test reads the application's sources and tests.

use std::fs;
use std::path::{Path, PathBuf};

/// What no file outside the backend modules may contain: the names of the two GPU crates, as a Rust path
/// segment, a crate name in a manifest, or a prose mention.
fn forbidden() -> [String; 4] {
    ["metal", "vulkan"]
        .map(|api| format!("ck_gpu_{api}"))
        .into_iter()
        .chain(["metal", "vulkan"].map(|api| format!("ck-gpu-{api}")))
        .collect::<Vec<_>>()
        .try_into()
        .expect("four names")
}

/// The files that may name a GPU crate: paths relative to the application's directory.
fn allowed(relative: &str) -> bool {
    relative == "src/native.rs"
        || relative == "src/vulkan.rs"
        || relative.starts_with("src/vulkan/")
        // The Vulkan path's own end-to-end tests, and the shared child supervisor they use.
        || relative == "tests/vulkan_app.rs"
        // This file names the crates it forbids.
        || relative == "tests/source_guard.rs"
}

fn collect(directory: &Path, found: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()));
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            collect(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// The application's Rust files outside the backend modules that name a GPU crate: `(file, name)` pairs.
fn offenders(root: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    for directory in ["src", "tests"] {
        collect(&root.join(directory), &mut files);
    }
    let names = forbidden();
    let mut found = Vec::new();
    for path in files {
        let relative = path
            .strip_prefix(root)
            .expect("under the application's directory")
            .to_string_lossy()
            .replace('\\', "/");
        if allowed(&relative) {
            continue;
        }
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        for name in &names {
            if text.contains(name.as_str()) {
                found.push((relative.clone(), name.clone()));
            }
        }
    }
    found
}

#[test]
fn no_source_names_a_gpu_crate_except_the_backend_modules() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let found = offenders(root);
    assert!(
        found.is_empty(),
        "only src/native.rs, src/vulkan.rs, src/vulkan/ and their own tests may name a GPU crate: {found:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// CK-132: every process and anonymous-pipe creation is covered by the executable's spawn guard
// ---------------------------------------------------------------------------------------------

const SPAWN_GUARD_FILES: [&str; 7] = [
    "src/boundary.rs",
    "src/signals.rs",
    "src/vulkan/tests.rs",
    "tests/supervisor/mod.rs",
    "tests/common/mod.rs",
    "tests/runtime.rs",
    "tests/lua_vms.rs",
];

#[derive(Debug, PartialEq, Eq)]
struct RawCall {
    file: String,
    line: usize,
    function: String,
    call: &'static str,
}

fn count_empty_calls(line: &str, name: &str) -> usize {
    let mut rest = line;
    let mut count = 0;
    while let Some(start) = rest.find(name) {
        let after_name = &rest[start + name.len()..];
        let after_space = after_name.trim_start();
        if let Some(after_open) = after_space.strip_prefix('(')
            && after_open.trim_start().starts_with(')')
        {
            count += 1;
        }
        rest = after_name;
    }
    count
}

fn raw_calls(text: &str, file: &str) -> Vec<RawCall> {
    let lines: Vec<&str> = text.lines().map(code).collect();
    let mut functions = vec![String::new(); lines.len()];
    let mut current = String::new();
    for (index, line) in lines.iter().enumerate() {
        if let Some(start) = line.find("fn ") {
            let declaration = &line[start + 3..];
            let end = declaration
                .find(|character: char| !(character.is_alphanumeric() || character == '_'))
                .unwrap_or(declaration.len());
            declaration[..end].clone_into(&mut current);
        }
        functions[index].clone_from(&current);
    }

    let mut declarations = std::collections::HashMap::<String, String>::new();
    let mut current_signature = String::new();
    let mut signature_name = String::new();
    let mut collecting = false;
    for line in &lines {
        if let Some(start) = line.find("fn ") {
            let declaration = &line[start + 3..];
            let end = declaration
                .find(|character: char| !(character.is_alphanumeric() || character == '_'))
                .unwrap_or(declaration.len());
            declaration[..end].clone_into(&mut signature_name);
            line[start..].clone_into(&mut current_signature);
            collecting = !current_signature.contains('{');
            if !collecting {
                declarations.insert(signature_name.clone(), current_signature.clone());
            }
        } else if collecting {
            current_signature.push_str(line);
            current_signature.push('\n');
            if line.contains('{') {
                collecting = false;
                declarations.insert(signature_name.clone(), current_signature.clone());
            }
        }
    }

    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let function = &functions[index];
        for (call, token) in [
            ("spawn", ".spawn"),
            ("output", ".output"),
            ("status", ".status"),
            ("io_pipe", "io::pipe"),
            ("libc_pipe", "libc::pipe"),
        ] {
            for _ in 0..count_empty_calls(line, token) {
                found.push(RawCall {
                    file: file.to_owned(),
                    line: index + 1,
                    function: function.clone(),
                    call,
                });
            }
        }
    }

    let mut calls_by_function = std::collections::HashMap::<(String, String), Vec<&str>>::new();
    for call in &found {
        calls_by_function
            .entry((call.file.clone(), call.function.clone()))
            .or_default()
            .push(call.call);
    }
    for call in &found {
        let is_helper_file = matches!(
            call.file.as_str(),
            "tests/supervisor/mod.rs" | "tests/common/mod.rs"
        );
        let helper_signature = declarations
            .get(&call.function)
            .is_some_and(|signature| signature.contains("&SpawnGuard"));
        let expected = match (call.function.as_str(), call.call) {
            ("spawn_locked", "spawn") if is_helper_file && helper_signature => Some("spawn"),
            ("pipe_locked", "io_pipe") if is_helper_file && helper_signature => Some("io_pipe"),
            ("spawn_bypassing_the_guard_for_the_negative_control", "spawn")
                if call.file == "tests/runtime.rs" =>
            {
                Some("spawn")
            }
            _ => None,
        };
        let group = calls_by_function
            .get(&(call.file.clone(), call.function.clone()))
            .expect("the raw call has a function group");
        if let Some(expected) = expected {
            if group.len() != 1 || group[0] != expected {
                return found;
            }
        } else {
            return found;
        }
    }
    Vec::new()
}

fn spawn_guard_offences(root: &Path) -> Vec<RawCall> {
    let mut found = Vec::new();
    for relative in SPAWN_GUARD_FILES {
        let path = root.join(relative);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        found.extend(raw_calls(&text, relative));
    }
    found
}

#[test]
fn all_process_and_pipe_creation_sites_are_guarded() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let found = spawn_guard_offences(root);
    assert!(
        found.is_empty(),
        "raw process or pipe creation outside guarded helpers: {found:#?}"
    );
}

#[test]
fn spawn_guard_scan_accepts_only_the_guarded_helpers_and_named_control() {
    let supervisor = r"
fn spawn_locked(_held: &SpawnGuard, command: &mut Command) { command.spawn() }
fn pipe_locked(_held: &SpawnGuard) { io::pipe() }
";
    let found = raw_calls(supervisor, "tests/supervisor/mod.rs");
    assert!(found.is_empty(), "{found:?}");
    let common = r"
fn spawn_locked(_held: &SpawnGuard, command: &mut Command) { command.spawn() }
fn pipe_locked(_held: &SpawnGuard) { io::pipe() }
";
    assert!(raw_calls(common, "tests/common/mod.rs").is_empty());
    let exception = r"
fn spawn_bypassing_the_guard_for_the_negative_control(command: &mut Command) {
    command.spawn();
}
";
    assert!(raw_calls(exception, "tests/runtime.rs").is_empty());
}

#[test]
fn spawn_guard_scan_rejects_expired_guards_later_spawns_and_thread_closures() {
    for text in [
        "fn case() { let guard = spawn_guard(); drop(guard); command.spawn(); }",
        "fn case() { { let _guard = spawn_guard(); } command.spawn(); }",
        "fn case() { thread::spawn(|| command.spawn()); }",
    ] {
        assert!(
            !raw_calls(text, "tests/runtime.rs").is_empty(),
            "accepted {text:?}"
        );
    }
}

#[test]
fn spawn_guard_scan_rejects_unguarded_pipes_misplaced_helpers_and_invalid_exceptions() {
    assert!(!raw_calls("fn case() { io::pipe(); }", "tests/common/mod.rs").is_empty());
    let misplaced =
        "fn spawn_locked(_held: &SpawnGuard, command: &mut Command) { command.spawn() }";
    assert!(!raw_calls(misplaced, "tests/runtime.rs").is_empty());
    assert!(
        !raw_calls(
            "fn spawn_bypassing_the_guard_for_the_negative_control() { command.spawn(); }",
            "tests/common/mod.rs"
        )
        .is_empty()
    );
    assert!(!raw_calls(
        "fn spawn_bypassing_the_guard_for_the_negative_control() { command.spawn(); command.spawn(); }",
        "tests/runtime.rs"
    ).is_empty());
}

#[test]
fn the_guard_sees_the_backend_modules_and_catches_a_violation() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    // The backend modules do name their crate (so the guard is looking at the right files).
    for module in ["src/native.rs", "src/vulkan/real.rs"] {
        let text = fs::read_to_string(root.join(module)).expect("a backend module");
        assert!(
            forbidden().iter().any(|name| text.contains(name.as_str())),
            "{module} should name its GPU crate"
        );
    }
    // A copy of the tree with one violation added is caught, in `src` and in `tests`.
    let scratch =
        std::env::temp_dir().join(format!("codekarl-source-guard-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    for directory in ["src", "tests"] {
        fs::create_dir_all(scratch.join(directory)).expect("a directory");
    }
    fs::write(
        scratch.join("src/window_api.rs"),
        "use ck_gpu_metal::Device;\n",
    )
    .expect("a file");
    fs::write(scratch.join("tests/modes.rs"), "// uses ck-gpu-vulkan\n").expect("a file");
    fs::write(scratch.join("src/native.rs"), "use ck_gpu_metal::Device;\n").expect("a file");
    fs::write(scratch.join("src/main.rs"), "fn main() {}\n").expect("a file");
    let mut found = offenders(&scratch);
    found.sort();
    let _ = fs::remove_dir_all(&scratch);
    assert_eq!(
        found,
        [
            ("src/window_api.rs".to_owned(), "ck_gpu_metal".to_owned()),
            ("tests/modes.rs".to_owned(), "ck-gpu-vulkan".to_owned()),
        ]
    );
}

// ---------------------------------------------------------------------------------------------
// CK-131: the shipped program has no keyboard filter, no switch for one, and only `main.rs` reads the arguments
// ---------------------------------------------------------------------------------------------

/// The code of a Rust source line: the line without its `//` comment (a doc comment included). Crude, and enough
/// for the application's sources, which have no `//` inside a string literal that this guard looks at.
fn code(line: &str) -> &str {
    line.split("//").next().unwrap_or_default()
}

/// A name's words: split at every character that is not a letter or digit and at every lower-to-upper change, in
/// lower case (`ignore_keys`, `--ignore-keys`, `IGNORE_KEYS`, `ignoreKeys` and `IgnoreKeys` all give `ignore`, `keys`).
fn words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for character in name.chars() {
        if !character.is_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            previous_lower = false;
            continue;
        }
        if character.is_uppercase() && previous_lower && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        previous_lower = character.is_lowercase() || character.is_ascii_digit();
        current.extend(character.to_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Words that turn something off, and what they would turn off. A flag, setting, identifier or environment variable
/// whose name has a verb followed at once by an object (`ignore_keys`, `--disable-input`, `NoKeyboard`) is a
/// switch for keyboard input: the shipped program has none, and the owner's demonstration receives normal input.
const OFF: [&str; 14] = [
    "ignore", "disable", "drop", "discard", "swallow", "mute", "block", "suppress", "filter",
    "deny", "no", "without", "skip", "unhook",
];
const INPUT: [&str; 7] = [
    "key",
    "keys",
    "keyboard",
    "input",
    "inputs",
    "keypress",
    "keypresses",
];

/// Whether `name` (an identifier or the text of a literal) is a keyboard-off switch by its words.
fn is_input_switch(name: &str) -> bool {
    let words = words(name);
    words
        .windows(2)
        .any(|pair| OFF.contains(&pair[0].as_str()) && INPUT.contains(&pair[1].as_str()))
}

/// The identifiers of a line of code.
fn identifiers(code: &str) -> impl Iterator<Item = &str> {
    code.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
}

/// The string literals of a text (without escapes handled: names of flags and settings have none).
fn literals(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else {
            break;
        };
        found.push(&after[..close]);
        rest = &after[close + 1..];
    }
    found
}

/// What under `src/` names the test-only keyboard filter, reads the process arguments outside `main.rs`, or names
/// a flag, setting, identifier or environment variable that could turn keyboard input off (in code, across line
/// breaks): `(file, line, rule)`. Comments are not code; a name like `key_bindings`, `handle_key_down` or `NO_COLOR`
/// is not a switch.
fn keyboard_offences(root: &Path) -> Vec<(String, usize, &'static str)> {
    let mut files = Vec::new();
    collect(&root.join("src"), &mut files);
    let mut found = Vec::new();
    for path in files {
        let relative = path
            .strip_prefix(root)
            .expect("under the application's directory")
            .to_string_lossy()
            .replace('\\', "/");
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        let lines: Vec<&str> = text.lines().map(code).collect();
        for (number, line) in lines.iter().enumerate() {
            let lower = line.to_ascii_lowercase();
            if ["ck_app_t_", "typist", "input_filter"]
                .iter()
                .any(|word| lower.contains(word))
            {
                found.push((relative.clone(), number + 1, "names the keyboard filter"));
            }
            if relative != "src/main.rs" && (line.contains("args_os") || line.contains("env::args"))
            {
                found.push((
                    relative.clone(),
                    number + 1,
                    "reads the process arguments outside src/main.rs",
                ));
            }
            // A flag or a setting written as a literal, or any identifier (a field, a variant, a function).
            if literals(line).into_iter().any(is_input_switch)
                || identifiers(line).any(is_input_switch)
            {
                found.push((
                    relative.clone(),
                    number + 1,
                    "names a flag, setting or identifier that turns keyboard input off",
                ));
            }
            // An environment read: the call, and the name it reads, may be on different lines of the statement.
            if line.contains("env::var") || line.contains("var_os(") {
                let statement: String = lines[number..]
                    .iter()
                    .take(6)
                    .flat_map(|line| [*line, "\n"])
                    .collect::<String>()
                    .split(';')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                if literals(&statement).into_iter().any(is_input_switch) {
                    found.push((
                        relative.clone(),
                        number + 1,
                        "reads an environment variable that could turn keyboard input off",
                    ));
                }
            }
        }
    }
    found
}

#[test]
fn nothing_in_the_program_names_the_keyboard_filter_or_reads_the_arguments_outside_main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let found = keyboard_offences(root);
    assert!(found.is_empty(), "{found:?}");
    // The guard looks at the right files: `main.rs` does read the arguments, and so is the one file that may.
    let main = fs::read_to_string(root.join("src/main.rs")).expect("the thin main");
    assert!(
        main.contains("args_os"),
        "src/main.rs reads the process arguments"
    );
}

#[test]
fn the_names_of_input_switches_are_told_from_the_names_of_input_handling() {
    for switch in [
        "ignore_keys",
        "--ignore-keys",
        "IGNORE_KEYS",
        "ignoreKeys",
        "IgnoreKeys",
        "ignore_input",
        "disable_keyboard",
        "CODEKARL_DISABLE_INPUT",
        "no_keys",
        "NoKeyboard",
        "drop-key-events",
        "suppress_keypress",
    ] {
        assert!(is_input_switch(switch), "{switch} is a switch");
    }
    for handling in [
        "key_bindings",
        "KeyBindings",
        "handle_key_down",
        "keyboard_focus",
        "process_input",
        "input_bindings",
        "key_input",
        "NO_COLOR",
        "--config",
        "--headless",
        "unknown_key",
        "piano_key",
        "known_keys",
        "ignore_color",
        "filter_mode",
    ] {
        assert!(!is_input_switch(handling), "{handling} is not a switch");
    }
}

#[test]
fn the_keyboard_guard_refuses_each_stated_family_and_passes_legitimate_input_code() {
    let scratch =
        std::env::temp_dir().join(format!("codekarl-keyboard-guard-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(scratch.join("src")).expect("a directory");
    let lib = [
        // 1: the filter's own symbol.
        "extern \"C\" { fn ck_app_t_install_input_filter(); }",
        // 2: a comment may say anything.
        "// a comment may say keyboard, ignore_keys and args_os",
        // 3: the process arguments, outside main.rs.
        "fn a() { let _ = std::env::args_os(); }",
        // 4: an environment read of an input switch, on one line.
        "fn b() { let _ = std::env::var(\"CODEKARL_IGNORE_KEYS\"); }",
        // 5: legitimate: an unrelated environment variable.
        "fn c() { let _ = std::env::var_os(\"NO_COLOR\"); }",
        // 6-9: the same read across ordinary line breaks.
        "fn d() {",
        "    let _ = std::env::var(",
        "        \"CODEKARL_DISABLE_INPUT\",",
        "    );",
        // 10: legitimate: an unrelated variable across lines.
        "    let _ = std::env::var_os(",
        "        \"NO_COLOR\",",
        "    );",
        "}",
        // 14: a command-line flag.
        "const FLAG: &str = \"--ignore-keys\";",
        // 15: a configuration field.
        "struct Config { ignore_input: bool }",
        // 16: a variant, and 17: a camel-case function.
        "enum Mode { NoKeyboard }",
        "fn disableKeyboard() {}",
        // 18-20: legitimate input processing and unrelated settings.
        "struct KeyBindings { handle_key_down: bool, process_input: u8 }",
        "const CONFIG: &str = \"--config\";",
        "fn keyboard_focus() {}",
    ]
    .join("\n");
    fs::write(scratch.join("src/lib.rs"), lib + "\n").expect("a file");
    fs::write(
        scratch.join("src/main.rs"),
        "fn main() { let _ = std::env::args_os(); }\n",
    )
    .expect("a file");
    let found = keyboard_offences(&scratch);
    let _ = fs::remove_dir_all(&scratch);
    let at = |line: usize| -> Vec<&'static str> {
        found
            .iter()
            .filter(|(file, number, _)| file == "src/lib.rs" && *number == line)
            .map(|(_, _, rule)| *rule)
            .collect()
    };
    let filter = "names the keyboard filter";
    let arguments = "reads the process arguments outside src/main.rs";
    let switch = "names a flag, setting or identifier that turns keyboard input off";
    let environment = "reads an environment variable that could turn keyboard input off";
    assert_eq!(at(1), [filter]);
    assert!(at(2).is_empty(), "a comment is not code");
    assert_eq!(at(3), [arguments]);
    assert!(
        at(4).contains(&switch) && at(4).contains(&environment),
        "an environment read of an input switch, on one line: {:?}",
        at(4)
    );
    assert!(at(5).is_empty(), "NO_COLOR is not an input switch");
    assert!(
        at(6).is_empty() && at(9).is_empty(),
        "the lines around the read"
    );
    assert!(
        at(7).contains(&environment),
        "a read whose name is on the next line is refused: {:?}",
        at(7)
    );
    assert!(
        at(8).contains(&switch),
        "and so is the name's own line: {:?}",
        at(8)
    );
    assert!(
        at(10).is_empty() && at(11).is_empty() && at(12).is_empty(),
        "a multi-line NO_COLOR read passes: {:?}",
        (at(10), at(11), at(12))
    );
    assert!(at(14).contains(&switch), "a flag literal: {:?}", at(14));
    assert!(
        at(15).contains(&switch),
        "a configuration field: {:?}",
        at(15)
    );
    assert!(at(16).contains(&switch), "a variant: {:?}", at(16));
    assert!(
        at(17).contains(&switch),
        "a camel-case function: {:?}",
        at(17)
    );
    for line in [18, 19, 20] {
        assert!(
            at(line).is_empty(),
            "legitimate input code, line {line}: {:?}",
            at(line)
        );
    }
    assert!(
        found.iter().all(|(file, _, _)| file != "src/main.rs"),
        "main.rs may read the arguments"
    );
}
