//! Operator settings, and the slice of them the Ruby side is told about.
//!
//! Plain serde into `app_config_dir()` rather than tauri-plugin-store: the Rust
//! shell owns settings, the fs plugin's Rust side offers only scope methods anyway,
//! and this way the file is human-readable JSON with no IPC round-trip and no
//! capability to grant.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager};

use crate::problem::{detail, Problem};

/// The two overlay slots, which are also the two crop shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    Landscape,
    Portrait,
}

impl Orientation {
    /// The sample snappr ships for this slot, relative to the resource directory,
    /// or `None` when it ships none.
    ///
    /// Only landscape has one, deliberately. One sample is enough to show what an
    /// overlay is and where the `#FF00FF` window goes, and a second drawn to a
    /// different design would suggest the two slots are a matched pair that has to
    /// be kept in step.
    fn sample(self) -> Option<&'static str> {
        match self {
            Self::Landscape => Some("sample/overlay-landscape.svg"),
            Self::Portrait => Option::None,
        }
    }
}

/// What an orientation composites over the crop.
///
/// `Sample` is whatever snappr ships for that slot - which for portrait is
/// nothing, so a portrait `Sample` prints exactly like `None`. That is the
/// variant's meaning rather than a hole in it: it keeps the type independent of
/// which field it sits on, and a portrait sample drawn later is then picked up by
/// every install that never chose.
///
/// `Custom` remembers the path rather than copying the file, so re-exporting a
/// design over the same path takes effect on the next photo with no re-picking.
///
/// `None` is a legitimate setting, not a broken one: the photo is still cropped to
/// the print ratio, with nothing drawn over it.
///
/// Adjacently tagged, so the JSON is `{"kind":"sample"}` / `{"kind":"none"}` /
/// `{"kind":"custom","path":"/..."}`. Not `untagged`: that turns every parse error
/// into "data did not match any variant", which is the one message you cannot act
/// on from a log line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "path", rename_all = "lowercase")]
pub enum Overlay {
    Sample,
    None,
    Custom(PathBuf),
}

impl<'de> Deserialize<'de> for Overlay {
    /// Reads every shape this key has ever had on disk.
    ///
    /// Not politeness. `Settings::load` falls back to *all* defaults when the file
    /// will not parse, so a field that cannot read its own history does not cost
    /// the operator their overlay - it costs them their watch folder, their
    /// printer and their paper name too. Only this field's shape has ever changed,
    /// so only this field needs the treatment.
    ///
    /// Going through `Value` first is what makes try-this-then-that possible at
    /// all: a `Deserializer` cannot be rewound once an inner call has consumed a
    /// token. Settings are only ever read from JSON, so the cost is one small
    /// allocation per slot, once, at launch.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;

        // The current shape spelled out again, so this impl can defer to a derive
        // rather than recursing into itself.
        #[derive(Deserialize)]
        #[serde(tag = "kind", content = "path", rename_all = "lowercase")]
        enum Tagged {
            Sample,
            None,
            Custom(PathBuf),
        }

        match Value::deserialize(deserializer)? {
            // What the Option<PathBuf> build wrote for an empty slot - which was
            // also what it wrote for a fresh install, because empty was the
            // default then. "Never chose" now means the sample, so an untouched
            // install upgrades into showing something rather than staying blank,
            // which is the whole point of shipping one. An operator who
            // deliberately cleared landscape gets the sample back once; on disk
            // that is the same two bytes as never having chosen, so there is no
            // third answer available here, and it is one click to undo.
            Value::Null => Ok(Self::Sample),
            Value::String(path) if !path.is_empty() => Ok(Self::Custom(path.into())),
            // Never written by any build, but "" and null mean the same thing to
            // `Config#configured_overlay_svg`, so honour that reading.
            Value::String(_) => Ok(Self::None),
            value @ Value::Object(_) => serde_json::from_value::<Tagged>(value)
                .map(|tagged| match tagged {
                    Tagged::Sample => Self::Sample,
                    Tagged::None => Self::None,
                    Tagged::Custom(path) => Self::Custom(path),
                })
                .map_err(D::Error::custom),
            other => Err(D::Error::custom(format!(
                "an overlay slot is a path, null, or {{\"kind\": ...}}, not {other}"
            ))),
        }
    }
}

