//! The service's limits (`docs/scene-design.md` §7.1): the five fields of `[resources]` that the resource service owns.
//! This crate parses no TOML; the application's configuration (M5-8) builds a [`ResourceLimits`] and
//! [`validate`](ResourceLimits::validate)s it.

use std::fmt;
use std::ops::RangeInclusive;

/// The longest path the file sources accept, in bytes (§7.1's proposed cap; used by milestone 5's file sources).
pub const MAX_PATH_BYTES: usize = 1024;

/// The longest stored error text, in bytes; longer text is cut at a UTF-8 boundary.
pub const MAX_ERROR_BYTES: usize = 2048;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// The registry capacity: well under the `u32` slot width.
pub(crate) const TEXTURES_RANGE: RangeInclusive<u32> = 1..=1_048_576;
/// Cheap metadata per job (§7.3).
pub(crate) const JOBS_RANGE: RangeInclusive<u32> = 1..=1_048_576;
/// Used by milestone 5's loader threads; validated here so the struct is complete.
pub(crate) const WORKERS_RANGE: RangeInclusive<u32> = 1..=16;
/// At least one image at the decoder's 64 MiB limit, or such an image could never load.
pub(crate) const CPU_PIXEL_RANGE: RangeInclusive<u64> = (64 * MIB)..=(64 * GIB);
/// Room for the registry's growth and the job records.
pub(crate) const METADATA_RANGE: RangeInclusive<u64> = MIB..=(16 * GIB);

/// What the service may hold. The defaults are §7.1's numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLimits {
    /// Logical texture slots: the registry capacity. Range 1 to 1,048,576.
    pub max_textures: u32,
    /// Pending jobs (job slots): a queued or executing request holds one. Range 1 to 1,048,576.
    pub max_pending_jobs: u32,
    /// Loader threads (used by milestone 5's file sources). Range 1 to 16.
    pub decode_workers: u32,
    /// CPU pixel bytes held at once, in sources, jobs and revisions waiting for upload. Range 64 MiB to 64 GiB.
    pub cpu_pixel_bytes: u64,
    /// Metadata bytes: a fixed charge per resource and per job, plus a stored error's length. Range 1 MiB to 16 GiB.
    pub metadata_bytes: u64,
}

impl ResourceLimits {
    /// The defaults of §7.1.
    pub const DEFAULT: Self = Self {
        max_textures: 65_536,
        max_pending_jobs: 65_536,
        decode_workers: 4,
        cpu_pixel_bytes: 512 * MIB,
        metadata_bytes: 128 * MIB,
    };

    /// Checks every field against its range.
    ///
    /// # Errors
    ///
    /// [`LimitsError`] naming the first field out of range, its value and the range.
    pub fn validate(&self) -> Result<(), LimitsError> {
        check(
            "max_textures",
            u64::from(self.max_textures),
            &widen(&TEXTURES_RANGE),
        )?;
        check(
            "max_pending_jobs",
            u64::from(self.max_pending_jobs),
            &widen(&JOBS_RANGE),
        )?;
        check(
            "decode_workers",
            u64::from(self.decode_workers),
            &widen(&WORKERS_RANGE),
        )?;
        check("cpu_pixel_bytes", self.cpu_pixel_bytes, &CPU_PIXEL_RANGE)?;
        check("metadata_bytes", self.metadata_bytes, &METADATA_RANGE)
    }

    /// The most metadata the resources and jobs these limits allow can be charged, in checked arithmetic (`None` if it
    /// does not fit a `u64`; at every end of every range it does).
    #[must_use]
    pub fn worst_case_metadata(&self) -> Option<u64> {
        let resources =
            u64::from(self.max_textures).checked_mul(crate::service::RESOURCE_METADATA_BYTES)?;
        let jobs =
            u64::from(self.max_pending_jobs).checked_mul(crate::service::JOB_METADATA_BYTES)?;
        resources.checked_add(jobs)
    }
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

fn widen(range: &RangeInclusive<u32>) -> RangeInclusive<u64> {
    u64::from(*range.start())..=u64::from(*range.end())
}

fn check(field: &'static str, value: u64, range: &RangeInclusive<u64>) -> Result<(), LimitsError> {
    if range.contains(&value) {
        Ok(())
    } else {
        Err(LimitsError {
            field,
            value,
            min: *range.start(),
            max: *range.end(),
        })
    }
}

/// A limit outside its range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitsError {
    /// The field's name.
    pub field: &'static str,
    /// Its value.
    pub value: u64,
    /// The smallest allowed value.
    pub min: u64,
    /// The largest allowed value.
    pub max: u64,
}

