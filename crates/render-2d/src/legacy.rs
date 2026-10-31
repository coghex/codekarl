//! The synchronous legacy quad compatibility lane (M5-5d-1).

use std::fmt;
use std::sync::Arc;

use ck_resources::{
    Assets, AssetsError, HandleError, LegacyAdmitError, ResourceService, ResourceState, TextureRef,
};

use crate::bridge::SceneWriter;
use crate::quad::{Quad, QuadError};

/// The old `render.quad` operation, backed by the shared resource service and scene compatibility slot.
#[derive(Debug)]
pub struct LegacyLane {
    service: ResourceService,
    assets: Option<Assets>,
}

/// A compatibility call failed before it could change the scene.
#[derive(Debug)]
pub enum LegacyError {
    /// The file could not be opened or decoded.
    Assets(AssetsError),
    /// Synchronous resource admission failed.
    Admission(LegacyAdmitError),
    /// Geometry did not satisfy the legacy quad rules.
    Geometry(QuadError),
    /// The just-admitted resource could not be retained or released.
    Handle(HandleError),
    /// No assets root is available for the ended-service compatibility path.
    NoAssets,
}

impl fmt::Display for LegacyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Assets(error) => error.fmt(formatter),
            Self::Admission(error) => error.fmt(formatter),
            Self::Geometry(error) => error.fmt(formatter),
            Self::Handle(error) => error.fmt(formatter),
            Self::NoAssets => formatter.write_str("no assets directory is configured"),
        }
    }
}

impl std::error::Error for LegacyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Assets(error) => Some(error),
            Self::Admission(error) => Some(error),
            Self::Geometry(error) => Some(error),
            Self::Handle(error) => Some(error),
            Self::NoAssets => None,
        }
    }
}

impl LegacyLane {
    /// Makes a lane over the shared resource service and the assets root used by the legacy fallback after shutdown.
    #[must_use]
    pub fn new(service: ResourceService, assets: Option<Assets>) -> Self {
        Self { service, assets }
    }

    /// Synchronously admits and replaces the one compatibility quad.
    ///
    /// File and decode errors keep the legacy call's wording; a failed call leaves the previous compatibility quad
    /// unchanged. The decoded image's size supplies omitted width and height. This is [`admit`](Self::admit) followed by
    /// [`install`](Self::install) on one writer; a caller that must not hold a lock on its writer while the image is read
    /// and decoded calls the two itself.
    ///
    /// # Errors
    ///
    /// Returns an assets, shared-budget, resource, handle, or geometry error without changing the compatibility lane.
    pub fn quad(
        &self,
        writer: &SceneWriter,
        name: &str,
        x: f64,
        y: f64,
        width: Option<f64>,
        height: Option<f64>,
    ) -> Result<(), LegacyError> {
        let admission = self.admit(writer.is_ended(), name, x, y, width, height)?;
        self.install(writer, admission)
    }