impl Overlay {
    /// The file to composite for this slot, or `None` for a plain crop.
    ///
    /// A `Custom` path is passed through *without* checking it exists, on purpose:
    /// `Config#overlay_svg` already degrades a missing overlay to none with a
    /// warning that reaches the filmstrip, and that warning is the point - the
    /// operator moved the file and can move it back.
    ///
    /// A missing `Sample` gets the opposite treatment and is silently nothing,
    /// because there is no action behind it. The operator did not choose that path
    /// and cannot fix it, so a per-photo warning would be noise on every print. In
    /// a real build it also cannot happen: `bundle.resources` fails the build when
    /// its glob matches no file.
    ///
    /// Resolved on demand rather than cached. `resource_dir()` is a few string
    /// operations, the answer cannot change while the app runs, and every caller
    /// already holds the `AppHandle` it needs.
    pub fn resolve(&self, app: &AppHandle, orientation: Orientation) -> Option<PathBuf> {
        match self {
            Self::None => Option::None,
            Self::Custom(path) => Some(path.clone()),
            // `resolve` only joins and normalises, so the file has to be checked
            // for separately.
            Self::Sample => orientation.sample().and_then(|name| {
                app.path()
                    .resolve(name, BaseDirectory::Resource)
                    .ok()
                    .filter(|path| path.is_file())
            }),
        }
    }
}

fn default_watch_dir() -> PathBuf {
    dirs_home().join("Downloads")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

fn default_extensions() -> Vec<String> {
    ["heic", "heif", "jpg", "jpeg"].map(String::from).to_vec()
}

fn default_output_suffix() -> String {
    ".overlay.png".into()
}

fn default_background() -> String {
    "#ffffff".into()
}

fn default_png_compression() -> u8 {
    6
}

fn default_short_mm() -> f64 {
    100.0
}

fn default_long_mm() -> f64 {
    148.0
}

fn default_preview_max_px() -> u32 {
    2000
}

fn default_preview_thumb_px() -> u32 {
    400
}

fn default_preview_quality() -> u8 {
    80
}

fn default_stability_checks() -> u32 {
    2
}

fn default_stability_timeout_secs() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub watch_dir: PathBuf,
    /// The bar's watch toggle. Persisted so a kiosk comes back the way it was
    /// left; the toggle always shows which, so there is no hidden state.
    pub watch_enabled: bool,
    /// What each orientation composites, picked by the operator from the bar.
    ///
    /// Landscape starts on the sample that ships with the app; portrait starts on
    /// nothing, because no portrait sample is drawn. snappr ships **no branding** -
    /// `sample/` holds one worked example an operator replaces, and the files under
    /// `share/` are test material that must never be listed in `bundle.resources`.
    ///
    /// `alias` reads the key the `Option<PathBuf>` build wrote, and the field name
    /// is what gets written back, so the file self-migrates on the first save. The
    /// old name was a lie anyway: the value is no longer a path.
    #[serde(alias = "overlay_svg_landscape")]
    pub overlay_landscape: Overlay,
    #[serde(alias = "overlay_svg_portrait")]
    pub overlay_portrait: Overlay,
    pub extensions: Vec<String>,
    /// Dual-purpose: it names the output file *and* is the watcher's self-output
    /// guard, so it has to match what the Ruby side appends.
    pub output_suffix: String,
    pub background: String,
    pub png_compression: u8,
    pub print_short_mm: f64,
    pub print_long_mm: f64,
    pub preview_max_px: u32,
    pub preview_thumb_px: u32,
    pub preview_quality: u8,
    pub stability_checks: u32,
    pub stability_timeout_secs: u64,
    /// Empty means the system default printer.
    pub printer: String,
    /// The driver's PPD keyword for the paper to print on - NOT the label the
    /// dialog shows. `Postcard.Fullbleed` is what the Canon SELPHY calls the paper
    /// macOS displays as "Postcard Borderless"; plain `Postcard` has the same
    /// dimensions but ~3 mm margins. Empty means let the driver pick, which on the
    /// SELPHY is the bordered one.
    pub print_paper_name: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            watch_dir: default_watch_dir(),
            watch_enabled: true,
            overlay_landscape: Overlay::Sample,
            overlay_portrait: Overlay::None,
            extensions: default_extensions(),
            output_suffix: default_output_suffix(),
            background: default_background(),
            png_compression: default_png_compression(),
            print_short_mm: default_short_mm(),
            print_long_mm: default_long_mm(),
            preview_max_px: default_preview_max_px(),
            preview_thumb_px: default_preview_thumb_px(),
            preview_quality: default_preview_quality(),
            stability_checks: default_stability_checks(),
            stability_timeout_secs: default_stability_timeout_secs(),
            printer: String::new(),
            print_paper_name: "Postcard.Fullbleed".into(),
        }
    }
}