impl fmt::Display for LimitsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "resources.{} is {}, outside {} to {}",
            self.field, self.value, self.min, self.max
        )
    }
}

impl std::error::Error for LimitsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_documented_numbers_and_validate() {
        let defaults = ResourceLimits::DEFAULT;
        assert_eq!(defaults, ResourceLimits::default());
        assert_eq!(
            (
                defaults.max_textures,
                defaults.max_pending_jobs,
                defaults.decode_workers,
                defaults.cpu_pixel_bytes,
                defaults.metadata_bytes
            ),
            (65_536, 65_536, 4, 512 * 1024 * 1024, 128 * 1024 * 1024)
        );
        assert_eq!(defaults.validate(), Ok(()));
        assert_eq!((MAX_PATH_BYTES, MAX_ERROR_BYTES), (1024, 2048));
    }

    /// Every field is accepted at both ends of its range and refused one past each, naming the field.
    #[test]
    fn each_field_is_refused_one_past_each_end_naming_the_field() {
        type Set = fn(&mut ResourceLimits, u64);
        let fields: [(&str, Set, u64, u64); 5] = [
            (
                "max_textures",
                |l, v| l.max_textures = u32::try_from(v).unwrap(),
                1,
                1_048_576,
            ),
            (
                "max_pending_jobs",
                |l, v| l.max_pending_jobs = u32::try_from(v).unwrap(),
                1,
                1_048_576,
            ),
            (
                "decode_workers",
                |l, v| l.decode_workers = u32::try_from(v).unwrap(),
                1,
                16,
            ),
            (
                "cpu_pixel_bytes",
                |l, v| l.cpu_pixel_bytes = v,
                64 * MIB,
                64 * GIB,
            ),
            ("metadata_bytes", |l, v| l.metadata_bytes = v, MIB, 16 * GIB),
        ];
        for (name, set, min, max) in fields {
            for accepted in [min, max] {
                let mut limits = ResourceLimits::DEFAULT;
                set(&mut limits, accepted);
                assert_eq!(limits.validate(), Ok(()), "{name} = {accepted}");
            }
            for refused in [min - 1, max + 1] {
                let mut limits = ResourceLimits::DEFAULT;
                set(&mut limits, refused);
                let error = limits.validate().expect_err(name);
                assert_eq!(
                    error,
                    LimitsError {
                        field: error.field,
                        value: refused,
                        min,
                        max
                    },
                    "{name} = {refused}"
                );
                assert_eq!(error.field, name);
                let message = error.to_string();
                assert!(
                    message.contains(name) && message.contains(&refused.to_string()),
                    "{message}"
                );
            }
        }
    }

    #[test]
    fn the_arithmetic_on_the_limits_is_checked_at_every_range_end() {
        for textures in [1, 1_048_576] {
            for jobs in [1, 1_048_576] {
                let limits = ResourceLimits {
                    max_textures: textures,
                    max_pending_jobs: jobs,
                    ..ResourceLimits::DEFAULT
                };
                let worst = limits.worst_case_metadata().expect("fits a u64");
                assert_eq!(
                    worst,
                    u64::from(textures) * crate::service::RESOURCE_METADATA_BYTES
                        + u64::from(jobs) * crate::service::JOB_METADATA_BYTES
                );
            }
        }
        // And with the other end of the metadata range: the default charges do not exceed what one more byte allows.
        let top = ResourceLimits {
            max_textures: 1_048_576,
            max_pending_jobs: 1_048_576,
            cpu_pixel_bytes: 64 * GIB,
            metadata_bytes: 16 * GIB,
            decode_workers: 16,
        };
        assert_eq!(top.validate(), Ok(()));
        assert!(top.worst_case_metadata().is_some());
    }
}
