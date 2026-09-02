//! snappr: the Tauri shell around the Ruby pipeline.
//!
//! Rust owns the window, the settings, the drop-folder watch and its debounce, the
//! persisted index, the sidecar's lifetime, and printing and sharing. Ruby owns
//! exactly the image pipeline: one photo in, an overlay PNG and three previews out.
//! Nothing about image processing lives on this side.

#[cfg(target_os = "macos")]
mod mac;
mod photo;
mod problem;
mod runtime;
mod settings;
mod sidecar;
mod watcher;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, DragDropEvent, Emitter, Manager, State, WebviewWindow, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_window_state::StateFlags;

use photo::{Index, Photo, Status};
use problem::{detail, Problem};
use runtime::Runtime;
use settings::{Orientation, Overlay, Settings};
use sidecar::Sidecar;
use watcher::{Arrival, Refusal};

/// Whether the pipeline is usable, and why not if it isn't.
///
/// Held in state as well as emitted. Events are not replayed, and the sidecar is
/// ready about a second after launch - comfortably before the webview has finished
/// attaching its listeners - so a UI that only subscribed would wait forever for a
/// message that had already been sent. The frontend pulls this on mount and
/// subscribes for later changes; both paths read the same value.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RuntimeStatus {
    ready: bool,
    /// The pipeline is unusable. **Only ever set when `ready` is false**, because
    /// that is the only case the Stage callout renders. A watch that failed to
    /// start is not this: the pipeline is fine, and drops, `mise run overlay` and
    /// the retry button all still work. That one goes in `notice`.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// Watching was asked for and did not start. Rendered on the watch pill in
    /// the toolbar, which stays visible whatever the Stage is showing - and which
    /// needs its own field rather than sharing `notice`, because the pill offers
    /// a retry and must only offer it when this is the reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    watch_error: Option<String>,
    /// A one-off thing the operator should see once, discovered before the window
    /// existed - today only "your settings could not be read". Held here rather
    /// than emitted because events are not replayed; the frontend shows it on the
    /// info line on mount and it clears like any other transient message.
    #[serde(skip_serializing_if = "Option::is_none")]
    notice: Option<String>,
}

pub struct AppState {
    settings: Mutex<Settings>,
    index: Mutex<Index>,
    sidecar: Mutex<Option<Arc<Sidecar>>>,
    runtime: Mutex<RuntimeStatus>,
    /// Dropping the watcher stops it, so it has to be held for as long as the
    /// operator wants the folder watched. `None` is the paused state.
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
    /// Photos handed to the app before the sidecar was up - a drop on the Dock
    /// icon can be what launches the app. Drained by the boot thread.
    pending: Mutex<Vec<PathBuf>>,
}

/// Stores the status and announces it, in that order, so a listener that arrives
/// late can still pull the current value and get the same answer.
fn set_runtime_status(app: &AppHandle, status: RuntimeStatus) {
    *app.state::<AppState>().runtime.lock().unwrap() = status.clone();
    let _ = app.emit("runtime", status);
}

/// Every UI update goes through here, so the webview never polls and the index and
/// the screen cannot disagree.
fn publish(app: &AppHandle, photo: &Photo) {
    let state = app.state::<AppState>();
    {
        let mut index = state.index.lock().unwrap();
        index.upsert(photo.clone());
        if let Err(e) = index.save(app) {
            log::warn!("cannot persist the index: {e}");
        }
    }
    let _ = app.emit("photo", photo);
}