impl Settings {
    pub fn stability_timeout(&self) -> Duration {
        Duration::from_secs(self.stability_timeout_secs)
    }

    pub fn path(app: &AppHandle) -> Result<PathBuf, String> {
        let dir = app
            .path()
            .app_config_dir()
            .map_err(|e| Problem::CannotSaveSettings { detail: detail(e) }.say())?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| Problem::CannotSaveSettings { detail: format!("{}: {e}", dir.display()) }.say())?;
        Ok(dir.join("settings.json"))
    }

    /// A missing or unreadable file is not an error: it just means defaults. A
    /// corrupt one is worth saying out loud, but still not worth refusing to start
    /// over - a kiosk that will not open because of a stray character is worse.
    ///
    /// The sentence is handed back rather than raised from here, because this runs
    /// in `.setup()` before the window exists. The caller owns the window and
    /// decides when to say it.
    pub fn load(app: &AppHandle) -> (Self, Option<String>) {
        let Ok(path) = Self::path(app) else {
            return (Self::default(), None);
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            // No file at all is the ordinary first launch, and silent is right.
            return (Self::default(), None);
        };
        match serde_json::from_str(&raw) {
            Ok(settings) => (settings, None),
            Err(e) => {
                // Move it aside first. The next save() would otherwise overwrite
                // it - and the watch toggle saves on its very first click - so
                // without this the message announces a loss that is already
                // unrecoverable. This is the one file snappr renames, and it is
                // in snappr's own support folder, not the operator's.
                let kept = path.with_extension("json.broken");
                let note = match std::fs::rename(&path, &kept) {
                    Ok(()) => format!("{e}; moved aside to {}", kept.display()),
                    Err(rename) => format!("{e}; could not move it aside either: {rename}"),
                };
                let said = Problem::SettingsUnreadable { path: &path, detail: note }.warn();
                (Self::default(), Some(said))
            }
        }
    }

    pub fn save(&self, app: &AppHandle) -> Result<(), String> {
        let path = Self::path(app)?;
        let raw = serde_json::to_string_pretty(self)
            .map_err(|e| Problem::CannotSaveSettings { detail: detail(e) }.say())?;
        std::fs::write(&path, raw)
            .map_err(|e| Problem::CannotSaveSettings { detail: format!("{}: {e}", path.display()) }.say())
    }

    /// The keys the pipeline actually reads. Deliberately not the whole struct:
    /// the watch settings belong to this side now, and sending them would only
    /// create a second place for them to drift.
    ///
    /// **`Overlay` never reaches Ruby - only a path or an explicit null does.**
    /// That is what keeps the sample entirely a shell concept: `lib/snappr` sees
    /// exactly what it always saw. It could not do the lookup itself either, since
    /// `SNAPPR_ROOT` is `Resources/snappr` in a bundle and the repo root in
    /// development, and the sample sits *beside* the first but *inside* the second.
    ///
    /// A slot that resolves to nothing travels as an explicit `null` rather than an
    /// absent key. `Server#hello` merges this over `Config::DEFAULTS`, so an absent
    /// key takes Ruby's default - which is nil today, making the two identical, but
    /// this is the side that decides overlays and it should say so outright.
    pub fn pipeline_config(&self, app: &AppHandle) -> Value {
        json!({
            "print": { "short_mm": self.print_short_mm, "long_mm": self.print_long_mm },
            "overlay_svg": {
                "landscape": self.overlay_landscape.resolve(app, Orientation::Landscape),
                "portrait": self.overlay_portrait.resolve(app, Orientation::Portrait),
            },
            "output_suffix": self.output_suffix,
            "background": self.background,
            "png_compression": self.png_compression,
            "preview": {
                "max_px": self.preview_max_px,
                "thumb_px": self.preview_thumb_px,
                "quality": self.preview_quality,
            },
        })
    }
}

/// The overlay slot is the only field whose shape has ever changed on disk, and
/// `Settings::load` answers any parse failure by discarding the whole file. So the
/// thing worth testing is not that the new shape round-trips - it is that none of
/// the old ones can take the watch folder down with them.
#[cfg(test)]
mod tests {
    use super::*;

