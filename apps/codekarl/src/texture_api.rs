//! The `texture` extension of the UI VM (design §8.2): the Lua surface of the resource service's file textures.
//!
//! [`extension`] builds the `ck-lua` [`Extension`] the VM named `ui` gets, next to `window`, `render` and `frame`; no other
//! VM does. Its host functions run on the UI VM's owner thread and report a refusal as an ordinary, catchable Lua error whose
//! message starts with the function's name:
//!
//! - `texture.load(name)` → a texture token string. The service's `load` admits the request and returns at once: no file is
//!   touched here, the name rules and the 1,024-byte cap are checked at the call, and a missing or broken file arrives
//!   later in `status` (design §5.2). **There is no call that makes a texture from Lua pixels** (design §5.3, Firm);
//! - `texture.reload(token)` → the new requested revision (an integer);
//! - `texture.release(token)`: ends the script's public ownership; a second release is a stale-token error;
//! - `texture.status(token)` → a table (see [`status_table`]); it never waits and is not a promise that a pixel was shown.
//!
//! A token argument is a string of at most 64 bytes, parsed strictly by the registry; a quad token where a texture token is
//! wanted is a wrong-kind error, and a token of another run's registry is foreign. Tokens are process-local, not saved data.
//!
//! **Every call validates its cheap arguments first** (the argument shape and the token's syntax) and only then looks at the
//! service: after the run's end (the service was closed) it raises the service's `Ended` error without touching a file, a
//! worker or the registry (a test's run without a service raises [`no_service`]). A close that lands just after the check is the service's to refuse: `load` and `reload` return
//! `Ended` from inside, and `release` and `status` then see an ended registry (stale) at worst. The next call reports
//! `Ended`.

use std::collections::BTreeMap;
use std::sync::Arc;

use ck_lua::{ConfigValue, Extension, HostFunction};
use ck_resources::{
    AdmitError, Assets, MAX_PATH_BYTES, ResourceService, StatusState, TextureKey, TextureStatus,
};

/// The message of a call of a run that has no resource service (only a test builds one).
pub(crate) fn no_service(function: &str) -> String {
    format!("{function}: this run has no resource service")
}

/// A host function of this module: a method of [`Shared`].
type HostCall = fn(&Shared, &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String>;

/// What the four host functions share.
struct Shared {
    /// The run's resource service; `None` only in a test that builds the extension without one.
    service: Option<ResourceService>,
    /// What the run's measurement window learns of a `texture.load` (nothing else depends on it).
    on_load: Option<LoadNotes>,
}

/// The run's measurement window and a `texture.load`: the call takes a timestamp **before** the service can publish the request to
/// another thread, and tells the window that timestamp only if the request was accepted, so a loader and the render thread can
/// finish the work before the caller's own continuation runs without the span starting after it ended.
#[derive(Clone)]
pub(crate) struct LoadNotes {
    /// The instant to stamp a request with, read before it is submitted.
    pub(crate) before: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// The request stamped at that instant was accepted.
    pub(crate) accepted: Arc<dyn Fn(u64) + Send + Sync>,
}

/// The one string argument of `function`, as a `&str`.
fn one_string<'a>(
    function: &str,
    arguments: &'a [Option<ConfigValue>],
    what: &str,
) -> Result<&'a str, String> {
    match arguments {
        [Some(ConfigValue::String(text))] => Ok(text),
        _ => Err(format!(
            "{function}: expected one string argument (a {what})"
        )),
    }
}

/// A texture token argument: one string, parsed strictly. Nothing but the text is looked at.
fn token_argument(function: &str, arguments: &[Option<ConfigValue>]) -> Result<TextureKey, String> {
    let text = one_string(function, arguments, "texture token")?;
    TextureKey::from_token(text).map_err(|error| format!("{function}: {error}"))
}

/// The error of an ended service, as the service words it.
fn ended(function: &str) -> String {
    format!("{function}: {}", AdmitError::Ended)
}

impl Shared {
    /// The service of a call, or the refusal of this backend or of a run that ended. Called after the argument checks.
    fn service(&self, function: &str) -> Result<&ResourceService, String> {
        let Some(service) = &self.service else {
            return Err(no_service(function));
        };
        if service.is_closed() {
            return Err(ended(function));
        }
        Ok(service)
    }

