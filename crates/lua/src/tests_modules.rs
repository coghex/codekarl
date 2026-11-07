//! The rooted `require` (`Modules::Rooted`): what it loads, what it refuses, and that nothing else changed.

use std::fs;
use std::path::Path;

use super::*;
use crate::{Modules, Profile};

/// A root directory under a fresh temporary directory, with the modules `files` written beneath it.
fn rooted(files: &[(&str, &[u8])]) -> (Dir, PathBuf) {
    let dir = Dir::new();
    let root = dir.0.join("modules");
    for (name, bytes) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().expect("a parent")).expect("the module directory");
        fs::write(path, bytes).expect("the module file");
    }
    (dir, root)
}

fn run(name: &str, dir: &Dir, root: &Path, source: &str) -> Result<(), VmError> {
    let _ = buffer();
    let mut settings = VmSettings::new(name, dir.script(source), empty_config());
    settings.modules = Modules::Rooted(root.to_path_buf());
    let mut owner = VmOwner::spawn(settings).expect("the thread starts");
    let ready = owner.wait_ready(BOUND);
    if ready.is_ok() {
        owner.stop(BOUND).expect("the owner stops");
    }
    ready
}

#[test]
fn a_dotted_name_loads_the_file_and_a_directory_loads_its_init() {
    let (dir, root) = rooted(&[
        ("lib/text.lua", b"return {name = ..., kind = 'text'}"),
        ("pkg/init.lua", b"return {name = ..., kind = 'pkg'}"),
    ]);
    run(
        "modules_load",
        &dir,
        &root,
        r"
local text = require('lib.text')
assert(text.kind == 'text' and text.name == 'lib.text')
local pkg = require('pkg')
assert(pkg.kind == 'pkg' and pkg.name == 'pkg')
",
    )
    .expect("both modules load");
}

#[test]
fn a_second_require_returns_the_cached_module_and_runs_the_file_once() {
    let (dir, root) = rooted(&[("lib/once.lua", b"runs = (runs or 0) + 1\nreturn {}")]);
    run(
        "modules_cache",
        &dir,
        &root,
        r"
local first = require('lib.once')
assert(require('lib.once') == first)
assert(runs == 1)
",
    )
    .expect("the module is cached");
}

#[test]
fn the_chunk_name_is_the_path_relative_to_the_root() {
    let (dir, root) = rooted(&[(
        "lib/text.lua",
        b"return {fail = function() error('boom') end}",
    )]);
    let error = run(
        "modules_chunk_name",
        &dir,
        &root,
        "require('lib.text').fail()",
    )
    .expect_err("the module raises");
    let VmError::Script {
        message, traceback, ..
    } = error
    else {
        panic!("expected a script error, got {error:?}");
    };
    assert!(
        message.starts_with("lib/text.lua:1: boom"),
        "message: {message}"
    );
    assert!(
        traceback.contains("lib/text.lua:1"),
        "traceback: {traceback}"
    );
    assert!(
        !message.contains(root.to_str().expect("a UTF-8 path")),
        "the absolute root does not appear: {message}"
    );
}

#[test]
fn names_outside_the_pattern_are_refused_naming_the_name() {
    let (dir, root) = rooted(&[("x.lua", b"return 1"), ("a/b.lua", b"return 1")]);
    run(
        "modules_names",
        &dir,
        &root,
        r#"
for _, name in ipairs{"../x", "a/b", "A", "", "a..b", "a.", ".a", "a b"} do
  local ok, err = pcall(require, name)
  assert(not ok, "accepted " .. name)
  assert(string.find(tostring(err), "not allowed", 1, true), err)
  if name ~= "" then
    assert(string.find(tostring(err), name, 1, true), err)
  end
end
"#,
    )
    .expect("every bad name is refused with an error");
}

