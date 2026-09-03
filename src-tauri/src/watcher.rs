//! Watches the drop folder for photos that have finished arriving.
//!
//! Four behaviours here are carried over from the Ruby watcher rather than
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
//! **Only arrivals are acted on**: a name that was not in the folder before. A
//! write to a photo already sitting there, a chmod, a Finder tag and a deletion
//! are all somebody else's business - the drop folder is the operator's real
//! `~/Downloads`. `admit` is the rule and `known` is the state it needs.
//!
//! That it needs state at all is the part worth knowing, because the obvious
//! implementation does not work. **On macOS an event's kind cannot tell you
//! whether a file was just added.** FSEvents accumulates flags per path, so
//! `ITEM_CREATED` keeps being reported for every later event on that path -
//! measured against notify 8.2, a plain `xattr -w` on a file that existed *before
//! the watch started* arrives as `Create(File)`, `Modify(Metadata(Extended))` and
//! `Modify(Data(Content))` together, and so does an append, a truncating rewrite
//! and even a deletion. Filtering on `EventKind::Create` therefore looks right,
//! passes a hand test on a fresh file, and quietly admits everything. Since the
//! kinds cannot answer the only question being asked of them, this module ignores
//! them and decides from the directory's own state instead - which is also how the
//! `listen` gem gets the same answer on the Ruby side, by diffing snapshots.
//!
//! **The candidate filter runs before anything touches the disk.** It also has to
//! reject snappr's own output, including the in-progress `.tmp-<pid>.overlay.png`,
//! which is why the suffix test covers both.
//!
//! **Arrivals are debounced on size and mtime.** AirDrop writes non-atomically, so
//! a file that exists is not a file that is finished; without this the pipeline
//! reads half a photo.

use std::collections::{HashMap, HashSet};
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

/// What one `stat` of a watched path found.
///
/// The three cases used to be two: a single `Option<Fingerprint>` said `None` both
/// for a file that was not there and for one that was there and empty. That
/// conflation is what let a *deletion* be mistaken for a transfer that had only
/// just begun - `admit` is the other half of that fix.
#[derive(Clone, Copy)]
enum Presence {
    /// Not there at all: the vanishing side of a rename, or a create already undone.
    Gone,
    /// There but zero bytes. An AirDrop creates the file before it writes anything
    /// into it, so this is a transfer that has begun and not yet delivered.
    Empty,
    /// There, with bytes. A file is considered finished once this has been
    /// identical for `stability_checks` consecutive polls.
    Sized(Fingerprint),
}

fn presence(path: &Path) -> Presence {
    let Ok(meta) = std::fs::metadata(path) else {
        return Presence::Gone;
    };
    if meta.len() == 0 {
        return Presence::Empty;
    }
    Presence::Sized(Fingerprint {
        len: meta.len(),
        modified: meta.modified().ok(),
    })
}

/// What to do with a candidate path an event has just named.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Admission {
    /// A name that was not in the folder before. Watch it settle.
    Arrival,
    /// It was already there, so whatever just happened to it is not an arrival.
    Ignore,
    /// It is not there any more. Stop remembering the name, so that the same name
    /// coming back later counts as an arrival again - the operator re-sending a
    /// photo they deleted is a thing that happens at an event.
    Forget,
}

/// The whole of "only on added", kept pure so it can be tested without a
/// filesystem and read without the loop around it.
///
/// Note what it does *not* consult: the event's kind. See the module header - on
/// macOS the kind cannot answer this question.
///
/// `Presence::Empty` counts as an arrival when the name is new, because an AirDrop
/// creates the file before it writes anything into it; the poll loop is what
/// decides whether anything ever turns up in it.
fn admit(presence: &Presence, known: bool) -> Admission {
    match presence {
        Presence::Gone => Admission::Forget,
        _ if known => Admission::Ignore,
        _ => Admission::Arrival,
    }
}

