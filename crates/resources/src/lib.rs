//! `ck-resources`: images decoded from PNG under budgets, and the assets rule.
//!
//! This is the resource component of `docs/scene-design.md` §3: CPU-only, with no dependency on the scene, a renderer,
//! a GPU or a window.
//!
//! - [`Image`]: premultiplied RGBA8, made only by decoding a PNG under the budgets of [`budget`].
//! - [`Assets`]: the assets rule, which files the render API opens.
//! - [`TextureRegistry`], [`TextureKey`] and [`TextureRef`]: generational texture identity, public ownership, retained
//!   references and retirement; [`Token`], the string form of a handle, and [`RegistryId`]. Handle tokens are
//!   process-local identities, not saved data or asset IDs, and not a security boundary (the Lua runtime stays
//!   unsandboxed, `docs/scene-design.md` §4.1).
//! - [`ResourceService`]: resources admitted from Rust buffers or PNG files ([`ResourceService::load`]), their revisions,
//!   budgets and retirement; [`LoaderPool`]: the fixed set of threads that run its jobs, and a bounded end.
//!
//! # The end of a run
//!
//! The same pool runs CPU and file jobs with or without a backend. A run ends with [`LoaderPool::shutdown`] (a bounded
//! wait for the threads; a running decode is not interrupted) and then [`ResourceService::finish_teardown`], under the
//! caller's own deadline. **The error of a file that had not finished loading when the run ended is never observed**:
//! the run does not wait for optional textures (`docs/scene-design.md` §8.3).

#![deny(unsafe_code)]

mod assets;
pub mod budget;
mod chunks;
mod convert;
mod image;
#[cfg(feature = "test-support")]
mod jobgate;
mod ledger;
mod limits;
mod loaders;
#[cfg(test)]
mod lockdebug;
mod pixels;
mod queue;
mod registry;
mod service;
#[cfg(test)]
mod testkit;
mod token;
mod unwind;

/// Without the `test-support` feature the crate has no job gate and the service no way to be given one: this fails to
/// compile in an ordinary build (and is absent from a build that asks for the feature).
///
/// ```compile_fail
/// use ck_resources::JobGate;
/// let _ = JobGate::new();
/// ```
///
/// ```compile_fail
/// fn gated(service: &ck_resources::ResourceService) {
///     service.set_job_gate(std::sync::Arc::new(Default::default()));
/// }
/// ```
#[cfg(not(feature = "test-support"))]
#[expect(
    dead_code,
    reason = "carries the doctests that prove an ordinary build has no job gate"
)]
const NO_JOB_GATE_WITHOUT_THE_FEATURE: () = ();

pub use assets::{Assets, AssetsError, OpenReserveError};
pub use budget::DecodeLimits;
pub use image::{DecodeError, Image, ReserveError, TooLarge};
#[cfg(feature = "test-support")]
pub use jobgate::JobGate;
pub use ledger::{BudgetKind, BudgetStats, LedgerId, LedgerIdsExhausted, Reservation};
pub use limits::{LimitsError, MAX_ERROR_BYTES, MAX_PATH_BYTES, ResourceLimits};
pub use loaders::{LoaderPool, LoaderShutdown, LoopFailure, SpawnError};
pub use pixels::{CpuPixels, CpuPixelsError, PixelReservation, PixelRevision};
pub use registry::{
    CreateError, HandleError, NewError, RegistryStats, RetiringWake, TextureKey, TextureRef,
    TextureRegistry,
};
pub use service::{
    AdmitError, CommitOutcome, Decoded, ErrorText, Job, JobFailure, JobOutput, LegacyAdmitError,
    LegacyBudget, LoaderStats, Maintained, Mode, Refused, ResourceError, ResourceService,
    ResourceState, ServiceError, ServiceStats, ServiceWake, ServiceWakes, ShownRevision,
    StatusState, TextureStatus, UploadRequest,
};
pub use token::{
    MAX_TOKEN_BYTES, RegistryId, RegistryIdsExhausted, Token, TokenError, TokenField, TokenKind,
};
