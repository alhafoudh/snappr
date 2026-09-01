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
}

impl Index {
    pub fn path(app: &AppHandle) -> Result<PathBuf, String> {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("cannot resolve the data directory: {e}"))?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        Ok(dir.join("index.json"))
    }

    pub fn preview_dir(app: &AppHandle) -> Result<PathBuf, String> {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("cannot resolve the data directory: {e}"))?
            .join("previews");
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        Ok(dir)
    }

    /// Loads the index, dropping anything that no longer matches the disk.
    pub fn load(app: &AppHandle) -> Self {
        let Ok(path) = Self::path(app) else {
            return Self::default();
        };
        let photos: Vec<Photo> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();

        let total = photos.len();
        let photos: Vec<Photo> = photos.into_iter().filter(Photo::intact).collect();
        if total != photos.len() {
            log::info!("dropped {} index entries whose files are gone", total - photos.len());
        }
        Self { photos }
    }

    pub fn save(&self, app: &AppHandle) -> Result<(), String> {
        let path = Self::path(app)?;
        let raw = serde_json::to_string_pretty(&self.photos).map_err(|e| e.to_string())?;
        std::fs::write(&path, raw).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    pub fn all(&self) -> &[Photo] {
        &self.photos
    }

    /// Inserts or replaces by source path, so re-processing a photo updates its
    /// existing cell rather than adding a second one.
    pub fn upsert(&mut self, photo: Photo) {
        match self.photos.iter_mut().find(|p| p.source == photo.source) {
            Some(existing) => *existing = photo,
            None => self.photos.push(photo),
        }
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
