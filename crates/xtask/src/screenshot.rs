//! Every picture in the README, from real photos run through the real pipeline.
//!
//! ```text
//! cargo xtask screenshot    docs/images/demo-*  and  docs/images/app.png
//! ```
//!
//! The inputs are the stock photos in `docs/images/stock/` (Unsplash-licensed, see
//! the `LICENSE.md` beside them), the event designs in `docs/examples/` and the
//! sample overlays in `sample/`. Everything scratch goes to `build/screenshot/`.
//!
//! **The app screenshot runs Snappr under its own bundle identifier.** Tauri
//! derives `app_config_dir()` and `app_data_dir()` from the identifier, and the
//! app has no other override for them - so a dev run pointed at a test folder
//! would otherwise rewrite the operator's real `settings.json` and filmstrip.
//! Under `sk.freevision.snappr.screenshot` the settings, the index, the previews
//! and the overlay cards all live in a directory this task owns, wipes and seeds,
//! which is also what makes two runs start from the same state.
//!
//! **What fixes the picture**, so the next run shows the same thing:
//! - photos are fed one at a time, in filename order, each only after the last
//!   has finished. The app selects only the *first* arrival of a session and
//!   gives every later one a blue dot (`src/usePhotos.ts`), so `01` is the photo
//!   on the stage and the rest show as not looked at yet;
//! - the capture has to land within a minute of the first arrival, or the oldest
//!   cell stops reading "just now";
//! - the theme is hard-coded dark and the window size comes from
//!   `tauri.conf.json`, so neither depends on the machine.
//!
//! The pixels can still differ across macOS versions (font rendering), and the
//! capture needs Screen Recording permission for the terminal, so this is a
//! local task rather than a CI one.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::{output, relative, say, sh};

const IDENTIFIER: &str = "sk.freevision.snappr.screenshot";

/// Vite's port, fixed in `vite.config.ts` and `tauri.conf.json`. A dev session
/// already running would hold it, and `tauri dev` would then fail halfway.
const VITE_PORT: u16 = 5183;

/// Long side of the demo images in `docs/images/`. The README shows them at
/// 220-300px tall, so this is retina with room to spare.
const DEMO_PX: &str = "1200";

/// Generous: the first run compiles the whole app.
const WINDOW_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const PHOTO_TIMEOUT: Duration = Duration::from_secs(60);
/// Under a minute, with margin for the settle and the capture itself.
const JUST_NOW: Duration = Duration::from_secs(50);

pub fn run(root: &Path) -> Result<(), String> {
    for tool in ["vips", "oxipng", "screencapture", "npm"] {
        if output("which", &[tool]).is_none() {
            return Err(format!("{tool} is not on PATH - `brew install vips oxipng`, and `npm install`"));
        }
    }

    let photos = stock(root)?;
    demo_images(root, &photos)?;
    app_screenshot(root, &photos)
}

