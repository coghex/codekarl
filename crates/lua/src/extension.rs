//! Host extensions: global tables of host functions and callback setters, installed on the owner thread
//! before the entry script runs, in list order (a VM may have several).
//!
//! An [`Extension`] exposes no `mlua` type. Each host function receives its arguments converted to
//! [`ConfigValue`] (see the crate documentation for the rules) and runs on the owner thread. A panic in
//! one is owner-fatal, also under `pcall` and `xpcall` (see `interrupt::Fatal`).

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;

use mlua::{Function, Lua, MultiValue, Value};

use crate::config::ConfigValue;
use crate::convert;
use crate::interrupt::{self, Fatal};
use crate::owner::valid_name;
use crate::unwind;

/// A host function: it receives the converted arguments (`None` for `nil`) and returns the values Lua
/// gets back, or an error text that Lua raises as an ordinary error.
pub type HostFunction =
    Arc<dyn Fn(&[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> + Send + Sync>;

/// The registered callbacks of one VM, by name. It lives on the owner thread only.
pub(crate) type Callbacks = Rc<RefCell<HashMap<String, Function>>>;

/// What a callback observer is told: `true` after the setter registered a callback, `false` after it cleared one.
pub type CallbackObserver = Arc<dyn Fn(bool) + Send + Sync>;

/// What a setter does with a valid argument.
#[derive(Clone)]
enum SetterKind {
    /// Registers or clears the callback.
    Plain,
    /// Registers or clears the callback, then tells the observer.
    Observed(CallbackObserver),
    /// Refuses with this message and changes nothing.
    Unavailable(String),
}

/// One callback setter of an extension.
#[derive(Clone)]
struct Setter {
    /// The name in the table.
    name: String,
    /// The callback it registers (unused by an unavailable setter, which registers none).
    callback: String,
    kind: SetterKind,
}

/// Names an extension table may not take: `log` and `config`, the lifecycle globals, and every global
/// the standard libraries define (a test compares this list with a fresh state's globals).
const RESERVED: &[&str] = &[
    "log",
    "config",
    "startup",
    "shutdown",
    "assert",
    "collectgarbage",
    "coroutine",
    "debug",
    "dofile",
    "error",
    "getmetatable",
    "io",
    "ipairs",
    "load",
    "loadfile",
    "math",
    "next",
    "os",
    "package",
    "pairs",
    "pcall",
    "print",
    "rawequal",
    "rawget",
    "rawlen",
    "rawset",
    "require",
    "select",
    "setmetatable",
    "string",
    "table",
    "tonumber",
    "tostring",
    "type",
    "utf8",
    "warn",
    "xpcall",
];

/// One global table of host functions and callback setters.
///
/// `table.name(...)` calls a host function; `table.setter(fn)` registers the Lua function `fn` as a
/// callback that [`post`](crate::VmOwner::post) can call by name (`table.setter(nil)` clears it).
#[derive(Clone)]
pub struct Extension {
    table: String,
    functions: Vec<(String, HostFunction)>,
    callbacks: Vec<Setter>,
}

impl fmt::Debug for Extension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Extension")
            .field("table", &self.table)
            .field(
                "functions",
                &self
                    .functions
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .field(
                "callbacks",
                &self
                    .callbacks
                    .iter()
                    .map(|setter| (&setter.name, &setter.callback))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Extension {
    /// An extension whose global table is called `table`: lowercase letters, digits and `_` (the VM
    /// name rule), and not `log`, `config`, `startup`, `shutdown` or the name of a standard-library
    /// global. [`VmOwner::spawn`](crate::VmOwner::spawn) refuses an invalid extension.
    #[must_use]
    pub fn new(table: &str) -> Self {
        Self {
            table: table.to_owned(),
            functions: Vec::new(),
            callbacks: Vec::new(),
        }
    }

    /// Adds the host function `table.name`.
    #[must_use]
    pub fn function(mut self, name: &str, function: HostFunction) -> Self {
        self.functions.push((name.to_owned(), function));
        self
    }

    /// Adds the setter `table.setter`: Lua's `table.setter(fn)` registers `fn` as the callback named
    /// `callback`, replacing any earlier one, and `table.setter(nil)` clears it.
    #[must_use]
    pub fn callback(mut self, setter: &str, callback: &str) -> Self {
        self.callbacks.push(Setter {
            name: setter.to_owned(),
            callback: callback.to_owned(),
            kind: SetterKind::Plain,
        });
        self
    }

    /// Adds the setter `table.setter` like [`callback`](Self::callback), plus an observer: after the setter registered
    /// (`true`) or cleared (`false`) the callback, `observer` is called on the owner thread, outside any `ck-lua` lock.
    /// A panic in it is owner-fatal, like one in a host function. Existing setters are unchanged. (A host function cannot
    /// take a Lua function, so a setter is the only way for Rust to learn that a callback was registered.)
    #[must_use]
    pub fn callback_observed(
        mut self,
        setter: &str,
        callback: &str,
        observer: CallbackObserver,
    ) -> Self {
        self.callbacks.push(Setter {
            name: setter.to_owned(),
            callback: callback.to_owned(),
            kind: SetterKind::Observed(observer),
        });
        self
    }

    /// Adds a setter `table.setter` that is **not available**: it performs exactly the argument validation of
    /// [`callback`](Self::callback) (one function or `nil`; the same "bad argument" errors otherwise) and then, for a
    /// valid function or `nil`, raises a catchable Lua error with `message`, **without** touching the callback table.
    /// It registers no callback, so it takes no callback name. Its name shares the table's key space and `validate`'s
    /// rules like any setter.
    #[must_use]
    pub fn unavailable_callback(mut self, setter: &str, message: &str) -> Self {
        self.callbacks.push(Setter {
            name: setter.to_owned(),
            callback: String::new(),
            kind: SetterKind::Unavailable(message.to_owned()),
        });
        self
    }

    /// Why this extension is invalid, if it is: a bad or reserved table name, a function or setter
    /// name that is not a Lua identifier, or a name used twice (function and setter names share the
    /// table's keys; callback names are unique too).
    pub(crate) fn validate(&self) -> Result<(), String> {
        let table = &self.table;
        if !valid_name(table) {
            return Err(format!(
                "`{table}` is not a valid extension table name (lowercase letters, digits and `_`)"
            ));
        }
        if RESERVED.contains(&table.as_str()) {
            return Err(format!("`{table}` is a reserved global name"));
        }
        let mut keys = BTreeSet::new();
        let mut callbacks = BTreeSet::new();
        let names = self
            .functions
            .iter()
            .map(|(name, _)| name)
            .chain(self.callbacks.iter().map(|setter| &setter.name));
        for name in names {
            if !identifier(name) {
                return Err(format!(
                    "`{name}` is not a valid name in extension `{table}` (a Lua identifier)"
                ));
            }
            if !keys.insert(name.as_str()) {
                return Err(format!("`{name}` is used twice in extension `{table}`"));
            }
        }
        for setter in self
            .callbacks
            .iter()
            .filter(|setter| !matches!(setter.kind, SetterKind::Unavailable(_)))
        {
            let callback = &setter.callback;
            if !identifier(callback) {
                return Err(format!(
                    "`{callback}` is not a valid callback name in extension `{table}` (a Lua identifier)"
                ));
            }
            if !callbacks.insert(callback.as_str()) {
                return Err(format!(
                    "callback `{callback}` is registered twice in extension `{table}`"
                ));
            }
        }
        Ok(())
    }

    /// Installs the table in `lua`; its setters register into `callbacks`, the one set of the VM.
    pub(crate) fn install(
        &self,
        lua: &Lua,
        fatal: &Fatal,
        callbacks: &Callbacks,
    ) -> mlua::Result<()> {
        let table = lua.create_table()?;
        for (name, function) in &self.functions {
            let function = Arc::clone(function);
            let label = format!("{}.{name}", self.table);
            let fatal = fatal.clone();
            table.set(
                name.as_str(),
                lua.create_function(move |lua, args: MultiValue| {
                    call_host(lua, &label, &function, &fatal, &args)
                })?,
            )?;
        }
        for setter in &self.callbacks {
            let label = format!("{}.{}", self.table, setter.name);
            let callback = setter.callback.clone();
            let kind = setter.kind.clone();
            let registered = Rc::clone(callbacks);
            let fatal = fatal.clone();
            table.set(
                setter.name.as_str(),
                lua.create_function(move |_, args: MultiValue| {
                    register(&registered, &label, &callback, &kind, &fatal, &args)
                })?,
            )?;
        }
        lua.globals().set(self.table.as_str(), table)
    }
}

/// Why a list of extensions is invalid, if it is: any one of them invalid, two with the same table name, or
/// a callback name registered by two of them (callback names are unique across the VM, because a posted
/// call names its callback). Function and setter names belong to their own table, so two tables may each
/// have, say, `open`. Checked before any thread starts, so installation order never decides which table
/// wins a name.
pub(crate) fn validate_all(extensions: &[Extension]) -> Result<(), String> {
    let mut tables = BTreeSet::new();
    let mut callbacks: HashMap<&str, &str> = HashMap::new();
    for extension in extensions {
        extension.validate()?;
        let table = extension.table.as_str();
        if !tables.insert(table) {
            return Err(format!("extension table `{table}` is given twice"));
        }
        for setter in extension
            .callbacks
            .iter()
            .filter(|setter| !matches!(setter.kind, SetterKind::Unavailable(_)))
        {
            let callback = &setter.callback;
            if let Some(first) = callbacks.insert(callback.as_str(), table) {
                return Err(format!(
                    "callback `{callback}` is registered by extension `{first}` and by extension `{table}`"
                ));
            }
        }
    }
    Ok(())
}

/// Whether `name` is a Lua identifier (so that `table.name` is valid Lua).
fn identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Runs one host function: converts the arguments (a bad one is a Lua argument error and `function` is
/// not called), calls it with a panic caught, and converts what it returns.
fn call_host(
    lua: &Lua,
    label: &str,
    function: &HostFunction,
    fatal: &Fatal,
    args: &MultiValue,
) -> mlua::Result<MultiValue> {
    let mut converted = Vec::with_capacity(args.len());
    for (position, value) in args.iter().enumerate() {
        converted.push(convert::from_lua(value, 0).map_err(|error| {
            mlua::Error::runtime(format!(
                "bad argument #{} to '{label}' ({error})",
                position + 1
            ))
        })?);
    }
    match catch_unwind(AssertUnwindSafe(|| function(&converted))) {
        Ok(Ok(values)) => values
            .iter()
            .map(|value| convert::to_lua(lua, value))
            .collect(),
        Ok(Err(text)) => Err(mlua::Error::runtime(text)),
        Err(payload) => {
            fatal.record(unwind::message(payload.as_ref()));
            unwind::dispose(payload);
            Err(interrupt::interrupted())
        }
    }
}

/// `table.setter(fn)`: registers or (for `nil`) clears the callback; an observed setter then tells its observer, and an
/// unavailable one refuses a valid call without touching the table.
fn register(
    callbacks: &Callbacks,
    label: &str,
    callback: &str,
    kind: &SetterKind,
    fatal: &Fatal,
    args: &MultiValue,
) -> mlua::Result<()> {
    let bad = |got: &str| {
        mlua::Error::runtime(format!(
            "bad argument #1 to '{label}' (function or nil expected, got {got})"
        ))
    };
    let function = match (args.len(), args.front()) {
        (1, Some(Value::Function(function))) => Some(function),
        (1, Some(Value::Nil)) => None,
        (1, Some(other)) => return Err(bad(other.type_name())),
        (0, _) => return Err(bad("no value")),
        _ => {
            return Err(mlua::Error::runtime(format!(
                "bad argument #2 to '{label}' (no value expected)"
            )));
        }
    };
    if let SetterKind::Unavailable(message) = kind {
        return Err(mlua::Error::runtime(message.clone()));
    }
    {
        let mut table = callbacks.borrow_mut();
        match function {
            Some(function) => {
                table.insert(callback.to_owned(), function.clone());
            }
            None => {
                table.remove(callback);
            }
        }
    }
    if let SetterKind::Observed(observer) = kind {
        // Outside the callback table's borrow and every `ck-lua` lock; a panic is owner-fatal, as in a host function.
        let registered = function.is_some();
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| observer(registered))) {
            fatal.record(unwind::message(payload.as_ref()));
            unwind::dispose(payload);
            return Err(interrupt::interrupted());
        }
    }
    Ok(())
}
