//! One record per photo, and the index that outlives the session.
//!
//! This is the part that supersedes CLAUDE.md's "no state, no queue, no daemon
//! bookkeeping". That held while snappr was a headless pipeline; a filmstrip that
//! survives a restart cannot be stateless, and at an event a crash halfway through
//! should not lose what has already been printed.
//!
//! The index is the app's own bookkeeping and never authoritative about the disk:
//! the overlay PNG beside the original is still the real artefact, and a record
//! whose PNG has been deleted behind the app's back is dropped rather than shown as
//! a broken cell.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::problem::{detail, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Processing,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Previews {
    pub thumb: PathBuf,
    pub overlay: PathBuf,
    pub source: PathBuf,
}

/// What the camera said about the photo, as `Snappr::Exif` read it off the source.
///
/// Display data only: nothing here reaches the crop, the layout or the pixels. The
/// values arrive already formatted for a person to read ("f/1.8", "1/120s", "6.8mm"),
/// because the tag-by-tag knowledge of how to spell them belongs next to the tags.
///
/// Every field is independently optional, and cameras vary wildly in what they write -
/// a screenshot or a scan carries none of them, which is why the whole struct is an
/// `Option` on `Photo` rather than a default-empty one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Exif {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lens: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focal_length: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focal_length_35mm: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aperture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shutter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iso: Option<u32>,
    /// ISO 8601, and naive when the camera recorded no zone - EXIF keeps the offset in
    /// a separate tag that plenty of cameras never write.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taken_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Photo {
    /// The source path, which is also the identity: one photo on disk, one record.
    pub source: PathBuf,
    pub name: String,
    pub status: Status,
    pub added_at: u64,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previews: Option<Previews>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orientation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// Absent on every record written before this field existed, and on every photo
    /// that carries no camera tags. Both are ordinary, so this is `Option` rather than
    /// a default-empty struct: an index.json from an older build must keep loading, or
    /// `Index::load` drops the operator's whole session on the first launch after an
    /// update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exif: Option<Exif>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Photo {
    pub fn queued(source: PathBuf) -> Self {
        let name = source
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("photo")
            .to_string();
        Self {
            source,
            name,
            status: Status::Queued,
            added_at: now(),
            destination: None,
            previews: None,
            orientation: None,
            output_width: None,
            output_height: None,
            depth: None,
            bytes: None,
            exif: None,
            error: None,
            warnings: Vec::new(),
        }
    }

    /// Fills the record in from a successful `overlay` response.
    pub fn complete(&mut self, frame: &Value) {
        let str_at = |key: &str| frame.get(key).and_then(Value::as_str).map(PathBuf::from);
        let u32_at = |key: &str| frame.get(key).and_then(Value::as_u64).map(|v| v as u32);

        self.status = Status::Done;
        self.destination = str_at("destination");
        self.orientation = frame.get("orientation").and_then(Value::as_str).map(String::from);
        self.output_width = u32_at("output_width");
        self.output_height = u32_at("output_height");
        self.depth = frame.get("depth").and_then(Value::as_u64).map(|v| v as u8);
        self.bytes = frame.get("bytes").and_then(Value::as_u64);
        // Ruby omits the key rather than sending an empty object, so this is None for a
        // photo with no camera tags. Unparseable EXIF is not worth failing the photo
        // over either - the print is what matters and it is already written.
        self.exif = frame
            .get("exif")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok());
        self.error = None;

        self.warnings = frame
            .get("warnings")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|w| w.get("message").and_then(Value::as_str).map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        self.previews = frame.get("previews").and_then(|p| {
            Some(Previews {
                thumb: PathBuf::from(p.get("thumb")?.as_str()?),
                overlay: PathBuf::from(p.get("overlay")?.as_str()?),
                source: PathBuf::from(p.get("source")?.as_str()?),
            })
        });
    }

    pub fn fail(&mut self, error: String) {
        self.status = Status::Failed;
        self.error = Some(error);
    }

    /// The files this record owns outright, and which therefore go when it does.
    ///
    /// That is the three preview JPEGs and nothing else. They live in the app's own
    /// data directory and exist only to be drawn in the window, so they are snappr's
    /// to delete. The source photo and the overlay PNG beside it are not: they sit
    /// in the operator's folder, and CLAUDE.md's "snappr never moves or deletes
    /// anything" covers both. Clearing a photo is about the filmstrip, not about
    /// the print.
    pub fn owned_previews(&self) -> Vec<PathBuf> {
        self.previews
            .iter()
            .flat_map(|p| [p.thumb.clone(), p.overlay.clone(), p.source.clone()])
            .collect()
    }

    /// A finished record is only usable if what it points at still exists. Previews
    /// alone are enough for the UI; the overlay PNG is what print and share need.
    fn intact(&self) -> bool {
        match self.status {
            Status::Done => {
                self.destination.as_deref().is_some_and(Path::exists)
                    && self.previews.as_ref().is_some_and(|p| p.thumb.exists())
            }
            // A record that was mid-flight when the app died is not worth keeping:
            // the source is still on disk, so it will be picked up again.
            Status::Queued | Status::Processing => false,
            Status::Failed => self.source.exists(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Index {
    photos: Vec<Photo>,
    /// The source path of the cell the operator was last on, so a relaunch reopens
    /// it. Part of the index rather than of `Settings` because it is only meaningful
    /// against these records, and is dropped with them.
    selected: Option<PathBuf>,
}

/// What actually goes into `index.json`.
///
/// The file used to be a bare array of photos, and there are indexes on disk in that
/// shape, so the untagged `Legacy` arm keeps them loading. Everything written from
/// here on is the object.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Stored {
    Current {
        photos: Vec<Photo>,
        #[serde(default)]
        selected: Option<PathBuf>,
    },
    Legacy(Vec<Photo>),
}

impl From<Stored> for (Vec<Photo>, Option<PathBuf>) {
    fn from(stored: Stored) -> Self {
        match stored {
            Stored::Current { photos, selected } => (photos, selected),
            Stored::Legacy(photos) => (photos, None),
        }
    }
}

impl Index {
    pub fn path(app: &AppHandle) -> Result<PathBuf, String> {
        Ok(Self::data_dir(app)?.join("index.json"))
    }

    pub fn preview_dir(app: &AppHandle) -> Result<PathBuf, String> {
        let dir = Self::data_dir(app)?.join("previews");
        std::fs::create_dir_all(&dir)
            .map_err(|e| Problem::CannotRememberPhotos { detail: format!("{}: {e}", dir.display()) }.say())?;
        Ok(dir)
    }

    fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|e| Problem::CannotRememberPhotos { detail: detail(e) }.say())?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| Problem::CannotRememberPhotos { detail: format!("{}: {e}", dir.display()) }.say())?;
        Ok(dir)
    }

    /// Loads the index, dropping anything that no longer matches the disk.
    pub fn load(app: &AppHandle) -> Self {
        let Ok(path) = Self::path(app) else {
            return Self::default();
        };
        let (photos, selected): (Vec<Photo>, Option<PathBuf>) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Stored>(&raw).ok())
            .map(Into::into)
            .unwrap_or_default();

        let total = photos.len();
        let photos: Vec<Photo> = photos.into_iter().filter(Photo::intact).collect();
        if total != photos.len() {
            log::info!("dropped {} index entries whose files are gone", total - photos.len());
        }

        // A selection that did not survive the intactness filter is no selection.
        let selected = selected.filter(|source| photos.iter().any(|p| &p.source == source));
        Self { photos, selected }
    }

    pub fn save(&self, app: &AppHandle) -> Result<(), String> {
        let path = Self::path(app)?;
        let stored = Stored::Current {
            photos: self.photos.clone(),
            selected: self.selected.clone(),
        };
        let raw = serde_json::to_string_pretty(&stored)
            .map_err(|e| Problem::CannotRememberPhotos { detail: detail(e) }.say())?;
        std::fs::write(&path, raw)
            .map_err(|e| Problem::CannotRememberPhotos { detail: format!("{}: {e}", path.display()) }.say())
    }

    pub fn all(&self) -> &[Photo] {
        &self.photos
    }

    pub fn selected(&self) -> Option<&PathBuf> {
        self.selected.as_ref()
    }

    /// Records the selection, reporting whether it actually changed so the caller
    /// can skip a write. The frontend re-sends the value it was just given on
    /// launch, and that should not cost a rewrite of the index.
    pub fn select(&mut self, source: PathBuf) -> bool {
        if self.selected.as_ref() == Some(&source) {
            return false;
        }
        self.selected = Some(source);
        true
    }

    /// Inserts or replaces by source path, so re-processing a photo updates its
    /// existing cell rather than adding a second one.
    pub fn upsert(&mut self, photo: Photo) {
        match self.photos.iter_mut().find(|p| p.source == photo.source) {
            Some(existing) => *existing = photo,
            None => self.photos.push(photo),
        }
    }

    /// Drops one record and hands it back, so the caller can clean up the files it
    /// owned. `None` means it was already gone, which is not an error: two clears of
    /// the same cell should not fail the second one.
    pub fn remove(&mut self, source: &Path) -> Option<Photo> {
        let at = self.photos.iter().position(|p| p.source == source)?;
        if self.selected.as_deref() == Some(source) {
            self.selected = None;
        }
        Some(self.photos.remove(at))
    }

    /// Empties the index, handing back everything that was in it.
    pub fn drain(&mut self) -> Vec<Photo> {
        self.selected = None;
        std::mem::take(&mut self.photos)
    }

    pub fn get(&self, source: &Path) -> Option<&Photo> {
        self.photos.iter().find(|p| p.source == source)
    }

    pub fn update<F: FnOnce(&mut Photo)>(&mut self, source: &Path, change: F) -> Option<Photo> {
        let photo = self.photos.iter_mut().find(|p| p.source == source)?;
        change(photo);
        Some(photo.clone())
    }
}