/// Sends one photo through the sidecar and publishes the outcome.
///
/// Runs on its own thread: the sidecar handles one request at a time and a 12MP
/// composite takes about a second, which is far too long to hold the UI.
fn process(app: AppHandle, source: PathBuf) {
    let state = app.state::<AppState>();

    // Already known and finished? Then this is a duplicate event, not a new photo.
    // Known but queued or failed? Then this is a retry of the same photo, and it
    // keeps its arrival time so it neither moves along the strip nor restarts its
    // "added N minutes ago" label. Whatever previews that record owned are the ones
    // this run supersedes - see discard_superseded.
    let (known_added_at, superseded) = {
        let index = state.index.lock().unwrap();
        match index.get(&source) {
            Some(p) if matches!(p.status, Status::Processing | Status::Done) => {
                log::debug!("{} is already handled, ignoring", source.display());
                return;
            }
            Some(p) => (Some(p.added_at), p.owned_previews()),
            None => (None, Vec::new()),
        }
    };

    let mut photo = Photo::queued(source.clone());
    if let Some(added_at) = known_added_at {
        photo.added_at = added_at;
    }
    photo.status = Status::Processing;
    publish(&app, &photo);

    let sidecar = state.sidecar.lock().unwrap().clone();
    let Some(sidecar) = sidecar else {
        photo.fail(Problem::PipelineStarting.say());
        publish(&app, &photo);
        return;
    };

    match sidecar.overlay(&source.display().to_string()) {
        Ok(frame) => photo.complete(&frame),
        Err(error) => {
            // One bad photo must not take the app down, and the original is still
            // sitting in the drop folder, so retrying is always possible. The
            // sentence was already logged where it was built; this only adds
            // which photo it was about.
            log::error!("the photo was {}", source.display());
            photo.fail(error);
        }
    }
    publish(&app, &photo);
    discard_superseded(superseded, &photo);
}

/// The entry point for photos that arrive by hand - dropped on the window or on
/// the Dock icon - rather than through the watcher. Same candidate filter, same
/// `process`, so a drop behaves exactly like an AirDrop that landed in the folder.
/// Only the stability wait is skipped: a dropped file is already complete.
///
/// A drop on the Dock icon can also be what launches the app, in which case the
/// sidecar is still starting. Those are parked in `pending` and released by the
/// boot thread the moment the sidecar is up, rather than failed on the spot.
/// One drop, however many files it carried.
///
/// The drop is the unit rather than the file, because a drop of four unsupported
/// files should explain itself once and not stack four alerts. Accepted paths are
/// processed and refused ones are collected, so a mixed drop still prints the
/// photos it can and only explains the rest.
fn ingest_drop(app: &AppHandle, paths: Vec<PathBuf>) {
    let refused: Vec<(PathBuf, Refusal)> = paths
        .into_iter()
        .filter_map(|path| ingest_one(app, path).err())
        .collect();

    if !refused.is_empty() {
        explain_refusals(app, refused);
    }
}

/// The entry point for one photo that arrived by hand - dropped on the window or
/// on the Dock icon - rather than through the watcher. Same candidate filter, same
/// `process`, so a drop behaves exactly like an AirDrop that landed in the folder.
/// Only the stability wait is skipped: a dropped file is already complete.
///
/// A drop on the Dock icon can also be what launches the app, in which case the
/// sidecar is still starting. Those are parked in `pending` and released by the
/// boot thread the moment the sidecar is up, rather than failed on the spot.
fn ingest_one(app: &AppHandle, path: PathBuf) -> Result<(), (PathBuf, Refusal)> {
    let state = app.state::<AppState>();

    let refusal = {
        let settings = state.settings.lock().unwrap();
        watcher::refusal(&path, &settings)
    };
    if let Some(refusal) = refusal {
        // `watcher::refusal` does no I/O, because it runs on every filesystem
        // event in a folder with ~1.9k entries. Here there is exactly one path
        // and it is worth one stat to tell a folder from a nameless file.
        let refusal = match refusal {
            Refusal::Unknown if path.is_dir() => Refusal::IsAFolder,
            other => other,
        };
        return Err((path, refusal));
    }

    // Hold `pending` while looking at `sidecar`: the boot thread takes the same
    // lock before it installs the sidecar and drains the queue, so a path can
    // never fall between "not ready yet" here and the drain there.
    let mut pending = state.pending.lock().unwrap();
    if state.sidecar.lock().unwrap().is_some() {
        drop(pending);
        log::info!("{} handed to snappr, processing", path.display());
        let handle = app.clone();
        std::thread::spawn(move || process(handle, path));
    } else {
        log::info!("{} queued until the pipeline is ready", path.display());
        pending.push(path);
    }
    Ok(())
}

