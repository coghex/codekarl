//! The rooted `require`: Lua text modules loaded only from beneath one directory.
//!
//! With [`Modules::Rooted`] the state's module system is narrowed before any user chunk runs:
//!
//! - `package.searchers` is exactly two searchers, `package.preload` and the rooted searcher below;
//! - `package.path` and `package.cpath` are `""`, and `package.loadlib` is removed, so no C module can be loaded;
//! - the rooted searcher accepts only module names matching `[a-z0-9_]+(\.[a-z0-9_]+)*`, tries `<root>/<path>.lua` and then
//!   `<root>/<path>/init.lua`, opens a candidate only when its canonical path lies under the canonical root, and loads it as
//!   **text only** with the chunk name `@<path relative to the root>`.

use std::path::{Path, PathBuf};

use mlua::{Function, Lua, MultiValue, Table, Value};

/// How a VM's `require` finds modules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Modules {
    /// Lua's default `require` (searching `package.path` and `package.cpath`).
    #[default]
    Default,
    /// `require` loads Lua text modules only from beneath this directory (see the module documentation). The
    /// [`Profile::Deterministic`](crate::Profile::Deterministic) profile ignores it and still has no `require`.
    Rooted(PathBuf),
}

/// Whether `name` is a module name the rooted searcher accepts: one or more `.`-separated segments of `[a-z0-9_]+`.
fn valid_module_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
}

/// Replaces the state's module system with the rooted one (see the module documentation).
pub(crate) fn install_rooted(lua: &Lua, root: &Path) -> mlua::Result<()> {
    let package: Table = lua.globals().get("package")?;
    let searchers: Table = package.get("searchers")?;
    let preload: Function = searchers.get(1)?;
    let root = root.to_path_buf();
    let rooted =
        lua.create_function(move |lua, name: mlua::LuaString| search(lua, &root, &name))?;
    let narrowed = lua.create_table()?;
    narrowed.raw_set(1, preload)?;
    narrowed.raw_set(2, rooted)?;
    package.set("searchers", narrowed)?;
    package.set("path", "")?;
    package.set("cpath", "")?;
    package.set("loadlib", Value::Nil)?;
    Ok(())
}

/// The rooted searcher: the loader and the file's path, or the message of what was tried (Lua's searcher protocol).
fn search(lua: &Lua, root: &Path, name: &mlua::LuaString) -> mlua::Result<MultiValue> {
    let name = name.to_str()?.to_owned();
    if !valid_module_name(&name) {
        return Err(mlua::Error::runtime(format!(
            "module name '{name}' is not allowed: use lowercase letters, digits and '_' in names separated by '.'"
        )));
    }
    let relative = name.replace('.', "/");
    let canonical_root = root.canonicalize().ok();
    let mut tried = Vec::new();
    for candidate in [format!("{relative}.lua"), format!("{relative}/init.lua")] {
        let path = root.join(&candidate);
        let Some(canonical) = canonical_root
            .as_ref()
            .and_then(|_| path.canonicalize().ok())
        else {
            tried.push(format!("no file '{candidate}'"));
            continue;
        };
        if !canonical_root
            .as_ref()
            .is_some_and(|root| canonical.starts_with(root))
        {
            return Err(mlua::Error::runtime(format!(
                "module '{name}': '{candidate}' leads outside the module root (to '{}')",
                canonical.display()
            )));
        }
        if !canonical.is_file() {
            tried.push(format!("no file '{candidate}'"));
            continue;
        }
        let bytes = std::fs::read(&canonical).map_err(|error| {
            mlua::Error::runtime(format!(
                "module '{name}': cannot read '{candidate}': {error}"
            ))
        })?;
        let loader = lua
            .load(bytes)
            .set_name(format!("@{candidate}"))
            .set_mode(mlua::chunk::ChunkMode::Text)
            .into_function()
            .map_err(|error| {
                mlua::Error::runtime(format!(
                    "error loading module '{name}' from file '{candidate}':\n\t{error}"
                ))
            })?;
        return Ok(MultiValue::from_iter([
            Value::Function(loader),
            Value::String(lua.create_string(canonical.to_string_lossy().as_bytes())?),
        ]));
    }
    Ok(MultiValue::from_iter([Value::String(
        lua.create_string(tried.join("\n\t"))?,
    )]))
}
