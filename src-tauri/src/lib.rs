//! snappr: the window, and everything that is not a pixel.
//!
//! This side owns the window, the settings, the persisted index, printing and
//! sharing, and the drops. `snappr-core` owns the pipeline and the watch: one
//! photo in, an overlay PNG and three previews out.
//!
//! **There is no child process any more.** The pipeline used to be a Ruby
//! interpreter on the far end of an NDJSON pipe, and the shape of this file still
//! shows where that boundary was - work goes through
//! [`snappr_core::worker::Worker`], which is one thread and one queue, because the
//! child's one-request-at-a-time behaviour was the only thing stopping four 12MP
//! composites from running at once.

#[cfg(target_os = "macos")]
mod mac;
mod overlays;
mod photo;
mod problem;
mod settings;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use snappr_core::overlay::Overlay as Pipeline;
use snappr_core::watcher::{self, Arrival, Refusal};
use snappr_core::worker::Worker;
use tauri::{AppHandle, DragDropEvent, Emitter, Manager, State, WebviewWindow, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_window_state::StateFlags;

use overlays::OverlayCard;
use photo::{Index, Photo, Status};
use problem::{detail, name_of, Payload, Problem};
use settings::{Orientation, Overlay, Settings};

/// Whether the pipeline is usable, and why not if it isn't.
///
/// Held in state as well as emitted. Events are not replayed and the pipeline is
/// ready within a millisecond of launch - comfortably before the webview has
/// finished attaching its listeners - so a UI that only subscribed would wait
/// forever for a message that had already been sent. The frontend pulls this on
/// mount and subscribes for later changes; both paths read the same value.
///
/// It used to take about a second, because a Ruby interpreter had to start and
/// answer a handshake. Now the only thing that can go wrong is libvips failing to
/// initialise, which means a broken install rather than a slow one.
#[derive(Debug, Clone, Default, serde::Serialize, ts_rs::TS)]
#[ts(export, optional_fields)]
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
    runtime: Mutex<RuntimeStatus>,
    /// The pipeline's one worker thread. Every composite, preview and card render
    /// goes through it, in the order they were asked for.
    ///
    /// `None` only if libvips could not be initialised, which is the one failure
    /// [`RuntimeStatus::error`] reports. There is no window where the pipeline is
    /// merely *not up yet*, which is why `pending` is gone: a drop on the Dock
    /// icon at launch used to have to be parked until a child process had
    /// handshaken.
    worker: Mutex<Option<Worker>>,
    /// Dropping the watcher stops it, so it has to be held for as long as the
    /// operator wants the folder watched. `None` is the paused state.
    watcher: Mutex<Option<watcher::Handle>>,
    /// What the pipeline made of each overlay, keyed the same way its PNG is
    /// named. Not persisted: the PNGs on disk are, and a render is cheap enough
    /// that losing the map across a launch costs one pass through the panel.
    overlays: Mutex<overlays::Cache>,
    /// Held for the whole of one `overlays::refresh`, so only one runs at a time.
    ///
    /// Not about the cache - that has its own lock - but about the collection pass
    /// at the end. Two refreshes overlapping means the older one sweeps with a
    /// stale idea of which overlays exist and deletes the PNG the newer one has
    /// just written, leaving a card pointing at a file that is gone. Adding an
    /// overlay while the boot refresh is still going is exactly that race.
    drawing: Mutex<()>,
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
    // **What is emitted is what was stored, not what was passed in.** `upsert`
    // keeps the index's own three fields off an incoming record (see its comment),
    // so the argument and the stored record can differ - and the frontend has to be
    // told the version that won, or the cell disagrees with the index until the next
    // event about that photo.
    let stored = {
        let mut index = state.index.lock().unwrap();
        let stored = index.upsert(photo.clone());
        if let Err(e) = index.save(app) {
            log::warn!("cannot persist the index: {e}");
        }
        stored
    };
    let _ = app.emit("photo", &stored);
}