/// Says why a drop was refused, in a native alert.
///
/// A modal, and the only one in the app apart from Clear All's confirm, because a
/// refused drop is the one failure that otherwise produces *nothing at all*: no
/// cell, no spinner, no photo, and no line either - the problem line belongs to
/// the selected photo and this drop never created one. Everything else that can
/// go wrong has somewhere on screen it belongs.
///
/// The OS's alert rather than a Radix dialog, for the reason CLAUDE.md gives for
/// the context menus, plus one this side: a drop on the Dock icon can be what
/// *launches* the app, and a frontend-rendered modal cannot explain a drop that
/// happened before the webview existed.
///
/// `show`, never `blocking_show`: this runs on the event thread, and blocking
/// there freezes the window.
/// The catalogue entry for one refusal. A free function rather than a closure so
/// the returned Problem can borrow the path it was given.
fn refusal_problem<'a>(path: &'a Path, refusal: Refusal, accepted: &str) -> Problem<'a> {
    let file = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    match refusal {
        Refusal::AlreadyAPrint => Problem::DropAlreadyAPrint { file },
        Refusal::IsAFolder => Problem::DropIsAFolder { file },
        Refusal::StillDownloading => Problem::DropStillDownloading { file },
        Refusal::Unnamed => Problem::DropUnreadableName { path },
        // Unknown reaches here only for an extensionless file that is not a
        // folder, which reads the same way to an operator as a wrong extension.
        Refusal::Unknown | Refusal::WrongKind => Problem::DropWrongKind {
            kind: path
                .extension()
                .map(|e| format!("a .{} file", e.to_string_lossy()))
                .unwrap_or_else(|| "not a kind of file snappr knows".into()),
            file,
            accepted: accepted.to_string(),
        },
    }
}

fn explain_refusals(app: &AppHandle, refused: Vec<(PathBuf, Refusal)>) {
    let state = app.state::<AppState>();
    let accepted = {
        let settings = state.settings.lock().unwrap();
        settings.extensions.join(", ").to_uppercase()
    };

    // Each sentence is logged as it is built, so the console carries the paths
    // and the alert carries only the sentences.
    let mut sentences: Vec<String> = refused
        .iter()
        .map(|(path, refusal)| refusal_problem(path, *refusal, &accepted).warn())
        .collect();

    // One refusal reads as a sentence on its own; several want a lead-in and a
    // bullet each, because every sentence already names its own file and a run
    // of them reads as one paragraph otherwise.
    let body = if sentences.len() == 1 {
        sentences.remove(0)
    } else {
        format!(
            "Snappr could not use {} of the files you dropped.\n\n{}",
            sentences.len(),
            sentences.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n")
        )
    };

    let title = if refused.len() == 1 { "Nothing to print" } else { "Some files were skipped" };
    app.dialog()
        .message(body)
        .title(title)
        // A warning, not an error: the operator did nothing wrong, snappr just
        // cannot use what they dropped.
        .kind(MessageDialogKind::Warning)
        .show(|_| {});
}

fn start_watching(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let handle = app.clone();
    // By value: the closure outlives this call, so it cannot borrow `settings`.
    let timeout = settings.stability_timeout().as_secs();
    let watcher = watcher::spawn(settings.watch_dir.clone(), settings.clone(), move |arrival| {
        let handle = handle.clone();
        match arrival {
            Arrival::Settled(path) => {
                std::thread::spawn(move || process(handle, path));
            }
            // The watcher gave up on a file that never stopped changing, and no
            // Photo record was ever published for it - so without this the
            // operator's AirDrop simply vanishes. A failed cell is self-healing:
            // if the transfer was cancelled the record is dropped on the next
            // launch by Photo::intact, and if it completes later the watcher
            // re-sees the file and `process` replaces the record in place.
            Arrival::Abandoned(path) => {
                let mut photo = Photo::queued(path.clone());
                let said = Problem::TransferStalled {
                    file: photo.name.clone(),
                    path: &path,
                    secs: timeout,
                }
                .warn();
                photo.fail(said);
                publish(&handle, &photo);
            }
        }
    })?;

    app.state::<AppState>()
        .watcher
        .lock()
        .unwrap()
        .replace(watcher);
    log::info!("watching {}", settings.watch_dir.display());
    Ok(())
}

