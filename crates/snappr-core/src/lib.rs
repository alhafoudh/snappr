//! The snappr pipeline: everything that touches a photo, and nothing that knows
//! which front end it is running in.
//!
//! Both the desktop app and the CLI drive this crate, which is the whole point of
//! it existing: the watcher, the candidate rule, the settle test, the defaults
//! table and the message catalogue each used to be written twice, once in Ruby and
//! once in Rust, agreeing only by convention.

pub mod config;
pub mod exif;
pub mod inspect;
pub mod key;
pub mod overlay;
pub mod overlay_preview;
pub mod overlay_svg;
pub mod preview;
pub mod problem;
pub mod vips;
pub mod watcher;
pub mod worker;

pub use config::{Orientation, PipelineConfig};
pub use key::key_for;
pub use overlay::Overlay;
pub use preview::{Preview, Previews};
pub use problem::{Problem, Warning};

/// A size for a human, in the two units that matter for a print: KB under a
/// megabyte, MB above it.
pub fn human_size(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    if (bytes as f64) < MIB {
        format!("{} KB", (bytes as f64 / 1024.0) as u64)
    } else {
        format!("{:.1} MB", bytes as f64 / MIB)
    }
}