/// Sets or clears a photo's print mark and publishes the record.
///
/// Both ways of marking go through here, so the index write and the cell on screen
/// cannot disagree about which photos have been printed.
fn set_printed(app: &AppHandle, source: &Path, printed: bool) -> Result<(), Payload> {
    // One statement, so the index guard is dropped at the `;` - `publish` takes the
    // same lock. Binding this with `if let` instead holds the guard across the body
    // and deadlocks, which is why `reprocess` below is written the same way.
    let updated = app
        .state::<AppState>()
        .index
        .lock()
        .unwrap()
        .update(source, |photo| photo.printed(printed));

    match updated {
        Some(photo) => {
            publish(app, &photo);
            Ok(())
        }
        None => Err(Problem::PhotoForgotten { path: source.to_path_buf() }.say()),
    }
}

/// Sends one photo through the pipeline and publishes the outcome.
///
/// Called from a thread that is not the main one, and the composite itself is
/// queued on the worker - so this blocks twice over and must never run on the UI
/// thread. A 12MP composite takes about a second.
fn process(app: AppHandle, source: PathBuf) {
    let state = app.state::<AppState>();

    // Already known and finished? Then this is a duplicate event, not a new photo.
    // Known but queued or failed? Then this is a retry of the same photo, and it
    // keeps its arrival time so it neither moves along the strip nor restarts its
    // "added N minutes ago" label. Whatever previews that record owned are the ones
    // this run supersedes - see discard_superseded.
    //
    // Its arrival time, print mark and blue dot are kept by `Index::upsert` rather
    // than carried across by hand here - see its comment. A retry therefore neither
    // moves along the strip, nor un-prints a photo whose paper has already come out
    // of the printer, nor re-dots the photo the operator is looking at, and none of
    // those depends on this function remembering to copy a field.
    let (superseded, busy) = {
        let index = state.index.lock().unwrap();
        let busy = index.selected().is_some();
        let superseded = match index.get(&source) {
            Some(p) if matches!(p.status, Status::Processing | Status::Done) => {
                log::debug!("{} is already handled, ignoring", source.display());
                return;
            }
            Some(p) => p.owned_previews(),
            None => Vec::new(),
        };
        (superseded, busy)
    };

    let mut photo = Photo::queued(source.clone());
    // A blue dot means "this landed while you were busy with another photo", so an
    // arrival with nothing selected at all does not get one: it is what the window
    // is about to show, not something that happened behind the operator's back. The
    // frontend follows exactly this arrival for the same reason - `nothingSelected`
    // in `usePhotos.ts` reads the selection this is asking about.
    //
    // Only consulted for a genuinely new record: on a retry `upsert` keeps the mark
    // the record already had.
    photo.unseen = busy;
    photo.status = Status::Processing;
    publish(&app, &photo);

    let Some(worker) = state.worker.lock().unwrap().clone() else {
        photo.fail(Problem::ImageToolsUnavailable { detail: "no worker".into() }.say());
        publish(&app, &photo);
        return;
    };

    // The config is read now, not inside the job, so a photo composites with the
    // overlays that were live when it arrived rather than whatever the operator
    // switched to while it waited in the queue.
    let config = state.settings.lock().unwrap().pipeline_config(&app);
    let preview_dir = match Index::preview_dir(&app) {
        Ok(dir) => dir,
        Err(payload) => {
            photo.fail(payload);
            publish(&app, &photo);
            return;
        }
    };

    let outcome = worker
        .submit(move || {
            let pipeline = Pipeline::new(config.clone());
            let outcome = pipeline.process(&source)?;
            // **Previews are derived from the finished files**, and their
            // destination is the app's own data directory - never the watched
            // folder, where a JPEG preview would be composited in its turn and
            // the pipeline would eat its own output forever.
            let previews = snappr_core::Preview::new(config, preview_dir)
                .call(&outcome.source, &outcome.destination)?;
            Ok::<_, Problem>((outcome, previews))
        })
        .recv();

    match outcome {
        Ok(Ok((outcome, previews))) => photo.complete(outcome, previews),
        Ok(Err(problem)) => {
            // One bad photo must not take the app down, and the original is still
            // sitting in the drop folder, so retrying is always possible. This
            // adds which photo it was about; the sentence itself is the
            // catalogue's.
            log::error!("{}: {}", source_of(&photo), problem.log_line());
            photo.fail(problem.payload());
        }
        // The worker thread died. Nothing else can be printed until the app is
        // restarted, so say the same thing a broken install says.
        Err(_) => photo.fail(
            Problem::ImageToolsUnavailable { detail: "the worker stopped".into() }.say(),
        ),
    }
    publish(&app, &photo);
    discard_superseded(superseded, &photo);
}