    /// The first half of [`quad`](Self::quad), which needs **no writer**: reads and decodes the image (the synchronous I/O),
    /// checks the geometry and retains the admitted texture, or, when the bridge `ended` (or the service has), decodes and
    /// validates the image without admitting it. Nothing is installed; a failure changes nothing.
    ///
    /// `ended` is the writer's [`is_ended`](SceneWriter::is_ended) as the caller saw it: a bridge that ended while the image
    /// was decoding is caught again by [`install`](Self::install), which discards and counts the write.
    ///
    /// # Errors
    ///
    /// Returns an assets, shared-budget, resource, handle, or geometry error.
    pub fn admit(
        &self,
        ended: bool,
        name: &str,
        x: f64,
        y: f64,
        width: Option<f64>,
        height: Option<f64>,
    ) -> Result<Admission, LegacyError> {
        if ended {
            return self.decode_and_validate(name, x, y, width, height);
        }
        let decoded = match self.service.create_decoded(name) {
            Ok(decoded) => decoded,
            Err(LegacyAdmitError::Ended) => {
                return self.decode_and_validate(name, x, y, width, height);
            }
            Err(LegacyAdmitError::Assets(error)) => return Err(LegacyError::Assets(error)),
            Err(error) => return Err(LegacyError::Admission(error)),
        };
        let width = width.unwrap_or_else(|| f64::from(decoded.width));
        let height = height.unwrap_or_else(|| f64::from(decoded.height));
        if let Err(error) = crate::quad::validate_rect(x, y, width, height) {
            let _ = self.service.release(decoded.key);
            return Err(LegacyError::Geometry(error));
        }

        // The public resource key must never reach zero owners between admission and installation in the scene.
        let texture = match self.service.retain(decoded.key) {
            Ok(texture) => texture,
            Err(error) => {
                let _ = self.service.release(decoded.key);
                return Err(LegacyError::Handle(error));
            }
        };
        if let Err(error) = self.service.release(decoded.key) {
            drop(texture);
            return Err(LegacyError::Handle(error));
        }
        Ok(Admission(AdmissionKind::Texture {
            texture,
            x,
            y,
            width,
            height,
        }))
    }

    /// The second half of [`quad`](Self::quad): puts an [`Admission`] into the writer's compatibility lane (a short, O(1)
    /// step with no I/O). If the bridge ended while the image was decoding, the temporary resource is retired and the valid
    /// legacy write is counted as discarded, as before, not returned as an ended-service error.
    ///
    /// # Errors
    ///
    /// Returns a geometry error if the writer refuses the rectangle; the previous compatibility quad is then unchanged.
    pub fn install(&self, writer: &SceneWriter, admission: Admission) -> Result<(), LegacyError> {
        match admission.0 {
            AdmissionKind::Discard => {
                writer.count_discarded_compat_write();
                Ok(())
            }
            AdmissionKind::Texture {
                texture,
                x,
                y,
                width,
                height,
            } => {
                // A bridge that ended while the image decoded: `set_compat` counts the valid write as discarded and drops
                // the texture (retiring the temporary resource), exactly as the old M4 behaviour, and is not an error.
                writer
                    .set_compat(texture, x, y, width, height)
                    .map_err(LegacyError::Geometry)
            }
        }
    }

    /// Clears only the compatibility quad. A call after the bridge ended is counted and discarded by the writer.
    pub fn clear(&self, writer: &SceneWriter) {
        writer.clear_compat();
    }

    /// An ended bridge or service: the image is read, decoded and validated (the legacy call's errors) and nothing is
    /// admitted; [`install`](Self::install) counts the write as discarded.
    fn decode_and_validate(
        &self,
        name: &str,
        x: f64,
        y: f64,
        width: Option<f64>,
        height: Option<f64>,
    ) -> Result<Admission, LegacyError> {
        let assets = self.assets.as_ref().ok_or(LegacyError::NoAssets)?;
        let image = assets.open(name).map_err(LegacyError::Assets)?;
        let _quad =
            Quad::new(Arc::new(image), x, y, width, height).map_err(LegacyError::Geometry)?;
        Ok(Admission(AdmissionKind::Discard))
    }
}

/// An image the lane admitted and the writer has not taken yet ([`LegacyLane::admit`] makes one, [`LegacyLane::install`]
/// consumes it). Dropping it instead releases the texture it retained.
#[must_use = "an admission does nothing until it is installed"]
pub struct Admission(AdmissionKind);

enum AdmissionKind {
    /// The retained texture and its rectangle, to become the compatibility quad.
    Texture {
        texture: TextureRef<ResourceState>,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
    /// The bridge or the service had ended: the write is counted as discarded.
    Discard,
}

impl fmt::Debug for Admission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.0 {
            AdmissionKind::Texture { .. } => "Admission(texture)",
            AdmissionKind::Discard => "Admission(discard)",
        })
    }
}
