//! The fixtures directory of a scale run: set A and set B, generated at run time into a fresh temporary directory (deterministic, never
//! committed), removed when the run's handle drops.

use std::fs;
use std::path::{Path, PathBuf};

use crate::layout;

/// A fresh temporary directory holding the fixtures a phase needs.
pub(crate) struct Fixtures {
    root: PathBuf,
}

impl Fixtures {
    /// Makes an empty directory under the system temporary directory, named for this process and `label`.
    ///
    /// # Panics
    ///
    /// When the directory cannot be made.
    #[must_use]
    pub(crate) fn create(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!("ck-scale-{}-{label}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("a temporary fixtures directory");
        Self { root }
    }

    /// The directory.
    #[must_use]
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Writes set A: `count` distinct 32 x 32 identity PNGs `a/00000.png` ... (`count` is 32,768 for the recipe).
    ///
    /// # Panics
    ///
    /// On a file-system error.
    pub(crate) fn write_set_a(&self, count: u32) {
        let dir = self.root.join("a");
        fs::create_dir_all(&dir).expect("the set A directory");
        for k in 0..count {
            fs::write(
                self.root.join(layout::set_a_name(k)),
                layout::identity_png(k, layout::SET_A_SIDE),
            )
            .expect("a set A file");
        }
    }

    /// Writes set B: 128 images at 64 x 64, 64 at 256 x 256, 48 at 1024 x 1024 and 8 at 2048 x 2048, identity-coded (texture `n`).
    ///
    /// # Panics
    ///
    /// On a file-system error.
    pub(crate) fn write_set_b(&self) {
        let dir = self.root.join("b");
        fs::create_dir_all(&dir).expect("the set B directory");
        for n in 0..layout::set_b_count() {
            fs::write(
                self.root.join(layout::set_b_name(n)),
                layout::identity_png(n, layout::set_b_side(n)),
            )
            .expect("a set B file");
        }
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