    fn settings(json: &str) -> Settings {
        serde_json::from_str(json).expect("settings must parse")
    }

    #[test]
    fn an_absent_slot_takes_the_default() {
        let parsed = settings(r#"{"watch_dir": "/tmp/drop"}"#);
        assert_eq!(parsed.overlay_landscape, Overlay::Sample);
        assert_eq!(parsed.overlay_portrait, Overlay::None);
    }

    /// The shape the `Option<PathBuf>` build wrote, under the key it wrote it as.
    /// Null was both "cleared" and "never chose"; the sample is the right reading
    /// of the second, and the first is one click to undo.
    #[test]
    fn a_legacy_null_slot_becomes_the_sample_and_keeps_the_rest() {
        let parsed = settings(
            r#"{"watch_dir": "/tmp/drop", "printer": "SELPHY",
                "overlay_svg_landscape": null, "overlay_svg_portrait": null}"#,
        );
        assert_eq!(parsed.overlay_landscape, Overlay::Sample);
        assert_eq!(parsed.watch_dir, PathBuf::from("/tmp/drop"));
        assert_eq!(parsed.printer, "SELPHY");
    }

    #[test]
    fn a_legacy_path_slot_becomes_a_custom_pick() {
        let parsed = settings(r#"{"overlay_svg_landscape": "/brand/wide.svg"}"#);
        assert_eq!(parsed.overlay_landscape, Overlay::Custom("/brand/wide.svg".into()));
    }

    /// A whole file as the `Option<PathBuf>` build actually wrote one, rather than
    /// a two-key fragment. The fragments prove the variant mapping; this proves
    /// nothing *else* in a real document trips the parse and takes the file down
    /// with it - which is the failure that would cost an operator their setup.
    #[test]
    fn a_complete_legacy_file_survives_intact() {
        // r##, not r#: the background value contains `"#`, which would close an
        // r#"..."# string right there.
        let parsed = settings(
            r##"{
                "watch_dir": "/Users/op/Downloads",
                "watch_enabled": true,
                "overlay_svg_landscape": "/Users/op/brand/wide.svg",
                "overlay_svg_portrait": null,
                "extensions": ["heic", "heif", "jpg", "jpeg"],
                "output_suffix": ".overlay.png",
                "background": "#ffffff",
                "png_compression": 6,
                "print_short_mm": 100.0,
                "print_long_mm": 148.0,
                "preview_max_px": 2000,
                "preview_thumb_px": 400,
                "preview_quality": 80,
                "stability_checks": 2,
                "stability_timeout_secs": 30,
                "printer": "",
                "print_paper_name": "Postcard.Fullbleed"
            }"##,
        );

        assert_eq!(parsed.overlay_landscape, Overlay::Custom("/Users/op/brand/wide.svg".into()));
        assert_eq!(parsed.overlay_portrait, Overlay::Sample);
        assert_eq!(parsed.watch_dir, PathBuf::from("/Users/op/Downloads"));
        assert_eq!(parsed.print_paper_name, "Postcard.Fullbleed");
        assert_eq!(parsed.preview_quality, 80);
    }

    #[test]
    fn the_tagged_shape_round_trips() {
        for overlay in [Overlay::Sample, Overlay::None, Overlay::Custom("/b/w.svg".into())] {
            let raw = serde_json::to_string(&overlay).unwrap();
            assert_eq!(serde_json::from_str::<Overlay>(&raw).unwrap(), overlay, "{raw}");
        }
    }

    /// The old key is read and the new one is written, so the file migrates itself
    /// the first time anything is saved.
    #[test]
    fn saving_rewrites_the_slot_under_the_new_key() {
        let parsed = settings(r#"{"overlay_svg_landscape": "/brand/wide.svg"}"#);
        let raw = serde_json::to_string(&parsed).unwrap();
        assert!(raw.contains(r#""overlay_landscape":{"kind":"custom","path":"/brand/wide.svg"}"#), "{raw}");
        assert!(!raw.contains("overlay_svg_landscape"), "the old key must not be written back");
    }

    /// Anything genuinely unreadable still fails, rather than being papered over as
    /// a default - a number here means the file was written by something else.
    #[test]
    fn a_nonsense_slot_is_still_an_error() {
        assert!(serde_json::from_str::<Settings>(r#"{"overlay_landscape": 7}"#).is_err());
    }
}
