//! Operator settings: what the window shows and what the pipeline is handed.
//!
//! Plain serde into `app_config_dir()` rather than tauri-plugin-store: the Rust
//! shell owns settings, the fs plugin's Rust side offers only scope methods anyway,
//! and this way the file is human-readable JSON with no IPC round-trip and no
//! capability to grant.
//!
//! **The numbers here are not a second defaults table.** Everything the pipeline
//! reads defaults to `PipelineConfig::default()` and is handed back as one, so
//! there is no handshake in the middle and nothing to keep in step. What is left
//! in this file is genuinely the shell's: the watch toggle, the overlay library,
//! the panel's collapsed state, the printer and its paper name.

use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
pub use snappr_core::config::Orientation;
use snappr_core::config::PipelineConfig;
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager};

use crate::problem::{detail, Payload, Problem};

/// What the one overlay snappr ships is called wherever it is named. A constant
/// because the panel, the log and the settings file all have to agree - the
/// operator has to recognise the card as the thing Reveal in Finder hands them.
pub const SAMPLE_NAME: &str = "Sample overlay";

/// The sample snappr ships for an orientation, relative to the resource
/// directory, or `None` when it ships none.
///
/// Only landscape has one, deliberately. One sample is enough to show what an
/// overlay is and where the `#FF00FF` window goes, and a second drawn to a
/// different design would suggest the two slots are a matched pair that has to be
/// kept in step.
///
/// A free function rather than a method because `Orientation` belongs to
/// `snappr-core` now, and where a *bundle* keeps its resources is not something
/// the pipeline can be told about - see `Overlay::resolve`.
fn sample_for(orientation: Orientation) -> Option<&'static str> {
    match orientation {
        Orientation::Landscape => Some("sample/overlay-landscape.svg"),
        Orientation::Portrait => None,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(tag = "kind", content = "path", rename_all = "lowercase")]
#[ts(export)]
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
    /// What the panel and the log call this overlay.
    ///
    /// The one place a name is spelled. The frontend used to have its own copy of
    /// this and it is exactly the kind of thing that drifts, so the card carries a
    /// name computed here instead.
    pub fn label(&self) -> String {
        match self {
            Self::Sample => SAMPLE_NAME.to_string(),
            Self::None => "No overlay".to_string(),
            Self::Custom(path) => crate::problem::name_of(path),
        }
    }

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
            Self::Sample => sample_for(orientation).and_then(|name| {
                app.path()
                    .resolve(name, BaseDirectory::Resource)
                    .ok()
                    .filter(|path| path.is_file())
            }),
        }
    }
}

