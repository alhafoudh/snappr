//! The overlay panel's cards, and the PNGs they draw.
//!
//! The panel is a library, not a picker: the operator keeps several designs and
//! switches between them, and each card carries its own landscape and portrait
//! toggles so one overlay can be live for both. The library itself lives in
//! `Settings::overlays`; this module is everything the *panel* needs that the
//! settings file does not hold - the picture, the photo window, and whether the
//! file is still there.
//!
//! **The picture is a check, not a decoration.** The card paints a grey rectangle
//! with a photo icon at the window Ruby reports and draws the render over it, so
//! the operator sees where the photo will land before a print is made. That only
//! means anything because the render comes back through librsvg - the same
//! renderer the print uses. Handing the SVG to the webview instead would show what
//! WebKit makes of the file, complete with the magenta marker the pipeline strips,
//! and would prove nothing about what will actually be composited.
//!
//! Two ownership rules, both of which look like violations of "snappr never
//! deletes anything" and are not:
//!
//! - **The PNGs are snappr's**, in `app_data_dir()/overlays`, and are deleted when
//!   no card points at them any more. The operator's SVG is never touched - it is
//!   remembered by path and never copied, so re-exporting a design over the same
//!   file refreshes its card and takes effect on the next photo.
//! - **A stale PNG is a wrong answer, not a slow one.** The key is the path *and*
//!   the file's mtime, for the reason spelled out in `lib/snappr/preview.rb`: the
//!   webview serves an `asset://` URL it has already fetched out of its own memory
//!   cache for the life of the session, so re-exporting a design under a key that
//!   did not change would leave the operator looking at the previous artwork.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::problem::{detail, name_of, Problem};
use crate::settings::{Orientation, Overlay, Settings};
use crate::AppState;

/// Where the photo goes inside an overlay, as fractions of the frame in 0..1.
/// Fractions rather than pixels because the card is drawn at whatever size the
/// panel is, and because that is the shape `Snappr::OverlaySvg` reports.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Window {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// What Ruby made of one overlay file. Cached against the key, so switching an
/// overlay on and off does not re-render it.
#[derive(Debug, Clone)]
pub struct Render {
    thumb: PathBuf,
    /// The shape the overlay was *drawn* at, not the paper's. The card is sized to
    /// it so the artwork is never squashed - which means an overlay drawn at the
    /// wrong ratio looks wrong on its card, a second check for nothing.
    aspect: f64,
    window: Option<Window>,
    warnings: Vec<String>,
}

/// One card in the panel.
///
/// `landscape` and `portrait` are booleans rather than something the frontend
/// derives by comparing `Overlay` values: the comparison is structural, it would
/// be the second place the rule lived, and the card is the only thing that needs
/// the answer.
#[derive(Debug, Clone, Serialize)]
pub struct OverlayCard {
    /// The identity, and what goes back on `set_overlay` when a toggle is clicked.
    pub overlay: Overlay,
    pub name: String,
    /// Whether the card offers Remove. False for the sample, which lives in the
    /// app bundle and is not the operator's to delete.
    pub removable: bool,
    pub landscape: bool,
    pub portrait: bool,
    /// The rendered PNG, once it exists. `None` means "not drawn yet" and the card
    /// shows a skeleton - the render happens off the UI thread and queues behind
    /// any photo in flight.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb: Option<PathBuf>,
    /// The shape to draw the card at. `None` until the render lands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect: Option<f64>,
    /// `None` when the marker was not found. The card still draws the artwork and
    /// says so, which is the whole point of showing a picture at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<Window>,
    /// The overlay is configured and its file has gone. Not an error the operator
    /// caused now, so it rides on the card rather than interrupting anything.
    pub missing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub warnings: Vec<String>,
}

/// The PNGs' home. Snappr's own directory, and the second entry in
/// `assetProtocol.scope` - the directory and the scope are a single decision, so
/// moving one silently stops the panel drawing anything.
pub fn dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| Problem::CannotDrawOverlays { detail: detail(e) }.say())?
        .join("overlays");
    std::fs::create_dir_all(&dir)
        .map_err(|e| Problem::CannotDrawOverlays { detail: format!("{}: {e}", dir.display()) }.say())?;
    Ok(dir)
}

/// Path and mtime, hashed to something short and filename-safe.
///
/// Not a cryptographic hash: nothing here is adversarial, and the only thing the
/// key has to do is change when the file does. `DefaultHasher` is deterministic
/// for a given build, so the PNGs survive a restart; a Rust upgrade that changed
/// the algorithm would re-render every card once and collect the old files, which
/// is a cost of nothing.
fn key_for(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    // Nanoseconds, like Preview's key: a design re-exported twice inside one
    // second is exactly the case a coarser stamp cannot tell apart.
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_nanos())
        .unwrap_or(0)
        .hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// The key and the file for every overlay in the library that has one.
///
/// A `Sample` whose resource is missing and a `Custom` whose file has gone both
/// drop out here, which is what makes `missing` on the card and the GC below
/// agree without either having to check twice.
fn resolved(app: &AppHandle, settings: &Settings) -> Vec<(Overlay, PathBuf, String)> {
    settings
        .overlays
        .iter()
        .filter_map(|overlay| {
            // The slot an overlay is resolved against only matters for `Sample`,
            // and the sample only exists for landscape.
            let path = overlay.resolve(app, Orientation::Landscape)?;
            path.is_file().then(|| {
                let key = key_for(&path);
                (overlay.clone(), path, key)
            })
        })
        .collect()
}