/// Starts the watch and keeps the toolbar's notice in step with the outcome.
///
/// Every path that turns watching on goes through here - boot, the pill, and a
/// change of folder - so the pill can never claim to be watching when it is not.
/// That was a real lie and not just a missing message: `set_watching` put
/// `watch_enabled` back to false on failure but the boot path did not, so after a
/// boot-time watch failure the bar said "Watching" over a folder nothing was
/// looking at.
fn start_watching_noted(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let result = start_watching(app, settings);
    set_watch_error(app, result.as_ref().err().cloned());
    result
}

/// Records whether the watch is working, leaving the rest of the status alone.
fn set_watch_error(app: &AppHandle, watch_error: Option<String>) {
    let mut status = app.state::<AppState>().runtime.lock().unwrap().clone();
    if status.watch_error == watch_error {
        return;
    }
    status.watch_error = watch_error;
    set_runtime_status(app, status);
}

#[tauri::command]
fn list_photos(state: State<AppState>) -> Vec<Photo> {
    state.index.lock().unwrap().all().to_vec()
}

/// The photo the operator was last looking at, so a relaunch reopens it rather
/// than jumping to the newest. `None` means there was no selection to restore, and
/// the frontend falls back to the newest photo.
#[tauri::command]
fn selected_photo(state: State<AppState>) -> Option<PathBuf> {
    state.index.lock().unwrap().selected().cloned()
}