#[test]
fn a_symbolic_link_leading_outside_the_root_is_refused() {
    let (dir, root) = rooted(&[("inside.lua", b"return 'inside'")]);
    let outside = dir.0.join("outside.lua");
    fs::write(&outside, "return 'outside'").expect("the outside file");
    std::os::unix::fs::symlink(&outside, root.join("escape.lua")).expect("the link");
    // A link that stays inside the root is fine.
    std::os::unix::fs::symlink(root.join("inside.lua"), root.join("alias.lua")).expect("the link");
    run(
        "modules_symlink",
        &dir,
        &root,
        r"
local ok, err = pcall(require, 'escape')
assert(not ok)
assert(string.find(tostring(err), [[module 'escape']], 1, true), err)
assert(string.find(tostring(err), 'escape.lua', 1, true), err)
assert(string.find(tostring(err), 'outside the module root', 1, true), err)
assert(require('alias') == 'inside')
",
    )
    .expect("the escape is refused");
}

#[test]
fn a_binary_chunk_is_refused() {
    let (dir, root) = rooted(&[("compiled.lua", b"\x1bLua\x54\x00garbage")]);
    run(
        "modules_binary",
        &dir,
        &root,
        r"
local ok, err = pcall(require, 'compiled')
assert(not ok)
assert(string.find(tostring(err), 'binary chunk', 1, true), err)
assert(string.find(tostring(err), [['compiled']], 1, true), err)
",
    )
    .expect("the binary chunk is refused");
}

#[test]
fn no_c_module_can_be_loaded_and_the_searchers_are_exactly_two() {
    let (dir, root) = rooted(&[]);
    run(
        "modules_no_c",
        &dir,
        &root,
        r"
assert(package.loadlib == nil)
assert(package.cpath == '' and package.path == '')
assert(#package.searchers == 2)
package.preload.preloaded = function() return 'from preload' end
assert(require('preloaded') == 'from preload')
",
    )
    .expect("the module system is narrowed");
}

#[test]
fn a_missing_module_lists_the_candidates_relative_to_the_root() {
    let (dir, root) = rooted(&[]);
    run(
        "modules_missing",
        &dir,
        &root,
        r"
local ok, err = pcall(require, 'lib.missing')
err = tostring(err)
assert(not ok)
assert(string.find(tostring(err), [[module 'lib.missing' not found]], 1, true), err)
assert(string.find(tostring(err), [[no file 'lib/missing.lua']], 1, true), err)
assert(string.find(tostring(err), [[no file 'lib/missing/init.lua']], 1, true), err)
",
    )
    .expect("the candidates are listed");
}

#[test]
fn the_default_option_and_the_deterministic_profile_are_unchanged() {
    // Default: Lua's own path search still works, with its three searchers, `loadlib` and a `cpath`.
    let dir = Dir::new();
    let folder = dir.0.join("plain");
    fs::create_dir_all(&folder).expect("a folder");
    fs::write(folder.join("plain_module.lua"), "return 'plain'").expect("the module");
    let source = format!(
        "package.path = '{}/?.lua'\nassert(#package.searchers == 3 and package.loadlib ~= nil and package.cpath ~= '')\nassert(require('plain_module') == 'plain')",
        folder.display()
    );
    let _ = buffer();
    let mut owner = VmOwner::spawn(VmSettings::new(
        "modules_default",
        dir.script(&source),
        empty_config(),
    ))
    .expect("the thread starts");
    owner.wait_ready(BOUND).expect("the default require works");
    owner.stop(BOUND).expect("the owner stops");

    // Deterministic ignores the option and still has no `require`.
    let (dir, root) = rooted(&[("lib/text.lua", b"return {}")]);
    let mut settings = VmSettings::new(
        "modules_deterministic",
        dir.script("assert(require == nil and package == nil)"),
        empty_config(),
    );
    settings.profile = Profile::Deterministic;
    settings.modules = Modules::Rooted(root);
    let mut owner = VmOwner::spawn(settings).expect("the thread starts");
    owner
        .wait_ready(BOUND)
        .expect("the deterministic profile has no require");
    owner.stop(BOUND).expect("the owner stops");
}
