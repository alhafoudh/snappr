//! Watches the drop folder for photos that have finished arriving.
//!
//! Three behaviours here are carried over from the Ruby watcher rather than
//! reinvented, because each was established by measurement and each is easy to get
//! wrong in a way that only shows up at an event.
//!
//! **The watch must not recurse.** `RecursiveMode::NonRecursive`, and it is about
//! CPU, not tidiness. The Ruby version passed listen `ignore: %r{/}` because
//! listen's macOS adapter turns every fsevent into a recursive rescan of the
//! changed directory, and the drop folder is `~/Downloads`: about 1.9k entries at
//! the top level but about 19k once its 89 subdirectories are walked. One unrelated
//! file changing cost 0.5-5s of CPU, and since snappr writes its own 10-25 MB PNG
//! into that same folder, each photo pinned a core for 63 seconds after it was
//! already done. AirDrop only ever lands files at the top level, so nothing is lost.
//!
//! **The candidate filter runs before anything touches the disk.** It also has to
//! reject snappr's own output, including the in-progress `.tmp-<pid>.overlay.png`,
//! which is why the suffix test covers both.
//!
//! **Arrivals are debounced on size and mtime.** AirDrop writes non-atomically, so
//! a file that exists is not a file that is finished; without this the pipeline
//! reads half a photo.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use notify::{Event, RecursiveMode, Watcher as _};

use crate::settings::Settings;

const PARTIAL_EXTENSIONS: [&str; 3] = ["download", "part", "crdownload"];

/// How often to re-check whether a still-arriving file has settled.
const POLL: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq)]
struct Fingerprint {
    len: u64,
    modified: Option<SystemTime>,
}

/// A file is considered finished once its size and mtime have been identical for
/// `stability_checks` consecutive polls. Size 0 never counts: an AirDrop transfer
/// creates the file before it writes anything to it.
fn fingerprint(path: &Path) -> Option<Fingerprint> {
    let meta = std::fs::metadata(path).ok()?;
    (meta.len() > 0).then(|| Fingerprint {
        len: meta.len(),
        modified: meta.modified().ok(),
    })
}

pub fn is_candidate(path: &Path, settings: &Settings) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();

    // The self-output guard. Also catches the in-progress `.tmp-<pid>.overlay.png`,
    // which carries the same suffix precisely so one test covers both.
    if lower.ends_with(&settings.output_suffix.to_ascii_lowercase()) {
        return false;
    }

    let Some(extension) = Path::new(&lower).extension().and_then(|e| e.to_str()) else {
        return false;
    };
    if PARTIAL_EXTENSIONS.contains(&extension) {
        return false;
    }
    settings.extensions.iter().any(|allowed| allowed == extension)
}

/// Watches `dir`, calling `on_photo` once per photo, after it has stopped changing.
/// The returned watcher must be kept alive; dropping it stops the watch.
pub fn spawn<F>(dir: PathBuf, settings: Settings, on_photo: F) -> Result<notify::RecommendedWatcher, String>
where
    F: Fn(PathBuf) + Send + 'static,
{
    let (tx, rx) = channel::<Event>();

    let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
        if let Ok(event) = result {
            let _ = tx.send(event);
        }
    })
    .map_err(|e| format!("cannot create a watcher: {e}"))?;

    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .map_err(|e| format!("cannot watch {}: {e}", dir.display()))?;

    thread::spawn(move || {
        // Files seen but not yet settled: path -> (last fingerprint, how many
        // consecutive polls it has looked the same, when we first saw it).
        let mut arriving: HashMap<PathBuf, (Fingerprint, u32, Instant)> = HashMap::new();

        loop {
            // Drain whatever the watcher has produced without blocking, then poll.
            // The sender lives inside the watcher's callback, so once the caller
            // drops the watcher the channel disconnects - that is this thread's
            // signal to finish what is arriving and exit, rather than sleeping
            // forever. Every directory change and every toggle of the watch would
            // otherwise leave one more idle thread behind.
            let mut disconnected = false;
            loop {
                let event = match rx.try_recv() {
                    Ok(event) => event,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                };
                for path in event.paths {
                    if is_candidate(&path, &settings) && !arriving.contains_key(&path) {
                        if let Some(print) = fingerprint(&path) {
                            arriving.insert(path, (print, 1, Instant::now()));
                        } else {
                            // Exists but empty: a transfer that has only just begun.
                            arriving.insert(
                                path,
                                (Fingerprint { len: 0, modified: None }, 0, Instant::now()),
                            );
                        }
                    }
                }
            }

            let mut settled = Vec::new();
            let mut abandoned = Vec::new();

            for (path, (last, stable_for, first_seen)) in arriving.iter_mut() {
                match fingerprint(path) {
                    Some(current) if current == *last => {
                        *stable_for += 1;
                        if *stable_for >= settings.stability_checks {
                            settled.push(path.clone());
                        }
                    }
                    Some(current) => {
                        *last = current;
                        *stable_for = 1;
                    }
                    // Gone, or still zero-length. Not an error yet: AirDrop creates
                    // the file before it has anything to put in it.
                    None => *stable_for = 0,
                }

                if first_seen.elapsed() > settings.stability_timeout() {
                    abandoned.push(path.clone());
                }
            }

            for path in abandoned {
                if !settled.contains(&path) {
                    log::warn!(
                        "{} never stopped changing after {}s, giving up on it",
                        path.display(),
                        settings.stability_timeout().as_secs()
                    );
                }
                arriving.remove(&path);
            }
            for path in settled {
                arriving.remove(&path);
                log::info!("{} has settled", path.display());
                on_photo(path);
            }

            if disconnected && arriving.is_empty() {
                log::info!("stopped watching {}", dir.display());
                break;
            }

            thread::sleep(POLL);
        }
    });

    Ok(watcher)
}