/// The stock photos, in the order they are fed to the app.
fn stock(root: &Path) -> Result<Vec<PathBuf>, String> {
    let dir = root.join("docs/images/stock");
    let mut photos: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|error| format!("{}: {error}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "jpg"))
        .collect();
    photos.sort();
    if photos.len() < 5 {
        return Err(format!("{} holds {} photos, the filmstrip wants at least 5", relative(&dir, root), photos.len()));
    }
    Ok(photos)
}

fn is_portrait(photo: &Path) -> Result<bool, String> {
    let field = |name: &str| -> Result<u64, String> {
        output("vipsheader", &["-f", name, &photo.to_string_lossy()])
            .and_then(|text| text.trim().parse().ok())
            .ok_or_else(|| format!("vipsheader could not read the {name} of {}", photo.display()))
    };
    Ok(field("height")? > field("width")?)
}

// ---------------------------------------------------------------------------
// The demo images
// ---------------------------------------------------------------------------

/// The six `docs/images/demo-*` files: a guest's photo, a design, and what the
/// pipeline makes of the photo under the event designs and under the sample.
///
/// Driven through `snappr-cli`, the same code the app runs, so the prints in the
/// README are real output rather than a drawing of it.
fn demo_images(root: &Path, photos: &[PathBuf]) -> Result<(), String> {
    say("screenshot", "building snappr-cli");
    sh(Command::new("cargo").args(["build", "-p", "snappr-cli"]).current_dir(root))?;
    let cli = root.join("target/debug/snappr-cli");

    // The first photo is the one on the app's stage, so the README leads with it too.
    let landscape = photos.first().expect("stock() returns at least 5").clone();
    if is_portrait(&landscape)? {
        return Err(format!("{} is fed first and fills the stage, so it must be landscape", landscape.display()));
    }
    let portrait = photos
        .iter()
        .find(|photo| is_portrait(photo).unwrap_or(false))
        .ok_or("docs/images/stock has no portrait photo")?
        .clone();

    let scratch = root.join("build/screenshot/demo");
    fresh_dir(&scratch)?;
    let images = root.join("docs/images");

    for (design, prefix, landscape_svg, portrait_svg) in [
        ("event", "demo-print", "docs/examples/event-landscape.svg", "docs/examples/event-portrait.svg"),
        ("sample", "demo-sample", "sample/overlay-landscape.svg", "sample/overlay-portrait.svg"),
    ] {
        // The CLI writes its print beside the photo, so the photos are copied into
        // scratch first - never a `.overlay.png` inside docs/.
        let dir = scratch.join(design);
        std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        let inputs = [copy_into(&landscape, &dir)?, copy_into(&portrait, &dir)?];

        let config = dir.join("config.toml");
        let toml = format!(
            "open_after = false\n\n[overlay]\nlandscape = {:?}\nportrait = {:?}\n",
            root.join(landscape_svg).display().to_string(),
            root.join(portrait_svg).display().to_string(),
        );
        std::fs::write(&config, toml).map_err(|error| format!("{}: {error}", config.display()))?;

        say("screenshot", format!("compositing under the {design} overlays"));
        let run = Command::new(&cli)
            .arg("overlay")
            .arg("--config")
            .arg(&config)
            .arg("--no-open")
            .arg("--json")
            .args(&inputs)
            .output()
            .map_err(|error| format!("{}: {error}", cli.display()))?;
        // One JSON object per photo on stdout; a failure says which and why.
        for line in String::from_utf8_lossy(&run.stdout).lines() {
            let report: Value = serde_json::from_str(line).map_err(|_| format!("snappr-cli printed {line:?}"))?;
            if report.get("ok") != Some(&Value::Bool(true)) {
                return Err(format!("snappr-cli failed: {report}"));
            }
        }
        if !run.status.success() {
            return Err(format!("snappr-cli failed ({}): {}", run.status, String::from_utf8_lossy(&run.stderr)));
        }

        for (input, orientation) in [(&inputs[0], "landscape"), (&inputs[1], "portrait")] {
            let print = input.with_extension("overlay.png");
            shrink(&print, &images.join(format!("{prefix}-{orientation}.jpg")))?;
        }
    }

    shrink(&landscape, &images.join("demo-photo.jpg"))?;

    // The design as drawn, magenta window and all.
    let design = scratch.join("demo-design.png");
    sh(Command::new("vips").arg("thumbnail").arg(root.join("docs/examples/event-landscape.svg")).arg(&design).args([DEMO_PX, "--height", DEMO_PX]))?;
    sh(Command::new("oxipng").args(["-q", "-o", "4", "--strip", "safe", "--out"]).arg(images.join("demo-design.png")).arg(&design))?;

    say("screenshot", "demo images written to docs/images/");
    Ok(())
}

/// A README-sized JPEG. **EXIF is dropped** - a print carries the guest's whole
/// EXIF block, GPS included, and none of that belongs on a public page. The ICC
/// profile is kept so the colours stay right.
fn shrink(from: &Path, to: &Path) -> Result<(), String> {
    let target = format!("{}[Q=85,keep=icc]", to.display());
    sh(Command::new("vips").arg("thumbnail").arg(from).arg(target).args([DEMO_PX, "--height", DEMO_PX]))
}

// ---------------------------------------------------------------------------
// The app screenshot
// ---------------------------------------------------------------------------

fn app_screenshot(root: &Path, photos: &[PathBuf]) -> Result<(), String> {
    let support = support_dir()?;
    // Named after the folder a kiosk really watches, because the strip's watch
    // chip shows the folder name.
    let watched = root.join("build/screenshot/Downloads");
    fresh_dir(&support)?;
    fresh_dir(&watched)?;

    // Only the two keys that differ from a fresh install. `#[serde(default)]` on
    // `Settings` fills the rest, and its defaults are the sample overlay for both
    // orientations - exactly what a first launch shows.
    let settings = serde_json::json!({ "watch_dir": watched, "watch_enabled": true });
    std::fs::write(support.join("settings.json"), settings.to_string())
        .map_err(|error| format!("{}: {error}", support.display()))?;

    if port_taken(VITE_PORT) {
        return Err(format!("port {VITE_PORT} is in use - stop the running `mise run dev` first"));
    }

    say("screenshot", format!("launching Snappr as {IDENTIFIER}"));
    let app = App::launch(root)?;
    let window = app.wait_for_window()?;

    wait_for("the overlay card", Duration::from_secs(60), || {
        Ok(std::fs::read_dir(support.join("overlays"))
            .map(|entries| entries.flatten().any(|entry| entry.path().extension().is_some_and(|ext| ext == "png")))
            .unwrap_or(false))
    })?;
    // The watch begins in the app's setup, before the window, so it is running by
    // now. A copy made before it would be remembered as already there and ignored.
    std::thread::sleep(Duration::from_secs(2));

    let index = support.join("index.json");
    let mut first = None;
    for (fed, photo) in photos.iter().enumerate() {
        let name = file_name(photo);
        say("screenshot", format!("feeding {name}"));
        copy_into(photo, &watched)?;
        first.get_or_insert_with(Instant::now);

        wait_for(&format!("{name} to be composited"), PHOTO_TIMEOUT, || {
            let done = finished(&index)?;
            Ok(done.len() == fed + 1 && done.iter().any(|done| done == &name))
        })?;
    }

    // The first arrival is the one the app selected, and selecting it is what
    // cleared its blue dot. Waiting on that as well means the window has caught up.
    let first_name = file_name(photos.first().expect("stock() returns at least 5"));
    wait_for("the first photo to be selected", Duration::from_secs(20), || {
        Ok(read_json(&index)?
            .and_then(|index| index.get("selected").and_then(Value::as_str).map(|path| path.ends_with(&first_name)))
            .unwrap_or(false))
    })?;

    // Long enough for the webview to decode five previews and the stage.
    std::thread::sleep(Duration::from_secs(2));
    let elapsed = first.expect("at least one photo was fed").elapsed();
    if elapsed > JUST_NOW {
        return Err(format!(
            "feeding took {}s, so the oldest cell no longer reads \"just now\" - run it again",
            elapsed.as_secs()
        ));
    }

    let raw = root.join("build/screenshot/app.png");
    // `-o` drops the window shadow, `-x` the shutter sound.
    sh(Command::new("screencapture").args(["-o", "-x", "-l", &window.to_string()]).arg(&raw))?;
    if std::fs::metadata(&raw).map(|meta| meta.len()).unwrap_or(0) == 0 {
        return Err("screencapture wrote nothing - give the terminal Screen Recording permission".into());
    }
    let out = root.join("docs/images/app.png");
    sh(Command::new("oxipng").args(["-q", "-o", "4", "--strip", "safe", "--out"]).arg(&out).arg(&raw))?;

    say("screenshot", "closing Snappr");
    drop(app);
    say("screenshot", format!("wrote {}", relative(&out, root)));
    Ok(())
}

/// Where Tauri puts both the config and the data dir for this identifier on
/// macOS. Asserted before anything is deleted in it.
fn support_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let dir = PathBuf::from(home).join("Library/Application Support").join(IDENTIFIER);
    if !dir.to_string_lossy().ends_with(".screenshot") {
        return Err(format!("refusing to wipe {}", dir.display()));
    }
    Ok(dir)
}