/// Remembers the selection. Called on every change, which is fine: selection moves
/// at human speed, and `publish` already saves the whole index per photo event.
#[tauri::command]
fn set_selected(app: AppHandle, state: State<AppState>, source: PathBuf) -> Result<(), String> {
    let mut index = state.index.lock().unwrap();
    if index.select(source) {
        index.save(&app)?;
    }
    Ok(())
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

/// Changing the watch directory restarts the watch in place, so the operator does
/// not have to relaunch the app to point it somewhere else.
#[tauri::command]
fn set_watch_dir(app: AppHandle, state: State<AppState>, dir: PathBuf) -> Result<Settings, String> {
    if !dir.is_dir() {
        return Err(Problem::NotAFolder { path: &dir }.say());
    }

    let updated = {
        let mut settings = state.settings.lock().unwrap();
        settings.watch_dir = dir;
        settings.save(&app)?;
        settings.clone()
    };

    if updated.watch_enabled {
        start_watching_noted(&app, &updated)?;
    }
    Ok(updated)
}

/// Sets what one orientation composites: the shipped sample, a picked SVG, or
/// nothing.
///
/// None of the three is a broken state. Nothing at all still prints - the photo is
/// cropped to the print ratio with nothing over it - and the sample is what a fresh
/// install lands on so that a first photo shows something rather than looking like
/// the app did not run.
///
/// Only that a picked file exists is checked. An SVG with no #FF00FF window, or one
/// drawn at the wrong shape, comes back as a warning on the photo itself - which
/// the filmstrip and the Actions bar already show - rather than as a rejected pick,
/// because that is where the operator can see it against the result.
///
/// Photos already in the strip keep their output. Regenerating one is what the
/// retry button is for; silently rewriting a strip's worth of 10-25 MB PNGs
/// because a menu item was clicked is not worth the surprise.
#[tauri::command]
fn set_overlay(
    app: AppHandle,
    state: State<AppState>,
    orientation: Orientation,
    choice: Overlay,
) -> Result<Settings, String> {
    if let Overlay::Custom(path) = &choice {
        if !path.is_file() {
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            return Err(Problem::OverlayFileGone { file, path }.say());
        }
    }

    let updated = {
        let mut settings = state.settings.lock().unwrap();
        match orientation {
            Orientation::Landscape => settings.overlay_landscape = choice,
            Orientation::Portrait => settings.overlay_portrait = choice,
        }
        settings.save(&app)?;
        settings.clone()
    };

    // Push it to the running child so the next photo uses it. No sidecar yet just
    // means the boot handshake has not happened, and that sends these settings
    // anyway - so saving is the whole job in that case.
    let sidecar = state.sidecar.lock().unwrap().clone();
    if let Some(sidecar) = sidecar {
        let preview_dir = Index::preview_dir(&app)?;
        sidecar.reconfigure(updated.pipeline_config(&app), &preview_dir.display().to_string())?;
    }

    // The resolved path rather than the choice, because that is what the pipeline
    // was actually handed and what a puzzled log reader needs to see.
    let resolved = match orientation {
        Orientation::Landscape => updated.overlay_landscape.resolve(&app, orientation),
        Orientation::Portrait => updated.overlay_portrait.resolve(&app, orientation),
    };
    log::info!(
        "{orientation:?} overlay is now {}",
        resolved.map_or_else(|| "nothing".to_string(), |p| p.display().to_string())
    );
    Ok(updated)
}

/// Pauses or resumes the folder watch. Dropping the watcher is what stops it. The
/// choice is persisted, so a kiosk comes back the way it was left; the toggle in
/// the bar always shows which.
#[tauri::command]
fn set_watching(app: AppHandle, state: State<AppState>, enabled: bool) -> Result<Settings, String> {
    let updated = {
        let mut settings = state.settings.lock().unwrap();
        settings.watch_enabled = enabled;
        settings.save(&app)?;
        settings.clone()
    };

    if enabled {
        if let Err(error) = start_watching_noted(&app, &updated) {
            // Do not claim to be watching when we are not: put the setting back
            // and let the UI re-read it. The notice start_watching_noted left
            // behind is what the pill shows in the meantime.
            let mut settings = state.settings.lock().unwrap();
            settings.watch_enabled = false;
            settings.save(&app)?;
            return Err(error);
        }
    } else {
        state.watcher.lock().unwrap().take();
        // "Paused" is honest on its own, so any standing complaint goes away.
        set_watch_error(&app, None);
        log::info!("watching paused");
    }
    Ok(updated)
}

/// Re-runs a photo that failed, or one the operator wants regenerated after an
/// overlay change. The original is always still on disk, so this is the retry.
#[tauri::command]
fn reprocess(app: AppHandle, state: State<AppState>, source: PathBuf) -> Result<(), String> {
    state.index.lock().unwrap().update(&source, |photo| {
        photo.status = Status::Queued;
        photo.error = None;
    });
    std::thread::spawn(move || process(app, source));
    Ok(())
}

/// Finishes a clear: deletes the preview JPEGs the dropped records owned and tells
/// the webview which cells have gone.
///
/// Called with no locks held. Deleting a dozen files is quick but it is still I/O,
/// and `publish` takes the index lock from the processing threads.
///
/// **Nothing in the watched folder is touched.** The original and the overlay PNG
/// beside it both stay where they are, so a cleared photo can be brought straight
/// back by dropping it on the window again - which is also what makes clearing safe
/// enough to leave unconfirmed for a single cell.
fn discard(app: &AppHandle, gone: Vec<Photo>) -> usize {
    for path in gone.iter().flat_map(Photo::owned_previews) {
        delete_preview(&path);
    }

    let sources: Vec<&PathBuf> = gone.iter().map(|p| &p.source).collect();
    log::info!("cleared {} photo(s) from the filmstrip", sources.len());
    let _ = app.emit("cleared", &sources);
    sources.len()
}

/// Deletes the previews an earlier composite of the same photo owned.
///
/// A photo's preview filenames are keyed on the composite, not on the source alone
/// (`Snappr::Preview#key_for` says why), so re-running one - a retry, or the same
/// file dropped in again after the overlay was changed - leaves the previous set
/// behind with nothing pointing at it. Deleting exactly what the superseded record
/// named, and only what the new one does not, is the whole of the cleanup: the
/// index is the only thing that ever knew those paths.
///
/// Called after `publish`, so the record on disk already points at the survivors
/// and a crash in between leaks three JPEGs rather than blanking a live cell.
fn discard_superseded(previous: Vec<PathBuf>, photo: &Photo) {
    let current = photo.owned_previews();
    for path in previous.iter().filter(|path| !current.contains(path)) {
        delete_preview(path);
    }
}

/// Removes one preview JPEG. Only ever called on paths inside the app's own data
/// directory - nothing in the watched folder is snappr's to delete.
fn delete_preview(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => log::debug!("deleted {}", path.display()),
        // Already gone is the outcome we wanted, not a failure worth logging.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!("cannot delete {}: {e}", path.display()),
    }
}