/// The cards as they stand right now, drawn or not.
///
/// Synchronous and cheap - one `stat` per overlay - so it can answer
/// `list_overlays` and every re-emit without waiting on the sidecar.
pub fn cards(app: &AppHandle, settings: &Settings) -> Vec<OverlayCard> {
    let cache = app.state::<AppState>();
    let cache = cache.overlays.lock().unwrap();

    settings
        .overlays
        .iter()
        .map(|overlay| {
            let assigned = settings.assigned(overlay);
            let path = overlay.resolve(app, Orientation::Landscape);
            let missing = path.as_ref().map_or(true, |path| !path.is_file());
            let render = path.as_ref().filter(|_| !missing).and_then(|path| cache.get(&key_for(path)));

            OverlayCard {
                name: overlay.label(),
                removable: !matches!(overlay, Overlay::Sample),
                landscape: assigned.contains(&Orientation::Landscape),
                portrait: assigned.contains(&Orientation::Portrait),
                thumb: render.map(|render| render.thumb.clone()),
                aspect: render.map(|render| render.aspect),
                window: render.and_then(|render| render.window),
                warnings: render.map(|render| render.warnings.clone()).unwrap_or_default(),
                error: missing.then(|| {
                    let path = path.clone().unwrap_or_default();
                    Problem::OverlayFileGone { file: name_of(&path), path: &path }.ui()
                }),
                missing,
                overlay: overlay.clone(),
            }
        })
        .collect()
}

/// Stores the cards' state and announces it, in that order - the same rule
/// `set_runtime_status` follows, so a listener that arrives late pulls the same
/// answer it would have been sent.
pub fn announce(app: &AppHandle) {
    let settings = app.state::<AppState>().settings.lock().unwrap().clone();
    let _ = app.emit("overlays", cards(app, &settings));
}

/// Draws whatever is not drawn yet, then collects the files nothing points at.
///
/// **Runs on its own thread.** The sidecar takes one request at a time, so a card
/// asked for while a 12MP HEIC is compositing waits behind it; that is fine for a
/// picture and would not be fine for the window.
///
/// Announces once per render rather than once at the end, so the cards fill in as
/// they arrive instead of the panel sitting blank until the slowest one lands.
///
/// **One at a time**, on `AppState::drawing`. Overlapping refreshes would let the
/// older one's collection pass sweep away a PNG the newer one had just written -
/// see the field's comment. Serialising is enough on its own: whoever holds the
/// lock finishes sweeping before the next reads the settings, so the next always
/// sees an accurate library.
pub fn refresh(app: AppHandle) {
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let drawing = state.drawing.lock().unwrap();
        // Read the settings *after* taking the lock: a refresh that queued behind
        // another is usually queued precisely because the library just changed.
        let settings = state.settings.lock().unwrap().clone();
        let Ok(dir) = dir(&app) else { return };

        let wanted = resolved(&app, &settings);
        for (overlay, path, key) in &wanted {
            if state.overlays.lock().unwrap().contains_key(key) {
                continue;
            }
            let Some(sidecar) = state.sidecar.lock().unwrap().clone() else {
                // Not up yet. The boot thread calls this again once it is, so
                // there is nothing to retry and nothing to say.
                return;
            };

            match sidecar.overlay_preview(&path.display().to_string(), &dir.display().to_string(), key) {
                Ok(frame) => {
                    state.overlays.lock().unwrap().insert(key.clone(), render_from(&frame));
                    announce(&app);
                }
                // The overlay still works - it is only its picture that is
                // missing - so this is a log line and a card without a thumbnail,
                // not something to interrupt the operator with.
                Err(e) => log::warn!("cannot draw {}: {e}", overlay.label()),
            }
        }

        collect(&dir, &wanted, &state);
        drop(drawing);
    });
}

fn render_from(frame: &Value) -> Render {
    Render {
        thumb: PathBuf::from(frame["thumb"].as_str().unwrap_or_default()),
        // The rendered pixels are the authority, not the reported ratio: they are
        // what the card is drawing, and a divide-by-zero here would take the card
        // out rather than the overlay.
        aspect: match (frame["width"].as_f64(), frame["height"].as_f64()) {
            (Some(width), Some(height)) if height > 0.0 => width / height,
            _ => 1.0,
        },
        window: frame["window"].as_object().map(|window| {
            let at = |key: &str| window.get(key).and_then(Value::as_f64).unwrap_or(0.0);
            Window { x: at("x"), y: at("y"), width: at("width"), height: at("height") }
        }),
        // Only the operator's half crosses. The class and the code were logged on
        // the Ruby side; see the frame documentation in lib/snappr/server.rb.
        warnings: frame["warnings"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|warning| warning["message"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Deletes the PNGs no overlay points at any more, and forgets their renders.
///
/// Keyed on the filename rather than on a remembered list, so a PNG left behind by
/// a crash or by a previous build is collected too. This directory holds nothing
/// but these files, which is what makes that safe.
fn collect(dir: &Path, wanted: &[(Overlay, PathBuf, String)], state: &AppState) {
    let keys: Vec<&String> = wanted.iter().map(|(_, _, key)| key).collect();
    state.overlays.lock().unwrap().retain(|key, _| keys.iter().any(|kept| *kept == key));

    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let stem = path.file_stem().map(|stem| stem.to_string_lossy().into_owned());
        if stem.is_some_and(|stem| keys.iter().any(|kept| **kept == stem)) {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => log::debug!("deleted {}", path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("cannot delete {}: {e}", path.display()),
        }
    }
}

/// The renders, keyed by the same key the PNG is named after.
pub type Cache = HashMap<String, Render>;