// The twelve `default_*` functions that used to sit here are gone: every one of
// them restated a value that `PipelineConfig::default()` already holds, and
// `Settings::default` now derives from that instead. `#[serde(default)]` on the
// struct is what fills an absent key, and it calls `Settings::default`.

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(default)]
#[ts(export)]
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
    /// Every overlay the operator has added, the shipped sample first. The panel
    /// shows one card per entry; the two slots above say which of them is live for
    /// each orientation.
    ///
    /// **A third shape for the same setting, and deliberately not a third shape for
    /// the same *key*.** `Overlay`'s hand-written `Deserialize` exists because
    /// `load` throws the whole file away on a parse error, so re-spelling
    /// `overlay_landscape` as a list would put the watch folder and the printer at
    /// risk to gain a panel. A new key beside them cannot: `Vec<Overlay>` reuses the
    /// element impl that already reads every historic shape, `#[serde(default)]` on
    /// the struct covers the key being absent, and `normalise` derives the library
    /// an old file implies. Nothing here had to learn any history.
    ///
    /// `Overlay` is `PartialEq`, so the value is the identity - there are no ids to
    /// keep in step, and a slot pointing at a card is just the two being equal.
    pub overlays: Vec<Overlay>,
    /// Whether the overlay panel is showing as a rail rather than a column of
    /// cards. Overlays are set up once and then left alone, so a kiosk should come
    /// back the way it was left rather than giving the photo back 180px on every
    /// launch.
    pub overlays_collapsed: bool,
    pub extensions: Vec<String>,
    /// Dual-purpose: it names the output file *and* is the watcher's self-output
    /// guard. Both readers are now the same code, so they cannot disagree.
    pub output_suffix: String,
    pub background: String,
    pub png_compression: i32,
    pub print_short_mm: f64,
    pub print_long_mm: f64,
    pub preview_max_px: i32,
    pub preview_thumb_px: i32,
    pub preview_quality: i32,
    pub stability_checks: u32,
    #[ts(type = "number")]
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
    /// **Every pipeline value comes from `PipelineConfig::default()`**, which is
    /// the single defaults table. Only the settings that are genuinely the
    /// shell's - the toggle, the library, the printer - are written here.
    fn default() -> Self {
        let pipeline = PipelineConfig::default();
        Self {
            watch_dir: pipeline.watch_dir,
            watch_enabled: true,
            overlay_landscape: Overlay::Sample,
            overlay_portrait: Overlay::None,
            overlays: vec![Overlay::Sample],
            overlays_collapsed: false,
            extensions: pipeline.extensions,
            output_suffix: pipeline.output_suffix,
            background: pipeline.background,
            png_compression: pipeline.png_compression,
            print_short_mm: pipeline.print_short_mm,
            print_long_mm: pipeline.print_long_mm,
            preview_max_px: pipeline.preview_max_px,
            preview_thumb_px: pipeline.preview_thumb_px,
            preview_quality: pipeline.preview_quality,
            stability_checks: pipeline.stability_checks,
            stability_timeout_secs: pipeline.stability_timeout_secs,
            printer: String::new(),
            print_paper_name: "Postcard.Fullbleed".into(),
        }
    }
}

impl Settings {
    /// The slot for an orientation, so callers stop writing the same two-armed
    /// match every time they need one.
    pub fn slot(&self, orientation: Orientation) -> &Overlay {
        match orientation {
            Orientation::Landscape => &self.overlay_landscape,
            Orientation::Portrait => &self.overlay_portrait,
        }
    }

    pub fn slot_mut(&mut self, orientation: Orientation) -> &mut Overlay {
        match orientation {
            Orientation::Landscape => &mut self.overlay_landscape,
            Orientation::Portrait => &mut self.overlay_portrait,
        }
    }

    /// Which orientations this overlay is live for - the two lit toggles on its
    /// card. Empty is the ordinary state for an overlay the operator has added but
    /// not switched on yet.
    pub fn assigned(&self, overlay: &Overlay) -> Vec<Orientation> {
        Orientation::ALL.into_iter().filter(|&o| self.slot(o) == overlay).collect()
    }

    /// Adds an overlay to the library, or reports that it is already there.
    ///
    /// Deliberately does *not* switch it on. Adding and assigning are two gestures
    /// on the card, and an add that silently took over a slot would overwrite a
    /// choice the operator made without being asked to.
    pub fn add_overlay(&mut self, overlay: Overlay) -> bool {
        if self.overlays.contains(&overlay) {
            return false;
        }
        self.overlays.push(overlay);
        true
    }

    /// Removes an overlay and empties any slot that pointed at it.
    ///
    /// The slots have to be cleared here rather than left dangling: a slot holding
    /// an overlay with no card would print branding the panel does not show, which
    /// is the one thing the panel exists to prevent. `Sample` is refused because it
    /// is not the operator's to delete - it lives in the app bundle - and its card
    /// simply offers no Remove.
    pub fn remove_overlay(&mut self, overlay: &Overlay) -> bool {
        if matches!(overlay, Overlay::Sample) {
            return false;
        }
        let before = self.overlays.len();
        self.overlays.retain(|held| held != overlay);
        for orientation in Orientation::ALL {
            if self.slot(orientation) == overlay {
                *self.slot_mut(orientation) = Overlay::None;
            }
        }
        self.overlays.len() != before
    }