/// Clears one cell from the filmstrip.
///
/// A photo that is not in the index is already clear, so this succeeds rather than
/// complaining - the operator asked for a state, not for an action.
#[tauri::command]
fn clear_photo(app: AppHandle, state: State<AppState>, source: PathBuf) -> Result<(), String> {
    let gone = {
        let mut index = state.index.lock().unwrap();
        match index.remove(&source) {
            Some(photo) => {
                index.save(&app)?;
                photo
            }
            None => return Ok(()),
        }
    };
    discard(&app, vec![gone]);
    Ok(())
}

/// Clears the whole filmstrip.
///
/// A photo that is still being composited goes too, and then comes back when it
/// finishes: `process` publishes its outcome regardless of what happened to the
/// index underneath it. That is the honest behaviour - the photo really did arrive
/// after the operator cleared - and it is why this returns the count rather than
/// leaving the UI to assume the strip is now empty.
#[tauri::command]
fn clear_all_photos(app: AppHandle, state: State<AppState>) -> Result<usize, String> {
    let gone = {
        let mut index = state.index.lock().unwrap();
        let gone = index.drain();
        index.save(&app)?;
        gone
    };
    Ok(discard(&app, gone))
}

#[tauri::command]
fn runtime_status(state: State<AppState>) -> RuntimeStatus {
    state.runtime.lock().unwrap().clone()
}

/// The finished PNG for a photo, or a reason there isn't one. Share, print and
/// reveal all act on the print artefact, never on the source or a preview.
fn destination_of(state: &State<AppState>, source: &Path) -> Result<(PathBuf, bool), String> {
    let index = state.index.lock().unwrap();
    let photo = index
        .get(source)
        .ok_or_else(|| Problem::PhotoForgotten { path: source }.say())?;
    if photo.status != Status::Done {
        return Err(Problem::PhotoStillWorking.say());
    }
    let destination = photo
        .destination
        .clone()
        .filter(|d| d.exists())
        .ok_or_else(|| Problem::PrintFileGone.say())?;
    Ok((destination, photo.orientation.as_deref() == Some("landscape")))
}

/// Shares the overlay PNG itself - 10-25 MB, and 16-bit when the source was HEIC.
/// The decision was to share exactly what gets printed rather than a downsized
/// copy, so the share sheet may take a moment to appear.
#[tauri::command]
fn share_photo(
    window: WebviewWindow,
    state: State<AppState>,
    source: PathBuf,
    anchor: mac::Anchor,
) -> Result<(), String> {
    let (destination, _) = destination_of(&state, &source)?;
    mac::share(&window, &destination, anchor)
}

