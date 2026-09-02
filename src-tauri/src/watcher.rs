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

use crate::problem::{detail, Problem};
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

/// Why a file is not a photo snappr will print.
///
/// Two constraints, both from the hot path. `refusal` runs on every filesystem
/// event in the drop folder - `~/Downloads` has about 1.9k top-level entries and
/// refusal is the *common* answer there - so:
///
/// 1. **No payload.** A `String` here would allocate on every rejected event.
///    The extension is re-derived only when a sentence is actually being written.
/// 2. **No I/O.** `is_candidate` touches no disk today and must not start: a
///    `stat` per event is exactly what the `RecursiveMode::NonRecursive` note at
///    the top of this file exists to avoid. So an extensionless path comes back
///    as `Unknown`, and the drop path - which has one file, not 1.9k - upgrades
///    it to `IsAFolder` with a single `is_dir()`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Refusal {
    /// The name is not readable as UTF-8.
    Unnamed,
    /// snappr's own output, including the in-progress `.tmp-<pid>.overlay.png`.
    AlreadyAPrint,
    /// A `.download`, `.part` or `.crdownload` placeholder.
    StillDownloading,
    /// No extension at all. Might be a folder - only the caller can afford to look.
    Unknown,
    /// A folder. Never returned by `refusal`; only by the drop path's upgrade.
    IsAFolder,
    /// A real extension, just not one snappr prints.
    WrongKind,
}

/// The same test as [`is_candidate`], saying why when it says no.
///
/// This is the single implementation of the rule and `is_candidate` delegates to
/// it, so the watcher's filter and the drop's explanation can never drift apart.
pub fn refusal(path: &Path, settings: &Settings) -> Option<Refusal> {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return Some(Refusal::Unnamed);
    };
    let lower = name.to_ascii_lowercase();

    // The self-output guard. Also catches the in-progress `.tmp-<pid>.overlay.png`,
    // which carries the same suffix precisely so one test covers both.
    if lower.ends_with(&settings.output_suffix.to_ascii_lowercase()) {
        return Some(Refusal::AlreadyAPrint);
    }

    let Some(extension) = Path::new(&lower).extension().and_then(|e| e.to_str()) else {
        return Some(Refusal::Unknown);
    };
    if PARTIAL_EXTENSIONS.contains(&extension) {
        return Some(Refusal::StillDownloading);
    }
    if settings.extensions.iter().any(|allowed| allowed == extension) {
        None
    } else {
        Some(Refusal::WrongKind)
    }
}

pub fn is_candidate(path: &Path, settings: &Settings) -> bool {
    refusal(path, settings).is_none()
}

/// What the poll loop concluded about a file it was watching.
///
/// `Abandoned` is reported rather than only logged because no Photo record has
/// been published for that file yet, so there is nothing on screen for the
/// operator to notice. A stalled AirDrop used to disappear in silence.
pub enum Arrival {
    /// Size and mtime stopped changing: it is a whole photo.
    Settled(PathBuf),
    /// It never stopped changing before the timeout.
    Abandoned(PathBuf),
}

/// Watches `dir`, calling `on_arrival` once per file it reaches a conclusion
/// about. The returned watcher must be kept alive; dropping it stops the watch.
pub fn spawn<F>(dir: PathBuf, settings: Settings, on_arrival: F) -> Result<notify::RecommendedWatcher, String>
where
    F: Fn(Arrival) + Send + 'static,
{
    let (tx, rx) = channel::<Event>();

    let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
        if let Ok(event) = result {
            let _ = tx.send(event);
        }
    })
    .map_err(|e| Problem::CannotWatch { path: &dir, detail: detail(e) }.say())?;

    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .map_err(|e| Problem::CannotWatch { path: &dir, detail: detail(e) }.say())?;

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
                // A file that settled on this same poll is finished, not stalled.
                if !settled.contains(&path) {
                    on_arrival(Arrival::Abandoned(path.clone()));
                }
                arriving.remove(&path);
            }
            for path in settled {
                arriving.remove(&path);
                log::info!("{} has settled", path.display());
                on_arrival(Arrival::Settled(path));
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