/// The names of the photos the app has finished, from its own `index.json`.
fn finished(index: &Path) -> Result<Vec<String>, String> {
    let Some(index) = read_json(index)? else { return Ok(Vec::new()) };
    Ok(index
        .get("photos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|photo| photo.get("status").and_then(Value::as_str) == Some("done"))
        // Done is written before the window has anything to draw unless the
        // previews are on disk too.
        .filter(|photo| {
            photo.get("previews").and_then(Value::as_object).is_some_and(|previews| {
                ["thumb", "overlay", "source"]
                    .iter()
                    .all(|key| previews.get(*key).and_then(Value::as_str).is_some_and(|path| Path::new(path).is_file()))
            })
        })
        .filter_map(|photo| photo.get("name").and_then(Value::as_str).map(String::from))
        .collect())
}

/// `None` while the file does not exist yet, or is caught halfway through a write.
fn read_json(path: &Path) -> Result<Option<Value>, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(serde_json::from_str(&raw).ok()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// The dev app, in a process group of its own so the whole tree - npm, Vite,
/// cargo and Snappr - goes down together, on success or on any error.
struct App {
    child: Child,
}

impl App {
    fn launch(root: &Path) -> Result<Self, String> {
        let config = serde_json::json!({ "identifier": IDENTIFIER }).to_string();
        let child = Command::new("npm")
            .args(["run", "tauri", "--", "dev", "--no-watch", "--config", &config])
            .current_dir(root)
            .process_group(0)
            .spawn()
            .map_err(|error| format!("npm run tauri dev: {error}"))?;
        Ok(Self { child })
    }

    /// The CGWindowID of Snappr's main window, once it is on screen.
    ///
    /// Matched on the owning process being in *this* process group, not on the
    /// name: an installed Snappr.app the operator has open is also called Snappr.
    fn wait_for_window(&self) -> Result<u64, String> {
        let group = self.child.id().to_string();
        let mut found = None;
        wait_for("the Snappr window", WINDOW_TIMEOUT, || {
            let ours: Vec<u64> = output("pgrep", &["-g", &group])
                .unwrap_or_default()
                .lines()
                .filter_map(|line| line.trim().parse().ok())
                .collect();
            found = windows()?
                .into_iter()
                .find(|window| window.name == "Snappr" && ours.contains(&window.pid))
                .map(|window| window.number);
            Ok(found.is_some())
        })?;
        Ok(found.expect("wait_for returned only once a window was found"))
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = Command::new("kill").args(["-TERM", &format!("-{}", self.child.id())]).status();
        let _ = self.child.wait();
    }
}

struct Window {
    pid: u64,
    number: u64,
    name: String,
}

/// The on-screen, normal-layer windows, via CoreGraphics from JXA - no Swift
/// toolchain and no crate needed for one call.
fn windows() -> Result<Vec<Window>, String> {
    const SCRIPT: &str = r#"
        ObjC.import("CoreGraphics");
        const all = ObjC.deepUnwrap(ObjC.castRefToObject(
            $.CGWindowListCopyWindowInfo($.kCGWindowListOptionOnScreenOnly, 0)));
        JSON.stringify(all.filter(w => w.kCGWindowLayer === 0)
            .map(w => [w.kCGWindowOwnerPID, w.kCGWindowNumber, w.kCGWindowOwnerName]));
    "#;
    let raw = output("osascript", &["-l", "JavaScript", "-e", SCRIPT]).ok_or("osascript could not list windows")?;
    let rows: Vec<(u64, u64, String)> =
        serde_json::from_str(raw.trim()).map_err(|error| format!("window list: {error}: {raw}"))?;
    Ok(rows.into_iter().map(|(pid, number, name)| Window { pid, number, name }).collect())
}

fn port_taken(port: u16) -> bool {
    ["127.0.0.1", "[::1]"].iter().any(|host| {
        format!("{host}:{port}")
            .parse()
            .is_ok_and(|address| std::net::TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok())
    })
}

fn wait_for(what: &str, timeout: Duration, mut ready: impl FnMut() -> Result<bool, String>) -> Result<(), String> {
    let start = Instant::now();
    while !ready()? {
        if start.elapsed() > timeout {
            return Err(format!("gave up waiting for {what} after {}s", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

fn fresh_dir(dir: &Path) -> Result<(), String> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    }
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))
}

fn copy_into(file: &Path, dir: &Path) -> Result<PathBuf, String> {
    let to = dir.join(file_name(file));
    std::fs::copy(file, &to).map_err(|error| format!("{} -> {}: {error}", file.display(), to.display()))?;
    Ok(to)
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}
