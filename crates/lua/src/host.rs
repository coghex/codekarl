//! The host surface a script sees: `log.*` and `config`, plus the stdout flush captured at creation.

use ck_log::Level;
use mlua::{Function, Lua, Value};

use crate::config::{ConfigProxy, ConfigValue};

/// Writes one record in `category` through the installed global logger, with the waiting call.
///
/// With no logger installed it returns at once, before converting anything: a `__tostring` metamethod
/// is not run, so the call can neither fail the script nor have side effects. A string is used as it
/// is; any other value is converted by Lua's own `tostring` (captured when the state was created, so
/// a script that replaces the global cannot change it), which gives Lua's number formatting (`1.0`,
/// `-0.0`) and runs `__tostring` metamethods.
fn emit(level: Level, category: &str, tostring: &Function, message: Value) -> mlua::Result<()> {
    let Some(logger) = ck_log::installed() else {
        return Ok(());
    };
    let text = match message {
        Value::String(text) => text,
        other => tostring.call::<mlua::LuaString>(other)?,
    };
    logger.log(level, category, text.to_string_lossy());
    Ok(())
}

/// Installs `log` and `config` as globals of `lua`.
pub(crate) fn install(lua: &Lua, name: &str, config: &ConfigValue) -> mlua::Result<()> {
    let globals = lua.globals();
    let tostring: Function = globals.get("tostring")?;
    let log = lua.create_table()?;
    for (function, level) in [
        ("error", Level::Error),
        ("warn", Level::Warn),
        ("info", Level::Info),
        ("debug", Level::Debug),
        ("trace", Level::Trace),
    ] {
        let category = format!("lua.{name}");
        let tostring = tostring.clone();
        log.set(
            function,
            lua.create_function(move |_, message: Value| {
                emit(level, &category, &tostring, message)
            })?,
        )?;
    }
    globals.set("log", log)?;
    globals.set("config", ConfigProxy::to_lua(lua, config)?)?;
    Ok(())
}

/// Captures the standard `io.stdout` stream and its `flush` method in a closure, before any script
/// runs. The closure keeps both as upvalues, so a script that replaces `io`, or the stream's method
/// table, cannot change what it flushes.
pub(crate) fn capture_flush(lua: &Lua) -> mlua::Result<Function> {
    lua.load(
        "local stdout = io.stdout\n\
         local flush = stdout.flush\n\
         return function() return flush(stdout) end",
    )
    .set_name("=ck-lua flush")
    .eval()
}