    /// The library an existing settings file implies, derived rather than migrated.
    ///
    /// Every `settings.json` written before the panel existed has no `overlays` key
    /// and may have a custom SVG in either slot. Running this after every load means
    /// such a file opens with a correct panel - the sample plus whatever was already
    /// picked - and no version stamp, no migration code and no new way for the parse
    /// to fail.
    ///
    /// `Sample` is forced to the front because it is the one card that always
    /// exists, and a panel whose undeletable entry moved around as the operator
    /// added and removed others would read as a bug.
    fn normalise(&mut self) {
        let mut library = vec![Overlay::Sample];
        let assigned = [self.overlay_landscape.clone(), self.overlay_portrait.clone()];

        for overlay in self.overlays.drain(..).chain(assigned) {
            // `None` is how a slot says "nothing here"; it is not something the
            // panel can show a card for.
            if overlay != Overlay::None && !library.contains(&overlay) {
                library.push(overlay);
            }
        }
        self.overlays = library;
    }

    pub fn path(app: &AppHandle) -> Result<PathBuf, Payload> {
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
        match serde_json::from_str::<Self>(&raw) {
            Ok(mut settings) => {
                settings.normalise();
                (settings, None)
            }
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
                let said = Problem::SettingsUnreadable { path: path.clone(), detail: note }.warn();
                (Self::default(), Some(said.message))
            }
        }
    }

    pub fn save(&self, app: &AppHandle) -> Result<(), Payload> {
        let path = Self::path(app)?;
        let raw = serde_json::to_string_pretty(self)
            .map_err(|e| Problem::CannotSaveSettings { detail: detail(e) }.say())?;
        std::fs::write(&path, raw)
            .map_err(|e| Problem::CannotSaveSettings { detail: format!("{}: {e}", path.display()) }.say())
    }

    /// What the pipeline is handed.
    ///
    /// **`Overlay` never reaches the pipeline - only a path or nothing does.**
    /// That is what keeps the shipped sample entirely a shell concept:
    /// `snappr-core` sees a path, exactly as it did when it was Ruby on the far
    /// end of a pipe. It could not do the lookup itself either, since a bundle
    /// keeps `sample/` beside its `Resources` and the repo keeps it inside the
    /// source tree, and only the shell knows which layout it is in.
    ///
    /// `open_after` is false: the app draws the print in its own window, and
    /// handing it to `open(1)` as well would put Preview in front of the kiosk.
    pub fn pipeline_config(&self, app: &AppHandle) -> PipelineConfig {
        PipelineConfig {
            watch_dir: self.watch_dir.clone(),
            extensions: self.extensions.clone(),
            output_suffix: self.output_suffix.clone(),
            background: self.background.clone(),
            png_compression: self.png_compression,
            print_short_mm: self.print_short_mm,
            print_long_mm: self.print_long_mm,
            preview_max_px: self.preview_max_px,
            preview_thumb_px: self.preview_thumb_px,
            preview_quality: self.preview_quality,
            stability_checks: self.stability_checks,
            stability_timeout_secs: self.stability_timeout_secs,
            open_after: false,
            overlay_landscape: self.overlay_landscape.resolve(app, Orientation::Landscape),
            overlay_portrait: self.overlay_portrait.resolve(app, Orientation::Portrait),
        }
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

    /// What `load` actually hands the app: parsed, then normalised. The tests
    /// above deliberately stop at the parse, because the parse is what can take
    /// the whole file down; these ones are about what the panel then sees.
    fn loaded(json: &str) -> Settings {
        let mut parsed = settings(json);
        parsed.normalise();
        parsed
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

    /// The panel's guarantee: there is always exactly one sample card and it is
    /// always first.
    #[test]
    fn a_file_from_before_the_panel_gets_a_library_with_the_sample_in_it() {
        let parsed = loaded(r#"{"watch_dir": "/tmp/drop"}"#);
        assert_eq!(parsed.overlays, vec![Overlay::Sample]);
        assert!(!parsed.overlays_collapsed, "the panel opens showing its cards the first time");
    }

    /// An operator who had picked a custom landscape overlay must find it in the
    /// panel, still switched on - not have to go and pick the same file again.
    #[test]
    fn an_overlay_already_in_a_slot_joins_the_library() {
        let parsed = loaded(
            r#"{"overlay_svg_landscape": "/brand/wide.svg", "overlay_svg_portrait": null}"#,
        );
        assert_eq!(
            parsed.overlays,
            vec![Overlay::Sample, Overlay::Custom("/brand/wide.svg".into())]
        );
        assert_eq!(parsed.overlay_landscape, Overlay::Custom("/brand/wide.svg".into()));
        assert_eq!(parsed.overlay_portrait, Overlay::Sample);
    }

    /// One overlay in both slots is a supported setting - it is what both toggles
    /// lit on one card means - and must not produce two cards for the one file.
    #[test]
    fn the_library_never_holds_the_same_overlay_twice() {
        let parsed = loaded(
            r#"{"overlays": [{"kind":"sample"}, {"kind":"custom","path":"/b/w.svg"}],
                "overlay_landscape": {"kind":"custom","path":"/b/w.svg"},
                "overlay_portrait": {"kind":"custom","path":"/b/w.svg"}}"#,
        );
        assert_eq!(
            parsed.overlays,
            vec![Overlay::Sample, Overlay::Custom("/b/w.svg".into())]
        );
        assert_eq!(
            parsed.assigned(&Overlay::Custom("/b/w.svg".into())),
            vec![Orientation::Landscape, Orientation::Portrait]
        );
    }

    /// `None` is a slot saying "nothing here". It is not an overlay and must never
    /// become a card, or the panel grows an entry that composites nothing.
    #[test]
    fn an_empty_slot_does_not_become_a_card() {
        let parsed = loaded(r#"{"overlay_landscape": {"kind":"none"}, "overlay_portrait": {"kind":"none"}}"#);
        assert_eq!(parsed.overlays, vec![Overlay::Sample]);
    }

    /// A slot left pointing at a card that no longer exists would print branding
    /// the panel does not show - the one thing the panel is there to prevent.
    #[test]
    fn removing_an_overlay_empties_the_slots_that_used_it() {
        let mut parsed = loaded(
            r#"{"overlays": [{"kind":"sample"}, {"kind":"custom","path":"/b/w.svg"}],
                "overlay_landscape": {"kind":"custom","path":"/b/w.svg"},
                "overlay_portrait": {"kind":"sample"}}"#,
        );

        assert!(parsed.remove_overlay(&Overlay::Custom("/b/w.svg".into())));
        assert_eq!(parsed.overlays, vec![Overlay::Sample]);
        assert_eq!(parsed.overlay_landscape, Overlay::None);
        assert_eq!(parsed.overlay_portrait, Overlay::Sample, "the other slot is untouched");
    }

    /// The sample lives in the app bundle, so it is not the operator's to delete.
    /// Its card offers no Remove, and this is the belt to that braces.
    #[test]
    fn the_sample_cannot_be_removed() {
        let mut parsed = loaded("{}");
        assert!(!parsed.remove_overlay(&Overlay::Sample));
        assert_eq!(parsed.overlays, vec![Overlay::Sample]);
    }

    /// Adding is not assigning. An add that took over a slot would overwrite a
    /// choice the operator made without being asked to.
    #[test]
    fn adding_an_overlay_leaves_the_slots_alone_and_refuses_a_duplicate() {
        let mut parsed = loaded("{}");
        let brand = Overlay::Custom("/b/w.svg".into());

        assert!(parsed.add_overlay(brand.clone()));
        assert_eq!(parsed.overlays, vec![Overlay::Sample, brand.clone()]);
        assert_eq!(parsed.overlay_landscape, Overlay::Sample, "adding must not switch anything on");
        assert!(parsed.assigned(&brand).is_empty());

        assert!(!parsed.add_overlay(brand.clone()), "the same file must not make a second card");
        assert_eq!(parsed.overlays.len(), 2);
    }
}
