//! The gate: proves a built binary carries its own libvips.
//!
//! **The last check is the one that matters.** Everything above it can pass on a
//! machine that happens to have Homebrew, because dyld will quietly satisfy a
//! missing library from `/opt/homebrew/lib` and say nothing. So the run happens
//! with `DYLD_PRINT_LIBRARIES=1` in a stripped environment, and every path dyld
//! reports is checked: one line from Homebrew fails the build.
//!
//! It drives `snappr-cli`, which is the same code the app runs and links libvips
//! the same way - so this reports on the libvips that would actually ship. The
//! Ruby version had to spawn a Ruby probe script for this, which could only ever
//! test the interpreter's view of things.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{relative, say};

/// A HEIC the *system* libvips writes, so that decoding it proves the bundled one
/// can. Built by the test suite rather than committed, and 8-bit flat grey so the
/// mean of a patch is a known number.
const FIXTURE_MEAN: f64 = 120.0;

pub fn run(root: &Path) -> Result<(), String> {
    let binary = binary(root)?;
    let fixture = fixture(root)?;

    println!();
    say("verify", format!("running {}", relative(&binary, root)));

    // `env_clear`, so nothing in the developer's shell can be what makes it work:
    // no DYLD_*, no PKG_CONFIG_PATH, no PATH beyond the system's.
    let run = Command::new(&binary)
        .arg("inspect")
        .arg(&fixture)
        .arg("--mean-of")
        .arg("0,0,64,64")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .env("DYLD_PRINT_LIBRARIES", "1")
        .output()
        .map_err(|error| format!("{}: {error}", binary.display()))?;

    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);

    if !run.status.success() {
        eprintln!("{stderr}");
        return Err(format!("the inspect run failed ({})", run.status));
    }

    let report: serde_json::Value = stdout
        .lines()
        .last()
        .and_then(|line| serde_json::from_str(line).ok())
        .ok_or_else(|| format!("could not parse the report from: {stdout}"))?;

    println!();
    say("verify", "=== verification ===");
    let mut failures = 0;
    let mut check = |what: &str, ok: bool, detail: String| {
        if ok {
            say("verify", format!("  ok    {what}"));
        } else {
            failures += 1;
            say("verify", format!("  FAIL  {what} - {detail}"));
        }
    };

    let number = |key: &str| report.get(key).and_then(serde_json::Value::as_f64).unwrap_or(0.0);
    let text = |key: &str| {
        report.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_string()
    };

    check(
        "HEIC header read",
        number("width") == 2400.0 && number("height") == 1600.0,
        format!("{}x{}", number("width"), number("height")),
    );

    // **The check sharp's prebuilt libvips fails.** A header read is lazy; the
    // mean of a known flat patch forces a real HEVC decode and proves the pixels
    // that come out are the ones that went in.
    let mean = number("mean");
    check(
        "HEVC pixels actually decode",
        (mean - FIXTURE_MEAN).abs() < 8.0,
        format!("mean {mean}, wanted about {FIXTURE_MEAN}"),
    );

    check("the HEIC loader was used", text("loader") == "heifload", text("loader"));
    check("bit depth preserved as 8", number("depth") == 8.0, text("depth"));
    check("libvips reports its version", text("vips").starts_with("8."), text("vips"));

    // The one that catches a bundle which only works on this machine.
    let borrowed: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains("/opt/homebrew"))
        .filter_map(|line| line.split_whitespace().last())
        .collect();
    check(
        "nothing loaded from Homebrew",
        borrowed.is_empty(),
        format!("{} libs, e.g. {}", borrowed.len(), borrowed.iter().take(2).cloned().collect::<Vec<_>>().join(", ")),
    );

    let loaded = stderr.lines().filter(|line| line.contains("dyld[")).count();
    println!();
    say(
        "verify",
        format!("{loaded} libraries loaded, {} of them from Homebrew", borrowed.len()),
    );

    if failures == 0 {
        say("verify", "the bundled libvips is self-contained");
        Ok(())
    } else {
        Err(format!("{failures} check(s) failed"))
    }
}

/// The release CLI.
///
/// **What this proves, precisely**: that a binary linked the way the app is
/// linked - `@rpath` install names, resolved through `build/vips` - runs with
/// nothing from Homebrew. It does not open the assembled `.app`; `mise run app`
/// is what puts the same closure into `Contents/Frameworks`, and the app binary
/// carries the same two rpaths, so the linkage under test is the shipped one.
///
/// A debug build would pass this vacuously, because a debug build links the
/// prefix by absolute path and never consults an rpath at all - hence release
/// only.
fn binary(root: &Path) -> Result<PathBuf, String> {
    let release = root.join("target/release/snappr-cli");
    if release.is_file() {
        return Ok(release);
    }
    Err("no binary to verify - run `cargo build --release -p snappr-cli` first".into())
}

/// The HEIC fixture, written by the test suite with the *system* libvips.
///
/// It has to come from somewhere other than the thing under test: the point is
/// that the bundled libvips can decode a file it did not create.
fn fixture(root: &Path) -> Result<PathBuf, String> {
    let fixture = root.join("build/cache/verify.heic");
    if fixture.is_file() {
        return Ok(fixture);
    }
    Err(format!(
        "no HEIC fixture at {} - run `cargo test -p snappr-core --test fixtures` first",
        relative(&fixture, root)
    ))
}