fn source_of(photo: &Photo) -> String {
    photo.source.display().to_string()
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
    // An SVG is artwork, not a photo, so it goes to the overlay panel. Split by
    // kind rather than by where in the window the file landed: only Rust ever sees
    // a drop's position, and a drop on the Dock icon has no position at all - so
    // a panel-shaped drop target could not be made to work from the Dock, and an
    // SVG dropped an inch to the left of it would be refused for no reason the
    // operator could see. Dropping one anywhere adds it; the panel lights up
    // because the frontend classifies the drag the same way.
    let (drawings, photos): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|path| is_drawing(path));

    if !drawings.is_empty() {
        add_overlays(app, drawings);
    }

    let refused: Vec<(PathBuf, Refusal)> = photos
        .into_iter()
        .filter_map(|path| ingest_one(app, path).err())
        .collect();

    if !refused.is_empty() {
        explain_refusals(app, refused);
    }
}

/// The one place "is this an overlay?" is decided, so the drop router and
/// `add_overlay` cannot disagree about what the panel accepts.
fn is_drawing(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
}

/// Adds dropped drawings to the library and redraws the panel.
///
/// Quiet on a duplicate: dropping a design the operator already has is not a
/// mistake worth an alert, and the card is right there showing it is already
/// present. A *picked* duplicate does say so - see `add_overlay` - because the
/// operator went through a dialog to choose it and silence would read as a
/// failure.
fn add_overlays(app: &AppHandle, paths: Vec<PathBuf>) {
    let state = app.state::<AppState>();
    let mut settings = state.settings.lock().unwrap();
    let added = paths
        .into_iter()
        .filter(|path| path.is_file())
        .filter(|path| settings.add_overlay(Overlay::Custom(path.clone())))
        .count();
    if added == 0 {
        return;
    }
    if let Err(e) = settings.save(app) {
        log::warn!("cannot save the overlay library: {e}");
    }
    drop(settings);

    overlays::announce(app);
    overlays::refresh(app.clone());
}

/// The entry point for one photo that arrived by hand - dropped on the window or
/// on the Dock icon - rather than through the watcher. Same candidate filter, same
/// `process`, so a drop behaves exactly like an AirDrop that landed in the folder.
/// Only the stability wait is skipped: a dropped file is already complete.
///
/// A drop that arrives during launch needs no special handling any more. It used
/// to be parked in a queue with its own lock discipline, because the Ruby child
/// took about a second to start and a drop on the Dock icon can be what launches
/// the app; the pipeline is now up before the window is.
fn ingest_one(app: &AppHandle, path: PathBuf) -> Result<(), (PathBuf, Refusal)> {
    let state = app.state::<AppState>();

    let refusal = {
        let config = state.settings.lock().unwrap().pipeline_config(app);
        watcher::refusal(&path, &config)
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

    log::info!("{} handed to snappr, processing", path.display());
    // On a thread because `process` blocks twice: once on the worker queue and
    // once on the composite. This is the event thread.
    let handle = app.clone();
    std::thread::spawn(move || process(handle, path));
    Ok(())
}

/// The catalogue entry for one refusal.
fn refusal_problem(path: &Path, refusal: Refusal, accepted: &str) -> Problem {
    let file = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    match refusal {
        Refusal::AlreadyAPrint => Problem::DropAlreadyAPrint { file },
        Refusal::IsAFolder => Problem::DropIsAFolder { file },
        Refusal::StillDownloading => Problem::DropStillDownloading { file },
        Refusal::Unnamed => Problem::DropUnreadableName { path: path.to_path_buf() },
        // Unknown reaches here only for an extensionless file that is not a
        // folder, which reads the same way to an operator as a wrong extension.
        Refusal::Unknown | Refusal::WrongKind => Problem::DropWrongKind {
            kind: path
                .extension()
                .map(|e| format!("a .{} file", e.to_string_lossy()))
                .unwrap_or_else(|| "not a kind of file Snappr knows".into()),
            file,
            accepted: accepted.to_string(),
        },
    }
}

/// Says why a drop was refused, in a native alert.
///
/// The one modal that is not a question. `ask` covers those; this one *tells*,
/// because a refused drop is the failure that otherwise produces **nothing at
/// all**: no cell, no spinner, no photo, and no line either - the problem line
/// belongs to the selected photo and this drop never created one. Everything
/// else that can go wrong has somewhere on screen it belongs.
///
/// The OS's alert rather than a Radix dialog, for the reason CLAUDE.md gives for
/// the context menus, plus one this side: a drop on the Dock icon can be what
/// *launches* the app, and a frontend-rendered modal cannot explain a drop that
/// happened before the webview existed.
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
        .map(|(path, refusal)| refusal_problem(path, *refusal, &accepted).warn().message)
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
    let mut dialog = app
        .dialog()
        .message(body)
        .title(title)
        // A warning, not an error: the operator did nothing wrong, snappr just
        // cannot use what they dropped.
        .kind(MessageDialogKind::Warning);

    // Parented so this is a sheet on the window, the way `ask` is. A drop on the
    // Dock icon can be what *launches* the app, though, so there is not always a
    // window to hang one from - unparented then, which is the old behaviour and
    // the only thing available.
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.parent(&window);
    }

    // `show`, never `blocking_show`: unlike `ask` this runs on the event thread,
    // and blocking there freezes the window.
    dialog.show(|_| {});
}

