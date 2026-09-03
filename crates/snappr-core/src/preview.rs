//! The JPEGs the UI displays. Three per photo: a filmstrip thumbnail, a large
//! preview of the composite, and a large preview of the untouched source frame
//! for the overlayed/original toggle.
//!
//! **The thumbnail is made from the original photo, not from the composite.** At
//! filmstrip size the branding panel is a fifth of the cell and identical on
//! every one of them, so a strip of composites is a strip of near-identical
//! logos; the whole frame is what tells the operator which photo a cell is. The
//! composite is what the stage shows, and the toggle is there for comparing them.
//!
//! These exist because the webview cannot be shown the real output. Tauri's asset
//! protocol reads the whole file into memory for every request, and the overlay
//! PNGs are 10-25 MB and 16-bit when the source was HEIC - a filmstrip of those
//! would exhaust memory long before it finished drawing.
//!
//! Three decisions here are load-bearing:
//!
//! 1. **Previews are derived from the finished files on disk, not spliced into
//!    the compositing pipeline.** Re-reading the written PNG costs a fraction of
//!    a second. Fusing this into the composite would save that and put the
//!    invariant those tests guard - the photo bit-identical to the source
//!    wherever the overlay is clear - at risk for nothing.
//!
//! 2. **Previews never go in the watched folder.** `jpg` is a watched extension,
//!    so a preview beside its original would pass the watcher's candidate filter,
//!    be composited, produce previews of its own, and the pipeline would eat its
//!    own output forever. The overlay PNG gets away with living there only
//!    because `png` is not watched. That is why the destination directory is a
//!    **required argument with no default** that could quietly fall back to the
//!    drop folder.
//!
//! 3. **Previews are converted to sRGB.** Photos from an iPhone are Display P3.
//!    A webview interprets an untagged JPEG as sRGB, so handing it P3 numbers
//!    unconverted shows the operator colours that are not what will print.
//!    `export_profile` does the conversion and tags the result, and it copes with
//!    a source that has no profile at all by assuming sRGB.

use std::path::{Path, PathBuf};

use rs_vips::enums::Size;
use rs_vips::voption::{Setter, VOption};
use rs_vips::VipsImage;

use crate::config::PipelineConfig;
use crate::key::key_for;
use crate::problem::Problem;
use crate::vips;

/// Which of the three a preview is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Thumb,
    Overlay,
    Source,
}

impl Kind {
    pub const ALL: [Self; 3] = [Self::Thumb, Self::Overlay, Self::Source];

    fn as_str(self) -> &'static str {
        match self {
            Self::Thumb => "thumb",
            Self::Overlay => "overlay",
            Self::Source => "source",
        }
    }
}

/// The three files, and the key they share.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Previews {
    pub key: String,
    pub thumb: PathBuf,
    pub overlay: PathBuf,
    pub source: PathBuf,
}

pub struct Preview {
    config: PipelineConfig,
    dir: PathBuf,
}

impl Preview {
    /// `dir` is required and has no default. See the header: a default could
    /// point at the watched folder, and that mistake is unrecoverable in a way
    /// nothing else here is.
    pub fn new(config: PipelineConfig, dir: PathBuf) -> Self {
        Self { config, dir }
    }

    /// Writes the three previews for one photo.
    ///
    /// `destination` is the overlay PNG the pipeline just wrote; `source` is the
    /// original photo.
    pub fn call(&self, source: &Path, destination: &Path) -> Result<Previews, Problem> {
        let key = key_for(&[source, destination]);
        std::fs::create_dir_all(&self.dir).map_err(|error| Problem::PhotoUnusable {
            detail: format!("could not create {}: {error}", self.dir.display()),
        })?;

        Ok(Previews {
            thumb: self.write(source, &key, Kind::Thumb, self.config.preview_thumb_px)?,
            overlay: self.write(destination, &key, Kind::Overlay, self.config.preview_max_px)?,
            source: self.write(source, &key, Kind::Source, self.config.preview_max_px)?,
            key,
        })
    }

    pub fn path_for(&self, key: &str, kind: Kind) -> PathBuf {
        self.dir.join(format!("{key}.{}.jpg", kind.as_str()))
    }

    fn write(
        &self,
        from: &Path,
        key: &str,
        kind: Kind,
        max_px: i32,
    ) -> Result<PathBuf, Problem> {
        let target = self.path_for(key, kind);
        let temp = self
            .dir
            .join(format!("{key}.{}.tmp-{}.jpg", kind.as_str(), std::process::id()));

        // `thumbnail` on the *filename*, so libvips shrinks on load rather than
        // decoding the whole frame first. It downcasts 16-bit to 8-bit, which is
        // exactly right here and exactly wrong for the print - hence the crop
        // never going through it.
        let image = VipsImage::thumbnail_with_opts(
            &from.to_string_lossy(),
            max_px,
            VOption::new()
                // A box fit: both dimensions capped, so a portrait photo is not
                // taller than a landscape one is wide.
                .set("height", max_px)
                // Never enlarge. A photo smaller than the box is copied at its
                // own size.
                .set("size", Size::Down as i32)
                .set("export_profile", "srgb"),
        )
        .map_err(vips::failed)?;

        image
            .jpegsave_with_opts(
                &temp.to_string_lossy(),
                VOption::new()
                    .set("Q", self.config.preview_quality)
                    .set("optimize_coding", true),
            )
            .map_err(vips::failed)?;

        // Renamed into place, so the webview never sees a half-written JPEG -
        // and within one directory, so the rename is atomic.
        match std::fs::rename(&temp, &target) {
            Ok(()) => Ok(target),
            Err(error) => {
                let _ = std::fs::remove_file(&temp);
                Err(Problem::PhotoUnusable {
                    detail: format!("could not rename {}: {error}", temp.display()),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_kinds_are_named_on_disk() {
        let preview = Preview::new(PipelineConfig::default(), PathBuf::from("/previews"));

        assert_eq!(preview.path_for("abc123", Kind::Thumb), Path::new("/previews/abc123.thumb.jpg"));
        assert_eq!(
            preview.path_for("abc123", Kind::Overlay),
            Path::new("/previews/abc123.overlay.jpg")
        );
        assert_eq!(
            preview.path_for("abc123", Kind::Source),
            Path::new("/previews/abc123.source.jpg")
        );
    }
}
