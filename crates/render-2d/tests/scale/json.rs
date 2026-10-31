//! A minimal JSON value, enough for the harness's evidence (the workspace has no JSON dependency, and the report only writes).

use std::fmt::Write as _;

/// A JSON value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum J {
    Null,
    Bool(bool),
    U(u64),
    F(f64),
    S(String),
    A(Vec<J>),
    O(Vec<(String, J)>),
}

impl J {
    /// A string value.
    pub(crate) fn s(text: impl Into<String>) -> Self {
        Self::S(text.into())
    }

    /// An object from `(name, value)` pairs, in the order given.
    pub(crate) fn o<const N: usize>(fields: [(&str, J); N]) -> Self {
        Self::O(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }

    /// Appends `(name, value)` to an object.
    ///
    /// # Panics
    ///
    /// When `self` is not an object.
    pub(crate) fn push(&mut self, name: &str, value: J) {
        match self {
            Self::O(fields) => fields.push((name.to_owned(), value)),
            other => panic!("not an object: {other:?}"),
        }
    }

    /// The value of field `name` of an object.
    #[must_use]
    pub(crate) fn get(&self, name: &str) -> Option<&J> {
        match self {
            Self::O(fields) => fields.iter().find(|(k, _)| k == name).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The compact text.
    #[must_use]
    pub(crate) fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::U(value) => {
                let _ = write!(out, "{value}");
            }
            Self::F(value) if value.is_finite() => {
                let _ = write!(out, "{value}");
            }
            Self::Null | Self::F(_) => out.push_str("null"),
            Self::S(text) => {
                out.push('"');
                for c in text.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
                        c if u32::from(c) < 0x20 => {
                            let _ = write!(out, "\\u{:04x}", u32::from(c));
                        }
                        c => out.push(c),
                    }
                }
                out.push('"');
            }
            Self::A(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Self::O(fields) => {
                out.push('{');
                for (index, (name, value)) in fields.iter().enumerate() {
                    if index != 0 {
                        out.push(',');
                    }
                    Self::S(name.clone()).write(out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}
