//! The configuration copy a script sees as `config`: an immutable userdata proxy.
//!
//! A proxy is a userdata, not a table, so Lua's `rawset` (which accepts only tables) cannot write to
//! it, and every assignment goes to a `__newindex` that always raises. Nested tables and arrays come
//! back as further proxies.

use std::collections::BTreeMap;
use std::sync::Arc;

use mlua::{Function, Lua, MetaMethod, UserData, UserDataMethods, Value};

/// A configuration value, copied into a VM owner. It keeps `ck-lua` independent of any configuration
/// format: the application converts its own configuration into this tree.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    /// A table with string keys, read in key order by `pairs`.
    Table(BTreeMap<String, ConfigValue>),
    /// An array, indexed from 1 in Lua.
    Array(Vec<ConfigValue>),
    /// A string.
    String(String),
    /// An integer.
    Integer(i64),
    /// A floating-point number.
    Float(f64),
    /// A boolean.
    Bool(bool),
}

/// A float with an integral value that fits an integer, as the integer; `None` for a fraction, a
/// NaN, an infinity or a value out of range.
fn integral(number: f64) -> Option<i64> {
    // 2^63 as a float: the first value past `i64::MAX` (`i64::MIN` is exactly -2^63).
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    if number.fract() == 0.0 && (-LIMIT..LIMIT).contains(&number) {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "an integral value in [-2^63, 2^63) converts exactly"
        )]
        Some(number as i64)
    } else {
        None
    }
}

/// The read-only proxy over one table or array of the copied tree.
pub(crate) struct ConfigProxy(Arc<ConfigValue>);

impl ConfigProxy {
    /// The Lua value for `value`: a scalar as itself, a table or an array as a proxy.
    pub(crate) fn to_lua(lua: &Lua, value: &ConfigValue) -> mlua::Result<Value> {
        Ok(match value {
            ConfigValue::String(text) => Value::String(lua.create_string(text)?),
            ConfigValue::Integer(number) => Value::Integer(*number),
            ConfigValue::Float(number) => Value::Number(*number),
            ConfigValue::Bool(flag) => Value::Boolean(*flag),
            ConfigValue::Table(_) | ConfigValue::Array(_) => {
                Value::UserData(lua.create_userdata(Self(Arc::new(value.clone())))?)
            }
        })
    }

    /// `proxy[key]`: a missing key, or a key of the wrong kind, is `nil`.
    fn get(&self, lua: &Lua, key: &Value) -> mlua::Result<Value> {
        let found = match (&*self.0, key) {
            (ConfigValue::Table(entries), Value::String(name)) => entries.get(&*name.to_str()?),
            (ConfigValue::Array(items), Value::Integer(index)) => Self::item(items, *index),
            // Lua turns a float key with an integral value into the integer, as for a table.
            (ConfigValue::Array(items), Value::Number(number)) => {
                integral(*number).and_then(|index| Self::item(items, index))
            }
            _ => None,
        };
        found.map_or(Ok(Value::Nil), |value| Self::to_lua(lua, value))
    }

    /// The array item at Lua index `index` (from 1), if there is one.
    fn item(items: &[ConfigValue], index: i64) -> Option<&ConfigValue> {
        usize::try_from(index)
            .ok()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| items.get(index))
    }

    /// `#proxy`: the length of an array, 0 for a table.
    fn len(&self) -> i64 {
        match &*self.0 {
            ConfigValue::Array(items) => i64::try_from(items.len()).unwrap_or(i64::MAX),
            _ => 0,
        }
    }

    /// `pairs(proxy)`: the entries in order (a table by key, an array by index from 1), as a Lua
    /// iterator function over a snapshot.
    fn pairs(&self, lua: &Lua) -> mlua::Result<(Function, Value, Value)> {
        let mut entries: Vec<(Value, Value)> = Vec::new();
        match &*self.0 {
            ConfigValue::Table(map) => {
                for (key, value) in map {
                    entries.push((
                        Value::String(lua.create_string(key)?),
                        Self::to_lua(lua, value)?,
                    ));
                }
            }
            ConfigValue::Array(items) => {
                for (index, value) in (1_i64..).zip(items) {
                    entries.push((Value::Integer(index), Self::to_lua(lua, value)?));
                }
            }
            _ => {}
        }
        let next = std::cell::Cell::new(0_usize);
        let iterator = lua.create_function(move |_, _: (Value, Value)| {
            let at = next.get();
            next.set(at + 1);
            Ok(entries.get(at).cloned().unwrap_or((Value::Nil, Value::Nil)))
        })?;
        Ok((iterator, Value::Nil, Value::Nil))
    }
}

impl UserData for ConfigProxy {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::Index, |lua, this, key: Value| {
            this.get(lua, &key)
        });
        methods.add_meta_method(
            MetaMethod::NewIndex,
            |_, _, (_key, _value): (Value, Value)| -> mlua::Result<()> {
                Err(mlua::Error::runtime("config is read-only"))
            },
        );
        methods.add_meta_method(MetaMethod::Len, |_, this, ()| Ok(this.len()));
        methods.add_meta_method(MetaMethod::Pairs, |lua, this, ()| this.pairs(lua));
    }
}