/// The candidate names already in the drop folder when the watch begins.
///
/// One non-recursive listing, and it is what makes "added" mean added: without it
/// the first stray event on a photo that was already sitting there - Spotlight
/// touching it, a Finder tag, an unrelated `cp` - would look exactly like an
/// arrival and get composited.
///
/// Measured on the real drop folder, 1965 top-level entries: 55ms cold, 2ms warm,
/// once per watch. This is emphatically *not* the recursive walk the module header
/// forbids - that one was 19k files on every event.
///
/// The files it finds are remembered, never processed. A photo already in the
/// folder when the watch starts is still not printed until it is dropped on the
/// window or run through `mise run overlay`, which is how it has always been.
///
/// **The listing must be canonicalised.** notify's FSEvents backend reports fully
/// resolved paths, so an event in `/var/folders/...` arrives as
/// `/private/var/folders/...` - `/var` being a symlink to `/private/var` on macOS.
/// Listed uncanonicalised, every name in here is a near-miss for the events it is
/// meant to match, `known` never contains anything, and the watcher goes quietly
/// back to treating every touch of every photo as an arrival. Nothing errors and
/// the common cases still work, so only a file that predates the watch shows it -
/// which is what `tagging_a_photo_that_predates_the_watch_says_nothing` is for.
fn already_there(dir: &Path, settings: &Settings) -> HashSet<PathBuf> {
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let Ok(entries) = std::fs::read_dir(&dir) else {
        // Unreadable, so nothing is known to be there. `watch` below reports the
        // real problem to the operator; guessing at a message here would only
        // duplicate it.
        return HashSet::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| is_candidate(path, settings))
        .collect()
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
///
/// It is not the last word on a path, though. A file reported `Abandoned` stays
/// under observation, so a transfer that finishes long after the timeout still
/// reaches the operator as a `Settled` for the same path - see `Arriving::stalled`.
pub enum Arrival {
    /// Size and mtime stopped changing: it is a whole photo.
    Settled(PathBuf),
    /// It never stopped changing before the timeout.
    Abandoned(PathBuf),
}

/// A file seen but not yet settled.
struct Arriving {
    last: Fingerprint,
    /// How many consecutive polls it has looked the same.
    stable_for: u32,
    first_seen: Instant,
    /// Already reported as [`Arrival::Abandoned`].
    ///
    /// Still watched, and that is the point of the flag rather than dropping the
    /// entry: a very slow transfer that finishes after the timeout must still
    /// reach the operator, and `lib.rs` promises exactly that where it publishes
    /// the failed cell. Events cannot deliver it - the later writes are not
    /// arrivals - so polling is the only thing left that can notice, and the
    /// settle test below is already the right one.
    ///
    /// It does not hold the poll thread open, though. See the exit condition.
    stalled: bool,
}

/// Watches `dir`, calling `on_arrival` once per file it reaches a conclusion
/// about. The returned watcher must be kept alive; dropping it stops the watch.
pub fn spawn<F>(dir: PathBuf, settings: Settings, on_arrival: F) -> Result<notify::RecommendedWatcher, String>
where
    F: Fn(Arrival) + Send + 'static,
{
    // Before the watch, not after: a photo landing in the gap would otherwise be
    // listed as already there and then ignored for the rest of the session.
    let mut known = already_there(&dir, &settings);

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
        // Files seen but not yet settled.
        let mut arriving: HashMap<PathBuf, Arriving> = HashMap::new();

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
                    // `is_candidate` first, because it is the common answer here
                    // and the only test that costs nothing. Everything below it
                    // stats, and does so only for a real photo in the folder.
                    if !is_candidate(&path, &settings) || arriving.contains_key(&path) {
                        continue;
                    }
                    let found = presence(&path);
                    match admit(&found, known.contains(&path)) {
                        Admission::Ignore => continue,
                        Admission::Forget => {
                            known.remove(&path);
                            continue;
                        }
                        Admission::Arrival => {}
                    }
                    // Empty is the only other thing that reaches here, `Gone` being
                    // `Admission::Forget` - and it is not worth a panic to say so
                    // in a thread whose death would leave the bar claiming to
                    // watch a folder nothing is watching.
                    let (last, stable_for) = match found {
                        Presence::Sized(print) => (print, 1),
                        _ => (Fingerprint { len: 0, modified: None }, 0),
                    };
                    known.insert(path.clone());
                    arriving.insert(
                        path,
                        Arriving { last, stable_for, first_seen: Instant::now(), stalled: false },
                    );
                }
            }

            let mut settled = Vec::new();
            let mut abandoned = Vec::new();
            let mut forgotten = Vec::new();

            for (path, entry) in arriving.iter_mut() {
                let mut gone = false;
                match presence(path) {
                    Presence::Sized(current) if current == entry.last => {
                        entry.stable_for += 1;
                        if entry.stable_for >= settings.stability_checks {
                            settled.push(path.clone());
                        }
                    }
                    Presence::Sized(current) => {
                        entry.last = current;
                        entry.stable_for = 1;
                    }
                    // Still zero-length. Not an error yet: AirDrop creates the file
                    // before it has anything to put in it.
                    Presence::Empty => entry.stable_for = 0,
                    Presence::Gone => {
                        entry.stable_for = 0;
                        gone = true;
                    }
                }

                let expired = entry.first_seen.elapsed() > settings.stability_timeout();
                if gone && (entry.stalled || expired) {
                    // It was there when the event named it and it is not there now.
                    // Nothing is left to wait for, and there is nothing to say
                    // either: no record was ever published for it, and "your
                    // transfer stalled" about a file the operator deleted is
                    // precisely the message this watcher used to get wrong.
                    forgotten.push(path.clone());
                } else if expired && !entry.stalled {
                    abandoned.push(path.clone());
                }
            }

            for path in &abandoned {
                // A file that settled on this same poll is finished, not stalled.
                if settled.contains(path) {
                    continue;
                }
                // Reported once, and then kept under observation rather than
                // dropped: with only arrival events to go on, this poll is the one
                // thing that can still notice a very slow transfer finishing, and
                // `lib.rs` promises the operator exactly that where it publishes
                // the failed cell.
                if let Some(entry) = arriving.get_mut(path) {
                    entry.stalled = true;
                }
                on_arrival(Arrival::Abandoned(path.clone()));
            }
            for path in forgotten {
                log::debug!("{} is gone, no longer watching for it", path.display());
                arriving.remove(&path);
                // Forgotten on this side too, so the same name arriving again is
                // an arrival. Deleting a settled photo takes the `Admission::Forget`
                // path instead; this is the one that never settled.
                known.remove(&path);
            }
            for path in settled {
                arriving.remove(&path);
                log::info!("{} has settled", path.display());
                on_arrival(Arrival::Settled(path));
            }

            // A stalled entry is kept for as long as the watch lives, so it must
            // not be the thing holding this thread open: once the watcher has been
            // dropped, genuine arrivals are drained and stalled ones given up on.
            if disconnected && arriving.values().all(|entry| entry.stalled) {
                log::info!("stopped watching {}", dir.display());
                break;
            }

            thread::sleep(POLL);
        }
    });

    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sized() -> Presence {
        Presence::Sized(Fingerprint { len: 12, modified: None })
    }

    #[test]
    fn a_name_that_was_not_there_is_an_arrival() {
        assert_eq!(admit(&sized(), false), Admission::Arrival);
    }

    #[test]
    fn an_empty_new_name_is_an_arrival() {
        // An AirDrop creates the file before it writes anything into it, so this
        // is a transfer that has begun. Whether anything turns up in it is the
        // poll loop's business, not this rule's.
        assert_eq!(admit(&Presence::Empty, false), Admission::Arrival);
    }

    #[test]
    fn a_name_already_in_the_folder_is_ignored() {
        // The whole point. Every event on a photo already sitting in the drop
        // folder lands here: a write, an append, a chmod, a Finder tag, a
        // Spotlight touch. On macOS all of those *also* carry FSEvents'
        // ITEM_CREATED flag, which is why this is decided on the folder's state
        // and not on the event's kind - see the module header.
        assert_eq!(admit(&sized(), true), Admission::Ignore);
        assert_eq!(admit(&Presence::Empty, true), Admission::Ignore);
    }

    #[test]
    fn a_name_that_is_gone_is_forgotten_either_way() {
        // Forgotten rather than ignored, so that the operator re-sending a photo
        // they deleted counts as an arrival again.
        assert_eq!(admit(&Presence::Gone, true), Admission::Forget);
        assert_eq!(admit(&Presence::Gone, false), Admission::Forget);
    }

    /// Drives the real watcher against a real folder through real FSEvents.
    ///
    /// `#[ignore]` because these take about 30 seconds between them and they are at
    /// the mercy of fsevent latency, so they have no business in the normal run -
    /// `cargo test -- --ignored` is how to run them. They are kept because the
    /// pure rule above cannot catch what actually went wrong here: the first
    /// version of this filter tested `EventKind::Create`, passed every hand test
    /// on a fresh file, and admitted a mere `xattr -w` on a file that predated the
    /// watch. Only driving the thing found that.
    mod live {
        use super::super::*;
        use std::sync::mpsc::channel as results;

        fn folder(name: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!("snappr-live-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn settings(watch: &Path) -> Settings {
            Settings { watch_dir: watch.to_path_buf(), stability_timeout_secs: 2, ..Settings::default() }
        }

        /// Runs `set_up` before the watch starts and `act` after it, and returns
        /// what the watcher concluded, sorted.
        fn drive(name: &str, set_up: impl FnOnce(&Path), act: impl FnOnce(&Path)) -> Vec<String> {
            let dir = folder(name);
            set_up(&dir);
            let (tx, rx) = results::<String>();
            let watcher = spawn(dir.clone(), settings(&dir), move |arrival| {
                let said = match arrival {
                    Arrival::Settled(p) => format!("settled {}", p.file_name().unwrap().to_str().unwrap()),
                    Arrival::Abandoned(p) => format!("stalled {}", p.file_name().unwrap().to_str().unwrap()),
                };
                let _ = tx.send(said);
            })
            .unwrap();
            thread::sleep(Duration::from_millis(400));
            act(&dir);
            thread::sleep(Duration::from_secs(5));
            drop(watcher);
            let mut concluded: Vec<String> = rx.try_iter().collect();
            concluded.sort();
            concluded
        }

        fn nothing(_: &Path) {}

        #[test]
        #[ignore = "drives real FSEvents; run with --ignored"]
        fn a_photo_created_in_place_settles() {
            let got = drive("create", nothing, |dir| {
                std::fs::write(dir.join("a.jpg"), b"abcdefghij").unwrap();
            });
            assert_eq!(got, ["settled a.jpg"]);
        }

        #[test]
        #[ignore = "drives real FSEvents; run with --ignored"]
        fn a_photo_renamed_into_the_folder_settles() {
            // FSEvents reports this as ITEM_RENAMED and never as a create, so a
            // create-only filter would drop it. Finder's move, `mv`, and a browser
            // download completing off its `.download` name are all this shape.
            let got = drive("rename", nothing, |dir| {
                let staged = dir.parent().unwrap().join("staged-rename.jpg");
                std::fs::write(&staged, b"abcdefghij").unwrap();
                std::fs::rename(&staged, dir.join("b.jpg")).unwrap();
            });
            assert_eq!(got, ["settled b.jpg"]);
        }

        #[test]
        #[ignore = "drives real FSEvents; run with --ignored"]
        fn tagging_a_photo_that_predates_the_watch_says_nothing() {
            // The case that broke the first attempt at this. `xattr -w` on a file
            // the watch never saw created still arrives carrying ITEM_CREATED.
            let got = drive(
                "xattr",
                |dir| std::fs::write(dir.join("old.jpg"), b"abcdefghij").unwrap(),
                |dir| {
                    std::process::Command::new("xattr")
                        .args(["-w", "com.apple.metadata:kMDItemComment", "hi"])
                        .arg(dir.join("old.jpg"))
                        .status()
                        .unwrap();
                },
            );
            assert!(got.is_empty(), "expected silence, got {got:?}");
        }

        #[test]
        #[ignore = "drives real FSEvents; run with --ignored"]
        fn rewriting_a_settled_photo_says_nothing_more() {
            let got = drive("modify", nothing, |dir| {
                let path = dir.join("d.jpg");
                std::fs::write(&path, b"abcdefghij").unwrap();
                thread::sleep(Duration::from_secs(2)); // let it settle first
                std::fs::write(&path, b"zyxwvutsrqponm").unwrap();
            });
            assert_eq!(got, ["settled d.jpg"], "the rewrite must not settle again");
        }

        #[test]
        #[ignore = "drives real FSEvents; run with --ignored"]
        fn deleting_a_photo_before_it_settles_says_nothing() {
            // This used to tell the operator their transfer had stalled, 30
            // seconds after they threw the file away.
            let got = drive("delete", nothing, |dir| {
                let path = dir.join("c.jpg");
                std::fs::write(&path, b"abcdefghij").unwrap();
                thread::sleep(Duration::from_millis(50));
                std::fs::remove_file(&path).unwrap();
            });
            assert!(
                got.iter().all(|said| !said.starts_with("stalled")),
                "expected no stall report, got {got:?}"
            );
        }

        #[test]
        #[ignore = "drives real FSEvents; run with --ignored"]
        fn a_stalled_transfer_that_finishes_late_still_settles() {
            // `Arriving::stalled`: the message goes out at the timeout, but the
            // path stays under observation, and the writes that finish the
            // transfer are not arrivals so only the poll can notice.
            let got = drive("stalled", nothing, |dir| {
                let path = dir.join("e.jpg");
                std::fs::write(&path, b"").unwrap(); // a transfer that delivers nothing
                thread::sleep(Duration::from_secs(3)); // past the 2s timeout
                std::fs::write(&path, b"abcdefghij").unwrap(); // and now it arrives
            });
            assert_eq!(got, ["settled e.jpg", "stalled e.jpg"]);
        }

        #[test]
        #[ignore = "drives real FSEvents; run with --ignored"]
        fn a_photo_already_there_when_the_watch_starts_is_left_alone() {
            // Not a scan. `already_there` remembers what it finds and prints none
            // of it.
            let got = drive(
                "preexisting",
                |dir| std::fs::write(dir.join("f.jpg"), b"abcdefghij").unwrap(),
                nothing,
            );
            assert!(got.is_empty(), "expected silence, got {got:?}");
        }
    }
}
