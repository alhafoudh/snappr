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
use tauri::{AppHandle, Manager};

use crate::problem::{detail, Payload, Problem};

/// The three display JPEGs, as the pipeline names them.
///
/// Re-exported rather than redeclared: this shape crossed a process boundary as
/// JSON once, and there is no boundary left for it to drift across.
pub use snappr_core::preview::Previews;

/// What the camera said, as the pipeline read it off the source header.
pub use snappr_core::exif::Exif;

use snappr_core::config::Orientation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Status {
    Queued,
    Processing,
    Done,
    Failed,
}

// `Previews` and `Exif` used to be declared here, field for field, mirroring
// structs on the Ruby side that crossed the wire as JSON. They are re-exported
// from `snappr-core` above instead: display data the pipeline produces, and one
// declaration is now enough. Note what `Exif` being the pipeline's own type
// buys - the values arrive already formatted for a person to read ("f/1.8",
// "1/120s", "6.8mm"), because the tag-by-tag knowledge of how to spell them
// belongs next to the tags.

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields)]
pub struct Photo {
    /// The source path, which is also the identity: one photo on disk, one record.
    pub source: PathBuf,
    pub name: String,
    pub status: Status,
    /// `number`, not `bigint`: ts-rs maps `u64` to `bigint` by default, and the
    /// frontend does arithmetic on this to render "added 3 minutes ago". A unix
    /// timestamp is nowhere near `Number.MAX_SAFE_INTEGER`.
    #[ts(type = "number")]
    pub added_at: u64,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previews: Option<Previews>,
    /// **The enum, not a string.** This was `Option<String>` filled from whatever
    /// the child process had put in a JSON field, while the frontend narrowed it
    /// to `"landscape" | "portrait"` and nothing checked that it was either. One
    /// place in Rust then compared it against the literal `"landscape"`.
    ///
    /// An old `index.json` still loads: serde reads the same two lower-case
    /// spellings it wrote.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orientation: Option<Orientation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Same reason: the info line divides this by 1024 twice to show megabytes.
    #[ts(type = "number")]
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
    /// When the operator last sent this photo to the printer, in Unix seconds, or
    /// `None` if they never have. Set when the print dialog is *opened*, not when
    /// it is confirmed - `print_photo` says why, and the cell's context menu is
    /// the correction for a print that was then cancelled.
    ///
    /// `#[serde(default)]` for the same reason as `exif` above, and for a second
    /// one: **both serde attributes together are what make this optional in
    /// TypeScript.** `#[ts(type = ...)]` overrides the field's type, and ts-rs
    /// skips the struct's `optional_fields` for any field that does
    /// (`ts-rs-macros/src/optional.rs`), falling back to "optional if it may be
    /// omitted *and* has a default". `bytes` above has only the first half, which
    /// is why it is typed as required despite being an `Option`. Dropping the
    /// `default` here would silently do the same to this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "number")]
    pub printed_at: Option<u64>,
    /// Whether the photo has arrived and not been looked at yet - the blue dot
    /// before the filename in its cell. Set on every new record and cleared by
    /// selecting it, in `set_selected`.
    ///
    /// **Negative on purpose**, and the whole of the upgrade story: `#[serde
    /// (default)]` on a `bool` is `false`, so a record written before this field
    /// existed loads as *seen* and an update is quiet rather than a strip full of
    /// dots. Spelling it `seen` would need a custom serde default returning true to
    /// get the same thing, which is the same behaviour spelled less obviously.
    ///
    /// No `skip_serializing_if`, so this stays a **required** `boolean` in
    /// `src/bindings/Photo.ts`: it is the *pair* of attributes that makes a field
    /// optional there, as `printed_at` above explains and `warnings` demonstrates.
    #[serde(default)]
    pub unseen: bool,
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
            printed_at: None,
            // Unlooked-at is the safe default for a record nobody has decided about
            // yet - a dot the operator does not need is a smaller wrong than a photo
            // that arrived without saying so. `process` overrides it per arrival,
            // and `upsert` keeps it across a reprocess.
            unseen: true,
        }
    }

    /// Fills the record in from a finished composite.
    ///
    /// **Typed, by move.** This used to take the raw JSON frame the Ruby child
    /// sent and pick fields out of it by string key, which meant a renamed field
    /// silently became `None` and a malformed `exif` was silently dropped - both
    /// on the path where the print had already been written. The pipeline hands
    /// over its own structs now, so neither is expressible.
    pub fn complete(&mut self, outcome: snappr_core::overlay::Outcome, previews: Previews) {
        self.status = Status::Done;
        self.destination = Some(outcome.destination);
        self.orientation = Some(outcome.orientation);
        self.output_width = Some(outcome.output_width as u32);
        self.output_height = Some(outcome.output_height as u32);
        self.depth = Some(outcome.depth);
        self.bytes = Some(outcome.bytes);
        // `None` for a photo that carries no camera tags, which is ordinary.
        self.exif = outcome.exif;
        self.error = None;
        // The operator's half of each warning. The `kind` is dropped here on
        // purpose: the cell shows a sentence, and anything that wants to branch on
        // the code reads it from the payload instead.
        self.warnings = outcome.warnings.into_iter().map(|warning| warning.message).collect();
        self.previews = Some(previews);
    }

    pub fn fail(&mut self, error: Payload) {
        self.status = Status::Failed;
        self.error = Some(error.message);
    }

    /// Sets or clears the print mark.
    ///
    /// The timestamp is when it was *marked*, which for a print made through the
    /// app is when the dialog opened rather than when paper came out. Nothing
    /// downstream reads it as anything finer than "this one has been printed".
    pub fn printed(&mut self, printed: bool) {
        self.printed_at = printed.then(now);
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
    pub fn path(app: &AppHandle) -> Result<PathBuf, Payload> {
        Ok(Self::data_dir(app)?.join("index.json"))
    }

    pub fn preview_dir(app: &AppHandle) -> Result<PathBuf, Payload> {
        let dir = Self::data_dir(app)?.join("previews");
        std::fs::create_dir_all(&dir)
            .map_err(|e| Problem::CannotRememberPhotos { detail: format!("{}: {e}", dir.display()) }.say())?;
        Ok(dir)
    }

    fn data_dir(app: &AppHandle) -> Result<PathBuf, Payload> {
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

    pub fn save(&self, app: &AppHandle) -> Result<(), Payload> {
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

    /// Inserts or replaces by source path - so re-processing a photo updates its
    /// existing cell rather than adding a second one - and hands back what is now
    /// stored.
    ///
    /// **The operator's own facts survive the replacement, and that is not a
    /// nicety.** `added_at`, `printed_at` and `unseen` belong to the index; the rest
    /// of the record belongs to the pipeline. `process` holds its `Photo` across a
    /// second of compositing and publishes that same struct at the end, so an
    /// incoming record is always a second stale, and without this it writes back
    /// whatever those three fields were when the photo was queued.
    ///
    /// That is a lost update, and it shipped once: selecting a photo while it
    /// composited cleared its blue dot, and then the finished record put the dot
    /// straight back and left it there. Anything the operator does to a cell during
    /// its own second of work is in this window.
    ///
    /// Preserving them here rather than in the caller is what makes the ownership
    /// one rule in one place. `process` used to carry the three forward by hand and
    /// nothing failed to compile when a field was left out of that tuple.
    pub fn upsert(&mut self, photo: Photo) -> Photo {
        match self.photos.iter_mut().find(|p| p.source == photo.source) {
            Some(existing) => {
                let added_at = existing.added_at;
                let printed_at = existing.printed_at;
                let unseen = existing.unseen;

                *existing = photo;
                existing.added_at = added_at;
                existing.printed_at = printed_at;
                existing.unseen = unseen;

                existing.clone()
            }
            None => {
                self.photos.push(photo.clone());
                photo
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A record written before `printed_at` existed must still load.
    ///
    /// This is the failure `Index::load` turns into a lost session: it answers any
    /// parse error by falling back to `default()`, so one field that cannot read
    /// its own history costs the operator the whole filmstrip on the first launch
    /// after an update. `Photo::exif` carries the same guarantee for the same
    /// reason.
    ///
    /// Deserialized directly rather than through `Index::load`, which needs an
    /// `AppHandle` and a real data directory.
    #[test]
    fn a_record_from_before_the_print_mark_still_loads() {
        let legacy = r#"{
            "source": "/Users/someone/Downloads/IMG_1234.heic",
            "name": "IMG_1234.heic",
            "status": "done",
            "added_at": 1772534400,
            "warnings": []
        }"#;

        let photo: Photo = serde_json::from_str(legacy).expect("an older record must still load");
        assert_eq!(photo.printed_at, None);
        assert_eq!(photo.status, Status::Done);
    }

    /// And the mark round-trips, so a printed photo is still printed after a
    /// relaunch.
    #[test]
    fn the_print_mark_survives_a_round_trip() {
        let mut photo = Photo::queued(PathBuf::from("/Users/someone/Downloads/IMG_1234.heic"));
        assert_eq!(photo.printed_at, None);

        photo.printed(true);
        let at = photo.printed_at.expect("marking must set a timestamp");

        let raw = serde_json::to_string(&photo).unwrap();
        let back: Photo = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.printed_at, Some(at));

        // Clearing it drops the key rather than writing a null, which is what the
        // `skip_serializing_if` is there for.
        photo.printed(false);
        assert_eq!(photo.printed_at, None);
        assert!(!serde_json::to_string(&photo).unwrap().contains("printed_at"));
    }

    /// A record written before `unseen` existed must load as *seen*.
    ///
    /// Two failures in one: the lost session `a_record_from_before_the_print_mark
    /// _still_loads` describes, and - because the field is spelled negatively so
    /// that `#[serde(default)]` means "seen" - the first launch after an update
    /// coming up with a blue dot on every cell in the strip.
    #[test]
    fn a_record_from_before_the_seen_mark_loads_as_seen() {
        let legacy = r#"{
            "source": "/Users/someone/Downloads/IMG_1234.heic",
            "name": "IMG_1234.heic",
            "status": "done",
            "added_at": 1772534400,
            "warnings": []
        }"#;

        let photo: Photo = serde_json::from_str(legacy).expect("an older record must still load");
        assert!(!photo.unseen, "a record from before the mark must not arrive dotted");
    }

    /// The finished composite must not write the operator's own facts back to what
    /// they were when the photo was queued.
    ///
    /// This is the lost update `upsert` exists to stop, and it shipped: `process`
    /// holds its record across a second of compositing, so selecting a photo while
    /// it worked cleared the blue dot and the finished record put it straight back.
    /// The same window covers the print mark and the arrival time.
    #[test]
    fn upsert_keeps_the_facts_the_pipeline_does_not_own() {
        let source = PathBuf::from("/Users/someone/Downloads/IMG_1234.heic");
        let mut index = Index::default();
        index.upsert(Photo::queued(source.clone()));

        // The operator looks at the cell and prints it while it is still working.
        index.update(&source, |photo| {
            photo.unseen = false;
            photo.printed(true);
        });
        let added_at = index.get(&source).expect("just inserted").added_at;

        // A second later the pipeline publishes the record it has been holding,
        // which still says unseen, unprinted, and carries its own arrival time.
        let mut finished = Photo::queued(source.clone());
        finished.status = Status::Done;
        finished.added_at = added_at + 900;

        let stored = index.upsert(finished);
        assert!(!stored.unseen, "a finished composite must not re-dot the photo");
        assert!(stored.printed_at.is_some(), "nor un-print it");
        assert_eq!(stored.added_at, added_at, "nor move it along the strip");
        assert_eq!(stored.status, Status::Done, "but the pipeline's own fields do land");
    }

    /// And the mark round-trips, so a photo the operator never got to is still
    /// waiting for them after a relaunch. That is the whole point of persisting it.
    #[test]
    fn the_seen_mark_survives_a_round_trip() {
        let mut photo = Photo::queued(PathBuf::from("/Users/someone/Downloads/IMG_1234.heic"));
        assert!(photo.unseen, "a new record starts unlooked-at");

        let back: Photo = serde_json::from_str(&serde_json::to_string(&photo).unwrap()).unwrap();
        assert!(back.unseen);

        photo.unseen = false;
        let back: Photo = serde_json::from_str(&serde_json::to_string(&photo).unwrap()).unwrap();
        assert!(!back.unseen);
    }
}