fn start_watching(app: &AppHandle, settings: &Settings) -> Result<(), Payload> {
    let handle = app.clone();
    let config = settings.pipeline_config(app);
    // By value: the closure outlives this call, so it cannot borrow anything here.
    let timeout = config.stability_timeout_secs;
    let watcher = watcher::spawn(config.watch_dir.clone(), config, move |arrival| {
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
            // settles it after all and `process` replaces the record in place.
            // Giving up is only about the message; the watcher keeps the path
            // under observation, which it has to do by polling because the writes
            // that would finish the transfer are not arrivals. See
            // `Arriving::stalled` in watcher.rs.
            Arrival::Abandoned(path) => {
                let mut photo = Photo::queued(path.clone());
                let said = Problem::TransferStalled {
                    file: photo.name.clone(),
                    path,
                    secs: timeout,
                }
                .warn();
                photo.fail(Payload { code: said.kind, message: said.message });
                publish(&handle, &photo);
            }
        }
    })
    .map_err(Problem::say)?;

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
fn start_watching_noted(app: &AppHandle, settings: &Settings) -> Result<(), Payload> {
    let result = start_watching(app, settings);
    set_watch_error(app, result.as_ref().err().map(|payload| payload.message.clone()));
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
fn set_selected(app: AppHandle, state: State<AppState>, source: PathBuf) -> Result<(), Payload> {
    // Looking at a photo is what clears its blue dot, so the mark is cleared here
    // rather than by a command of its own. Every path that selects - the keyboard,
    // a click, a right-click, the one arrival a session follows, the reconciliation
    // after a clear - already funnels through the single effect in `usePhotos` that
    // calls this, and a separate `mark_seen` would be a second thing all of them
    // had to remember.
    let cleared = {
        let mut index = state.index.lock().unwrap();
        let moved = index.select(source.clone());
        // Only when there is a mark to clear, so arrowing along the strip does not
        // emit a photo event per cell it passes.
        let marked = index.get(&source).is_some_and(|photo| photo.unseen);
        let cleared = if marked {
            index.update(&source, |photo| photo.unseen = false)
        } else {
            None
        };
        // `select` reports whether it actually changed so that re-sending the value
        // costs no write; the mark is a second reason to write, so both count.
        if moved || cleared.is_some() {
            index.save(&app)?;
        }
        cleared
    };

    // Emitted rather than `publish`ed: the record is already in the index and
    // already saved, and `publish` would upsert and write it a second time. The
    // block above is what drops the guard first - see `set_printed`.
    if let Some(photo) = cleared {
        let _ = app.emit("photo", &photo);
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
fn set_watch_dir(app: AppHandle, state: State<AppState>, dir: PathBuf) -> Result<Settings, Payload> {
    if !dir.is_dir() {
        return Err(Problem::NotAFolder { path: dir }.say());
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

/// Sets what one orientation composites: one of the overlays in the panel, or
/// nothing.
///
/// This is the whole of what the cards' toggles do. Lighting a card's landscape
/// icon sends that card's overlay; turning it off sends `None`. Exclusivity comes
/// for free because a slot holds one value, so switching an overlay on
/// automatically switches it off wherever it was - no second command, and no way
/// for the panel to show two cards lit for one orientation.
///
/// Neither state is broken. Nothing at all still prints - the photo is cropped to
/// the print ratio with nothing over it - and the sample is what a fresh install
/// lands on so that a first photo shows something rather than looking like the app
/// did not run.
///
/// Only that a picked file exists is checked. An SVG with no #FF00FF window, or one
/// drawn at the wrong shape, comes back as a warning on the photo itself - which
/// the filmstrip and the Actions bar already show - and now on the card as well,
/// rather than as a rejected pick.
///
/// Photos already in the strip keep their output. Regenerating one is what the
/// retry button is for; silently rewriting a strip's worth of 10-25 MB PNGs
/// because a toggle was clicked is not worth the surprise.
#[tauri::command]
fn set_overlay(
    app: AppHandle,
    state: State<AppState>,
    orientation: Orientation,
    choice: Overlay,
) -> Result<Settings, Payload> {
    if let Overlay::Custom(path) = &choice {
        if !path.is_file() {
            return Err(
                Problem::OverlayFileGone { file: name_of(path), path: path.clone() }.say()
            );
        }
    }

    let updated = {
        let mut settings = state.settings.lock().unwrap();
        *settings.slot_mut(orientation) = choice;
        settings.save(&app)?;
        settings.clone()
    };

    // The resolved path rather than the choice, because that is what the pipeline
    // was actually handed and what a puzzled log reader needs to see.
    let resolved = updated.slot(orientation).resolve(&app, orientation);
    log::info!(
        "{orientation:?} overlay is now {}",
        resolved.map_or_else(|| "nothing".to_string(), |p| p.display().to_string())
    );
    overlays::announce(&app);
    Ok(updated)
}

// `push_overlays` is gone, and nothing replaced it.
//
// It used to send the overlay settings to the running child, so the next photo
// would use them - a `configure` frame that rebuilt the child's whole Config,
// Overlay and Preview. There is no child and no copy of the settings to keep in
// step: `process` reads `pipeline_config` when it queues a photo, so switching an
// overlay takes effect on the next one by construction. Saving is now the whole
// job of `set_overlay` and `remove_overlay`.

/// The panel's cards, drawn or still being drawn.
///
/// The pull half of pull-then-subscribe, like `list_photos`: the renders may well
/// have finished before the webview attached its listener, and events are not
/// replayed.
#[tauri::command]
fn list_overlays(app: AppHandle, state: State<AppState>) -> Vec<OverlayCard> {
    let settings = state.settings.lock().unwrap().clone();
    overlays::cards(&app, &settings)
}

/// Adds an SVG the operator picked to the library.
///
/// Says so on a duplicate, where a drop stays quiet: this one cost a trip through
/// a file dialog, and a pick that appeared to do nothing would read as a failure.
///
/// Adding does not switch anything on. The card arrives with both toggles dark and
/// the operator lights the one they want - an add that took a slot would overwrite
/// a choice they had already made.
#[tauri::command]
fn add_overlay(app: AppHandle, state: State<AppState>, path: PathBuf) -> Result<Settings, Payload> {
    if !is_drawing(&path) || !path.is_file() {
        return Err(
            Problem::OverlayNotADrawing { file: name_of(&path), path: path.clone() }.say()
        );
    }

    let updated = {
        let mut settings = state.settings.lock().unwrap();
        if !settings.add_overlay(Overlay::Custom(path.clone())) {
            let said = Problem::OverlayAlreadyAdded { file: name_of(&path) }.warn();
            return Err(Payload { code: said.kind, message: said.message });
        }
        settings.save(&app)?;
        settings.clone()
    };

    log::info!("added the overlay {}", path.display());
    overlays::announce(&app);
    overlays::refresh(app.clone());
    Ok(updated)
}

/// Takes an overlay out of the library, and out of any slot that was using it.
///
/// **Deletes nothing the operator can see.** The SVG is theirs and stays where it
/// is - only snappr's own rendering of it is collected, on the next refresh. The
/// sample cannot be removed at all; its card offers no Remove item, and
/// `Settings::remove_overlay` refuses it as well.
#[tauri::command]
fn remove_overlay(app: AppHandle, state: State<AppState>, overlay: Overlay) -> Result<Settings, Payload> {
    let updated = {
        let mut settings = state.settings.lock().unwrap();
        if !settings.remove_overlay(&overlay) {
            // Nothing to do rather than a failure: the card is already gone, which
            // is what the operator asked for.
            return Ok(settings.clone());
        }
        settings.save(&app)?;
        settings.clone()
    };
    // A removal can empty a slot, and the next photo reads the slots when it is
    // queued - so the panel and the pipeline cannot disagree about what is live.
    // This used to need a push to the child, whose own copy of the settings would
    // otherwise have kept compositing branding the panel no longer showed.

    log::info!("removed the overlay {}", overlay.label());
    overlays::announce(&app);
    overlays::refresh(app.clone());
    Ok(updated)
}

/// Shows an overlay's SVG in Finder - the sample included, which is how the
/// operator gets at the worked example to copy and start a real one from.
///
/// The sample is revealed in place rather than exported as a copy, which is worth
/// knowing the shape of: in a built `.app` it sits in `Contents/Resources`, so
/// Finder opens a package-contents view, the file is read-only, and an app run from
/// a mounted DMG is on a read-only volume besides. Under `tauri dev` the target is
/// the build script's copy in `target/debug/sample/`, and edits there are
/// overwritten by the next build. Either way the operator copies it somewhere of
/// their own first, which is the intended gesture - this is a "here is the file"
/// item, not an "edit this" one. A card the operator added is their own file and
/// has none of that.
#[tauri::command]
fn reveal_overlay(app: AppHandle, overlay: Overlay) -> Result<(), Payload> {
    // Two different absences, and the operator can act on only one of them. A
    // custom overlay whose file has moved is theirs to put back; a sample this
    // build does not ship is not, so that one gets the generic sentence.
    let Some(path) = overlay.resolve(&app, Orientation::Landscape) else {
        return Err(Problem::FinderWontOpen { detail: format!("{} resolves to nothing", overlay.label()) }.say());
    };
    if !path.is_file() {
        return Err(Problem::OverlayFileGone { file: name_of(&path), path: path.clone() }.say());
    }
    reveal(&path)
}

/// Remembers whether the panel is a column of cards or a rail.
///
/// Persisted rather than kept in the webview: overlays are set up once and then
/// left alone, and a kiosk that gave the photo back 180px on every launch would
/// have to be collapsed again every morning.
#[tauri::command]
fn set_panel_collapsed(app: AppHandle, state: State<AppState>, collapsed: bool) -> Result<Settings, Payload> {
    let mut settings = state.settings.lock().unwrap();
    settings.overlays_collapsed = collapsed;
    settings.save(&app)?;
    Ok(settings.clone())
}

/// Pauses or resumes the folder watch. Dropping the watcher is what stops it. The
/// choice is persisted, so a kiosk comes back the way it was left; the toggle in
/// the bar always shows which.
#[tauri::command]
fn set_watching(app: AppHandle, state: State<AppState>, enabled: bool) -> Result<Settings, Payload> {
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
fn reprocess(app: AppHandle, state: State<AppState>, source: PathBuf) -> Result<(), Payload> {
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
fn clear_photo(app: AppHandle, state: State<AppState>, source: PathBuf) -> Result<(), Payload> {
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
fn clear_all_photos(app: AppHandle, state: State<AppState>) -> Result<usize, Payload> {
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
fn destination_of(state: &State<AppState>, source: &Path) -> Result<(PathBuf, bool), Payload> {
    let index = state.index.lock().unwrap();
    let photo = index
        .get(source)
        .ok_or_else(|| Problem::PhotoForgotten { path: source.to_path_buf() }.say())?;
    if photo.status != Status::Done {
        return Err(Problem::PhotoStillWorking.say());
    }
    let destination = photo
        .destination
        .clone()
        .filter(|d| d.exists())
        .ok_or_else(|| Problem::PrintFileGone.say())?;
    Ok((destination, photo.orientation == Some(Orientation::Landscape)))
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
) -> Result<(), Payload> {
    let (destination, _) = destination_of(&state, &source)?;
    mac::share(&window, &destination, anchor)
}

/// Opens the print dialog, and marks the photo printed.
///
/// **The mark is set when the dialog opens, not when it is confirmed**, so
/// cancelling the sheet still leaves the cell marked. macOS can tell the two
/// apart - `runOperationModalForWindow:delegate:didRunSelector:contextInfo:` is
/// handed `None` for both the delegate and the selector, and that pair is exactly
/// the callback carrying `didRun: BOOL` - but wiring an Objective-C delegate class
/// for it was judged not worth the cost. `set_photo_printed` is the correction,
/// and a cancelled print is the ordinary reason to reach for it.
///
/// After `mac::print` rather than before: a dispatch that never reached the main
/// thread put nothing on screen, so it should mark nothing either.
#[tauri::command]
fn print_photo(window: WebviewWindow, state: State<AppState>, source: PathBuf) -> Result<(), Payload> {
    let (destination, landscape) = destination_of(&state, &source)?;
    let paper = state.settings.lock().unwrap().print_paper_name.clone();
    // The job is named after the SOURCE, not the PNG being printed: the
    // destination's stem is `IMG_1234.overlay`, and the operator thinks of the
    // print as the photo they dropped.
    let title = source.file_stem().unwrap_or_default().to_string_lossy();
    mac::print(&window, &destination, landscape, &paper, &title)?;
    set_printed(window.app_handle(), &source, true)
}

/// The operator's override for the mark, from the cell's context menu.
///
/// A print that jammed, one made from Finder, or - most often - a print dialog
/// that was opened and then cancelled, which `print_photo` marks anyway.
#[tauri::command]
fn set_photo_printed(app: AppHandle, source: PathBuf, printed: bool) -> Result<(), Payload> {
    set_printed(&app, &source, printed)
}

/// Finder, with the file selected.
fn reveal(path: &Path) -> Result<(), Payload> {
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
fn reveal_photo(state: State<AppState>, source: PathBuf) -> Result<(), Payload> {
    let (destination, _) = destination_of(&state, &source)?;
    reveal(&destination)
}

/// Asks the operator a yes/no question in the OS's own alert, and answers with
/// what they clicked.
///
/// **The single place a question is put to the operator.** The wording arrives
/// from `src/confirm.ts`, which is the only place a prompt is spelled;
/// everything else about the alert is fixed here on purpose. `Warning` because a
/// confirm is only ever asked for something destructive, and "Cancel" because
/// that is the label a macOS user reads as "get me out of this" - a call site
/// free to invent "No" or "Keep" is a call site that eventually will.
///
/// `.parent(&window)` is what makes it a document-modal sheet on the window
/// rather than a panel floating in the middle of the screen. The frontend's
/// `plugin-dialog` `confirm` used to get this for free, because the plugin's own
/// command sets it (`tauri-plugin-dialog/src/commands.rs`), and dropping it on
/// the way over here would have been a silent regression in how native the app
/// looks.
///
/// **`blocking_show` is right here, and the rule it looks like it breaks does not
/// apply.** `explain_refusals` below must never block, because it runs on the
/// event thread. This is an `async` command, so Tauri runs it on the async
/// runtime and the event thread is never held - which is exactly what
/// `tauri-plugin-dialog`'s own `message` command does. Do not "fix" this into
/// `show` with a channel to bridge the callback: that is ceremony for nothing.
///
/// Returns a bare `bool` rather than the `Result<_, Payload>` every other
/// fallible command returns, because nothing here can fail. A sheet dismissed
/// with Escape is already `false`.
#[tauri::command]
async fn ask(window: WebviewWindow, title: String, question: String, confirm: String) -> bool {
    window
        .dialog()
        .message(question)
        .title(title)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(confirm, "Cancel".into()))
        .parent(&window)
        .blocking_show()
}

/// The folder picker for the watch folder.
///
/// Raised from here rather than from the webview for the reason `ask` is: every
/// dialog in the app comes from this side, and `capabilities/default.json`
/// granting no `dialog:*` permission at all is what keeps that checkable.
/// `async` for the same reason too - see `ask`.
#[tauri::command]
async fn pick_watch_dir(window: WebviewWindow, current: Option<PathBuf>) -> Option<PathBuf> {
    let mut dialog = window.dialog().file().set_parent(&window);
    // Opening in the folder being replaced, which is where the operator is
    // almost always about to navigate away from anyway. `is_dir` rather than
    // handing the path over unchecked: the usual reason to be changing the folder
    // is that the old one has gone, and AppKit silently ignores a directory URL
    // that does not resolve, so the guard is what makes "then just open wherever
    // the panel would" a decision rather than a coincidence.
    if let Some(current) = current.filter(|p| p.is_dir()) {
        dialog = dialog.set_directory(current);
    }
    dialog.blocking_pick_folder().and_then(|p| p.into_path().ok())
}

/// The file picker for adding an overlay to the panel. See `pick_watch_dir`.
#[tauri::command]
async fn pick_overlay_file(window: WebviewWindow) -> Option<PathBuf> {
    window
        .dialog()
        .file()
        .set_parent(&window)
        .add_filter("SVG", &["svg"])
        .blocking_pick_file()
        .and_then(|p| p.into_path().ok())
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
            runtime: Mutex::new(RuntimeStatus::default()),
            worker: Mutex::new(None),
            watcher: Mutex::new(None),
            overlays: Mutex::new(overlays::Cache::new()),
            drawing: Mutex::new(()),
        })
        .invoke_handler(tauri::generate_handler![
            list_photos,
            selected_photo,
            set_selected,
            get_settings,
            set_watch_dir,
            set_watching,
            set_overlay,
            list_overlays,
            add_overlay,
            remove_overlay,
            reveal_overlay,
            set_panel_collapsed,
            reprocess,
            clear_photo,
            clear_all_photos,
            runtime_status,
            share_photo,
            print_photo,
            set_photo_printed,
            reveal_photo,
            ask,
            pick_watch_dir,
            pick_overlay_file,
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

            // Still on a thread of its own, though it no longer needs to be for
            // speed: starting the pipeline is now one libvips init rather than a
            // Ruby interpreter and a handshake. What it buys is that a *broken*
            // install shows up as an error in the window rather than as an app
            // that never opens.
            let boot = handle.clone();
            std::thread::spawn(move || {
                let started = Worker::start();

                match started {
                    Ok(worker) => {
                        boot.state::<AppState>().worker.lock().unwrap().replace(worker);

                        // Nothing to drain. A drop that arrives during launch is
                        // handled where it lands, because there is no longer a
                        // window in which the pipeline is merely not up yet - the
                        // `pending` queue and its lock discipline went with the
                        // child process.

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
                            start_watching(&boot, &settings)
                                .err()
                                .map(|payload| payload.message)
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

                        // Draw the panel's cards. Last, and on its own thread: this
                        // is the one thing here that queues behind photo work, and
                        // a drop that arrived on the Dock icon has already been let
                        // through above. The panel shows skeletons until they land.
                        overlays::refresh(boot.clone());
                    }
                    Err(detail) => {
                        // libvips could not be initialised, which means a broken
                        // install rather than a broken photo. One message where
                        // there used to be seven, because there is no longer a
                        // child process to be starting, unreachable or wedged.
                        set_runtime_status(
                            &boot,
                            RuntimeStatus {
                                ready: false,
                                error: Some(
                                    Problem::ImageToolsUnavailable { detail }.say().message,
                                ),
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
            // Dropping the worker handle ends its thread once the queue drains, so
            // a composite that is already running finishes rather than being cut
            // off mid-write. There is no process to kill any more: this used to
            // have to close a pipe and reap a child.
            tauri::RunEvent::ExitRequested { .. } => {
                app.state::<AppState>().worker.lock().unwrap().take();
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
