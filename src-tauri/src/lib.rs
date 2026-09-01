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

use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

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
    /// Dropping the watcher stops it, so it has to be held for the app's lifetime.
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
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
    if state
        .index
        .lock()
        .unwrap()
        .get(&source)
        .is_some_and(|p| matches!(p.status, Status::Processing | Status::Done))
    {
        log::debug!("{} is already handled, ignoring", source.display());
        return;
    }

    let mut photo = Photo::queued(source.clone());
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

    start_watching(&app, &updated)?;
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
        })
        .invoke_handler(tauri::generate_handler![
            list_photos,
            get_settings,
            set_watch_dir,
            reprocess,
            runtime_status,
            share_photo,
            print_photo,
            reveal_photo,
        ])
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
                        boot.state::<AppState>()
                            .sidecar
                            .lock()
                            .unwrap()
                            .replace(Arc::new(sidecar));

                        // The pipeline is usable either way here: a watch that
                        // failed to start still leaves `mise run overlay` and the
                        // retry button working, so this is reported rather than
                        // treated as the app being broken.
                        let error = start_watching(&boot, &settings).err().inspect(|e| log::error!("{e}"));
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
        .run(|app, event| {
            // Closing stdin is the documented shutdown for `Server#run`, which reads
            // to EOF. Without this the child outlives the app.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                if let Some(sidecar) = app.state::<AppState>().sidecar.lock().unwrap().take() {
                    sidecar.shutdown();
                }
            }
        });
}