#[tauri::command]
fn print_photo(window: WebviewWindow, state: State<AppState>, source: PathBuf) -> Result<(), String> {
    let (destination, landscape) = destination_of(&state, &source)?;
    let paper = state.settings.lock().unwrap().print_paper_name.clone();
    mac::print(&window, &destination, landscape, &paper)
}

/// Finder, with the file selected.
fn reveal(path: &Path) -> Result<(), String> {
    std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .status()
        .map_err(|e| Problem::FinderWontOpen { detail: detail(e) }.say())
        .and_then(|status| {
            status.success().then_some(()).ok_or_else(|| {
                Problem::FinderWontOpen { detail: format!("open -R exited {status}") }.say()
            })
        })
}

/// Finder, with the PNG selected. The escape hatch for anything the app cannot do.
#[tauri::command]
fn reveal_photo(state: State<AppState>, source: PathBuf) -> Result<(), String> {
    let (destination, _) = destination_of(&state, &source)?;
    reveal(&destination)
}

/// Finder, with the shipped sample overlay selected, so it can be copied out and
/// used as the starting point for a real one.
///
/// This reveals the original in place rather than exporting a copy, which is worth
/// knowing the shape of: in a built `.app` the file sits in `Contents/Resources`,
/// so Finder opens a package-contents view, the file is read-only, and an app run
/// from a mounted DMG is on a read-only volume besides. Under `tauri dev` the
/// target is the build script's copy in `target/debug/sample/`, and edits there are
/// overwritten by the next build. Either way the operator copies it somewhere of
/// their own first, which is the intended gesture - this is a "here is the file"
/// button, not an "edit this" one.
#[tauri::command]
fn reveal_sample_overlay(app: AppHandle) -> Result<(), String> {
    let path = Overlay::Sample.resolve(&app, Orientation::Landscape).ok_or_else(|| {
        Problem::FinderWontOpen { detail: "this build ships no sample overlay".into() }.say()
    })?;
    reveal(&path)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be registered first, per the plugin's own documentation. Two
        // watchers on one folder is harmless; two windows is just confusing.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }))
        // The frame the operator set up, and nothing else: a window quit while
        // hidden comes back visible, and decorations stay whatever the config says.
        //
        // MAXIMIZED and FULLSCREEN are in here for the *size*, not because a kiosk
        // needs to reopen zoomed. macOS `isZoomed` is derived from the frame rather
        // than from a gesture, so a window merely dragged out to fill the screen -
        // the obvious kiosk setup - reports as maximized. The plugin then skips
        // saving the size while still tracking every move, so the file ends up with
        // the new position beside a stale size, and the window reopens small at the
        // right place. Recording the two states is what makes the saved frame
        // coherent again: size and position come back as the pre-zoom pair the
        // plugin actually kept, and the zoom is reapplied over them.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    StateFlags::SIZE
                        | StateFlags::POSITION
                        | StateFlags::MAXIMIZED
                        | StateFlags::FULLSCREEN,
                )
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .build(),
        )
        .manage(AppState {
            settings: Mutex::new(Settings::default()),
            index: Mutex::new(Index::default()),
            sidecar: Mutex::new(None),
            runtime: Mutex::new(RuntimeStatus::default()),
            watcher: Mutex::new(None),
            pending: Mutex::new(Vec::new()),
        })
        .invoke_handler(tauri::generate_handler![
            list_photos,
            selected_photo,
            set_selected,
            get_settings,
            set_watch_dir,
            set_watching,
            set_overlay,
            reprocess,
            clear_photo,
            clear_all_photos,
            runtime_status,
            share_photo,
            print_photo,
            reveal_photo,
            reveal_sample_overlay,
        ])
        // Files dropped on the window. Tauri owns drag-and-drop (dragDropEnabled is
        // on, the default), so the webview never sees the drop and cannot navigate
        // to the file - which is what it did while the flag was off.
        .on_window_event(|window, event| {
            if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event {
                // The whole drop at once, so a drop carrying several files it
                // cannot use explains itself once rather than once per file.
                ingest_drop(window.app_handle(), paths.clone());
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let state = app.state::<AppState>();

            // A settings file that could not be read has already been moved aside
            // and reported; the sentence comes back here because this runs before
            // the window exists. It rides along on the runtime status, which the
            // frontend pulls on mount for exactly this reason.
            let (settings, reset) = Settings::load(&handle);
            *state.settings.lock().unwrap() = settings.clone();
            *state.index.lock().unwrap() = Index::load(&handle);

            // The sidecar is started in the background so a slow or broken runtime
            // shows up as an error in the UI rather than as an app that never opens.
            let boot = handle.clone();
            std::thread::spawn(move || {
                let started = Runtime::detect(&boot).and_then(|runtime| {
                    let preview_dir = Index::preview_dir(&boot)?;
                    Sidecar::start(
                        &runtime,
                        settings.pipeline_config(&boot),
                        &preview_dir.display().to_string(),
                    )
                });

                match started {
                    Ok(sidecar) => {
                        // Install the sidecar and take whatever was dropped on us
                        // before it was ready, under the same lock `ingest` holds
                        // while it decides between "process now" and "queue".
                        let queued = {
                            let state = boot.state::<AppState>();
                            let mut pending = state.pending.lock().unwrap();
                            state.sidecar.lock().unwrap().replace(Arc::new(sidecar));
                            std::mem::take(&mut *pending)
                        };
                        for path in queued {
                            let handle = boot.clone();
                            std::thread::spawn(move || process(handle, path));
                        }

                        // The pipeline is usable either way here: a watch that
                        // failed to start still leaves drops, `mise run overlay`
                        // and the retry button working, so this is reported rather
                        // than treated as the app being broken.
                        //
                        // It goes in `notice`, not `error`. `error` is rendered by
                        // the Stage only while the pipeline is down, so a watch
                        // failure put there was invisible - and at an event the
                        // Stage always has a photo in it. `watch_error` is drawn
                        // on the watch pill, which is where "not watching"
                        // belongs anyway.
                        //
                        // start_watching, not start_watching_noted: the whole
                        // status is set wholesale below, so going through the
                        // noted variant would emit the event twice.
                        let watch_failed = if settings.watch_enabled {
                            start_watching(&boot, &settings).err()
                        } else {
                            log::info!("watching is paused, not watching {}", settings.watch_dir.display());
                            None
                        };
                        set_runtime_status(
                            &boot,
                            RuntimeStatus {
                                ready: true,
                                error: None,
                                watch_error: watch_failed,
                                notice: reset,
                            },
                        );
                    }
                    Err(error) => {
                        set_runtime_status(
                            &boot,
                            RuntimeStatus {
                                ready: false,
                                error: Some(error),
                                watch_error: None,
                                notice: reset,
                            },
                        );
                    }
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building snappr")
        .run(|app, event| match event {
            // Closing stdin is the documented shutdown for `Server#run`, which reads
            // to EOF. Without this the child outlives the app.
            tauri::RunEvent::ExitRequested { .. } => {
                if let Some(sidecar) = app.state::<AppState>().sidecar.lock().unwrap().take() {
                    sidecar.shutdown();
                }
            }
            // Files dropped on the Dock icon, or opened with snappr from Finder.
            // macOS delivers these to the running instance directly, so the
            // single-instance plugin is not involved. The Dock only accepts the
            // drop because src-tauri/Info.plist declares the photo types under
            // CFBundleDocumentTypes - and only for the bundled .app. Under
            // `tauri dev` the Dock refuses every drop: Launch Services will not
            // register a bare executable as an application, so there is nothing
            // for it to check the file against. Not a bug in this arm.
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Opened { urls } => {
                for url in urls {
                    match url.to_file_path() {
                        Ok(path) => ingest_drop(app, vec![path]),
                        Err(()) => {
                            Problem::DropNotAFile { url: url.to_string() }.warn();
                        }
                    }
                }
            }
            _ => {}
        });
}