    fn load(&self, arguments: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        let name = one_string("texture.load", arguments, "file name")?;
        // The name rules and the byte cap are the service's own, and cheap: they come before the backend/ended refusal.
        if name.len() > MAX_PATH_BYTES {
            return Err(format!(
                "texture.load: {}",
                AdmitError::PathTooLong { bytes: name.len() }
            ));
        }
        Assets::check_name(name)
            .map_err(|error| format!("texture.load: {}", AdmitError::Name(error.to_string())))?;
        let service = self.service("texture.load")?;
        let stamp = self.on_load.as_ref().map(|notes| (notes.before)());
        let key = service
            .load(name)
            .map_err(|error| format!("texture.load: {error}"))?;
        if let (Some(notes), Some(stamp)) = (&self.on_load, stamp) {
            (notes.accepted)(stamp);
        }
        Ok(vec![ConfigValue::String(key.token().to_string())])
    }

    fn reload(&self, arguments: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        let key = token_argument("texture.reload", arguments)?;
        let service = self.service("texture.reload")?;
        let revision = service
            .reload(key)
            .map_err(|error| format!("texture.reload: {error}"))?;
        Ok(vec![integer("texture.reload", revision)?])
    }

    fn release(&self, arguments: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        let key = token_argument("texture.release", arguments)?;
        let service = self.service("texture.release")?;
        service
            .release(key)
            .map_err(|error| format!("texture.release: {error}"))?;
        Ok(Vec::new())
    }

    fn status(&self, arguments: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        let key = token_argument("texture.status", arguments)?;
        let service = self.service("texture.status")?;
        // A released token is stale: the status needs live public ownership (design §8.2).
        service
            .check(key)
            .map_err(|error| format!("texture.status: {error}"))?;
        let status = service
            .status(key)
            .map_err(|error| format!("texture.status: {error}"))?;
        Ok(vec![status_table(&status)?])
    }
}

/// A revision or a size as a Lua integer.
fn integer(function: &str, value: u64) -> Result<ConfigValue, String> {
    i64::try_from(value)
        .map(ConfigValue::Integer)
        .map_err(|_| format!("{function}: {value} does not fit a Lua integer"))
}

/// The name of a state in the status table.
fn state_name(state: StatusState) -> &'static str {
    match state {
        StatusState::Pending => "pending",
        StatusState::Decoding => "decoding",
        StatusState::Uploading => "uploading",
        StatusState::CpuReady => "cpu_ready",
        StatusState::Ready => "ready",
        StatusState::Failed => "failed",
    }
}

/// The table `texture.status` returns: `state`, `requested`, and, when there is one, `committed`, `shown` (the revision
/// adopted by the backend) with its `width` and `height`, and `error` with `error_revision`. An absent value is an absent key.
fn status_table(status: &TextureStatus) -> Result<ConfigValue, String> {
    let mut table = BTreeMap::new();
    table.insert(
        "state".to_owned(),
        ConfigValue::String(state_name(status.state).to_owned()),
    );
    table.insert(
        "requested".to_owned(),
        integer("texture.status", status.requested)?,
    );
    if let Some(committed) = status.committed {
        table.insert(
            "committed".to_owned(),
            integer("texture.status", committed)?,
        );
    }
    if let Some(shown) = status.shown {
        table.insert(
            "shown".to_owned(),
            integer("texture.status", shown.revision)?,
        );
        table.insert(
            "width".to_owned(),
            ConfigValue::Integer(i64::from(shown.width)),
        );
        table.insert(
            "height".to_owned(),
            ConfigValue::Integer(i64::from(shown.height)),
        );
    }
    if let Some(error) = &status.error {
        table.insert(
            "error".to_owned(),
            ConfigValue::String(error.message().to_owned()),
        );
        table.insert(
            "error_revision".to_owned(),
            integer("texture.status", error.revision)?,
        );
    }
    Ok(ConfigValue::Table(table))
}

/// The `texture` extension. `service` is the run's resource service (`None` only in a test, where every call validates its
/// arguments and then says the run has no service).
#[cfg(test)]
pub(crate) fn extension(service: Option<ResourceService>) -> Extension {
    extension_noting(service, None)
}

/// [`extension`] that also tells `on_load` about `texture.load` calls (see [`LoadNotes`]).
pub(crate) fn extension_noting(
    service: Option<ResourceService>,
    on_load: Option<LoadNotes>,
) -> Extension {
    let shared = Arc::new(Shared { service, on_load });
    let function = |call: HostCall| {
        let shared = Arc::clone(&shared);
        let function: HostFunction = Arc::new(move |arguments| call(&shared, arguments));
        function
    };
    Extension::new("texture")
        .function("load", function(Shared::load))
        .function("reload", function(Shared::reload))
        .function("release", function(Shared::release))
        .function("status", function(Shared::status))
}
