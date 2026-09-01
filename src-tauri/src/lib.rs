//! snappr: the Tauri shell around the Ruby pipeline.
//!
//! Rust owns the window, the settings, the drop-folder watch and its debounce, the
//! persisted index, the sidecar's lifetime, and printing and sharing. Ruby owns
//! exactly the image pipeline: one photo in, an overlay PNG and three previews out.
//! Nothing about image processing lives on this side.

#[cfg(target_os = "macos")]
mod mac;
mod photo;
mod runtime;
mod settings;
mod sidecar;
mod watcher;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, DragDropEvent, Emitter, Manager, State, WebviewWindow, WindowEvent};
use tauri_plugin_window_state::StateFlags;

use photo::{Index, Photo, Status};
use runtime::Runtime;
use settings::Settings;
use sidecar::Sidecar;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
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
    // "added N minutes ago" label.
    let known_added_at = {
        let index = state.index.lock().unwrap();
        match index.get(&source) {
            Some(p) if matches!(p.status, Status::Processing | Status::Done) => {
                log::debug!("{} is already handled, ignoring", source.display());
                return;
            }
            Some(p) => Some(p.added_at),
            None => None,
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
        photo.fail("the Ruby sidecar is not running".into());
        publish(&app, &photo);
        return;
    };

    match sidecar.overlay(&source.display().to_string()) {
        Ok(frame) => photo.complete(&frame),
        Err(error) => {
            // One bad photo must not take the app down, and the original is still
            // sitting in the drop folder, so retrying is always possible.
            log::error!("{}: {error}", source.display());
            photo.fail(error);
        }
    }
    publish(&app, &photo);
}

/// The entry point for photos that arrive by hand - dropped on the window or on
/// the Dock icon - rather than through the watcher. Same candidate filter, same
/// `process`, so a drop behaves exactly like an AirDrop that landed in the folder.
/// Only the stability wait is skipped: a dropped file is already complete.
///
/// A drop on the Dock icon can also be what launches the app, in which case the
/// sidecar is still starting. Those are parked in `pending` and released by the
/// boot thread the moment the sidecar is up, rather than failed on the spot.
fn ingest(app: &AppHandle, path: PathBuf) {
    let state = app.state::<AppState>();

    let candidate = {
        let settings = state.settings.lock().unwrap();
        watcher::is_candidate(&path, &settings)
    };
    if !candidate {
        log::info!("{} ignored, not a file snappr handles", path.display());
        return;
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
}

fn start_watching(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let handle = app.clone();
    let watcher = watcher::spawn(settings.watch_dir.clone(), settings.clone(), move |path| {
        let handle = handle.clone();
        std::thread::spawn(move || process(handle, path));
    })?;

    app.state::<AppState>()
        .watcher
        .lock()
        .unwrap()
        .replace(watcher);
    log::info!("watching {}", settings.watch_dir.display());
    Ok(())
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
        return Err(format!("{} is not a directory", dir.display()));
    }

    let updated = {
        let mut settings = state.settings.lock().unwrap();
        settings.watch_dir = dir;
        settings.save(&app)?;
        settings.clone()
    };

    if updated.watch_enabled {
        start_watching(&app, &updated)?;
    }
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
        if let Err(error) = start_watching(&app, &updated) {
            // Do not claim to be watching when we are not: put the setting back
            // and let the UI re-read it.
            let mut settings = state.settings.lock().unwrap();
            settings.watch_enabled = false;
            settings.save(&app)?;
            return Err(error);
        }
    } else {
        state.watcher.lock().unwrap().take();
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
        match std::fs::remove_file(&path) {
            Ok(()) => log::debug!("deleted {}", path.display()),
            // Already gone is the outcome we wanted, not a failure worth logging.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("cannot delete {}: {e}", path.display()),
        }
    }

    let sources: Vec<&PathBuf> = gone.iter().map(|p| &p.source).collect();
    log::info!("cleared {} photo(s) from the filmstrip", sources.len());
    let _ = app.emit("cleared", &sources);
    sources.len()
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
        .ok_or_else(|| format!("{} is not in the index", source.display()))?;
    if photo.status != Status::Done {
        return Err("that photo has not finished processing".into());
    }
    let destination = photo
        .destination
        .clone()
        .filter(|d| d.exists())
        .ok_or("the overlay PNG is gone - try processing the photo again")?;
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

/// Finder, with the PNG selected. The escape hatch for anything the app cannot do.
#[tauri::command]
fn reveal_photo(state: State<AppState>, source: PathBuf) -> Result<(), String> {
    let (destination, _) = destination_of(&state, &source)?;
    std::process::Command::new("open")
        .arg("-R")
        .arg(&destination)
        .status()
        .map_err(|e| format!("cannot open Finder: {e}"))
        .and_then(|s| s.success().then_some(()).ok_or("Finder refused".into()))
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
        // Size and position only. A kiosk that was quit while minimised, maximised,
        // fullscreen or hidden should still come back as a plain visible window at
        // the frame the operator set up.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::SIZE | StateFlags::POSITION)
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
            reprocess,
            clear_photo,
            clear_all_photos,
            runtime_status,
            share_photo,
            print_photo,
            reveal_photo,
        ])
        // Files dropped on the window. Tauri owns drag-and-drop (dragDropEnabled is
        // on, the default), so the webview never sees the drop and cannot navigate
        // to the file - which is what it did while the flag was off.
        .on_window_event(|window, event| {
            if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event {
                for path in paths {
                    ingest(window.app_handle(), path.clone());
                }
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let state = app.state::<AppState>();

            let settings = Settings::load(&handle);
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
                        settings.pipeline_config(),
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
                        let error = if settings.watch_enabled {
                            start_watching(&boot, &settings).err().inspect(|e| log::error!("{e}"))
                        } else {
                            log::info!("watching is paused, not watching {}", settings.watch_dir.display());
                            None
                        };
                        set_runtime_status(&boot, RuntimeStatus { ready: true, error });
                    }
                    Err(error) => {
                        log::error!("the pipeline is unavailable: {error}");
                        set_runtime_status(
                            &boot,
                            RuntimeStatus { ready: false, error: Some(error) },
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
                        Ok(path) => ingest(app, path),
                        Err(()) => log::warn!("asked to open {url}, which is not a file"),
                    }
                }
            }
            _ => {}
        });
}
