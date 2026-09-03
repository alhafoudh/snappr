//! Tells the linker where libvips is.
//!
//! `rs-vips` asks for `-lvips` and stops there - it never emits a search path,
//! so without this every binary in the workspace fails to link with
//! `ld: library 'vips' not found`. The lib target itself builds fine, which means
//! the error first shows up on whatever happens to be the first test or binary
//! built, pointing nowhere near the cause.
//!
//! **Which libvips is found is a decision, not a detail**, and it is a different
//! one per profile:
//!
//! - **debug** links `build/libvips-prefix`, whose install name is the absolute
//!   path meson gave it. Every binary and every *test* binary then finds libvips
//!   with no `LC_RPATH` involved, which matters because `cargo:rustc-link-arg`
//!   and its `-tests` variant do not reach a library crate's own unit-test
//!   harness - so a dev build that needed an rpath would fail `cargo test` with
//!   "Library not loaded: @rpath/libvips.42.dylib" while `cargo run` worked.
//! - **release** links `build/link`, whose symlinks point at the relocated
//!   closure in `build/vips`, where libvips' install name is `@rpath/...`. That
//!   is what lets the shipped binary find libvips inside
//!   `Snappr.app/Contents/Frameworks` - see `src-tauri/build.rs`.
//!
//! Homebrew is the fallback for a fresh clone that has not run
//! `cargo xtask runtime` yet. It must never be what a *bundle* links: Homebrew's
//! libvips loads its HEIC support from a compile-time module directory, so a
//! copied Homebrew libvips silently cannot decode an iPhone photo. `cargo xtask
//! verify` is the gate that catches it.
//!
//! Link search paths propagate to every crate that depends on this one, so the
//! app and the CLI both get this for free.

fn main() {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    println!("cargo:rerun-if-env-changed=PROFILE");

    if let Some(dir) = release_link_directory() {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }
    prefer_the_built_prefix();

    // **`cargo_metadata(false)`, and only the search path is emitted.**
    //
    // Letting pkg-config emit its own directives links every library in libvips'
    // `Libs:` line - glib, gobject, gio, intl - directly into the binary, with
    // Homebrew's absolute install names, *while* libvips itself resolves through
    // `@rpath` to the bundled closure. The process then holds two copies of glib
    // and segfaults during `vips_init`. Nothing here calls a glib symbol
    // directly: they arrive transitively through libvips, which is the one
    // library that should be named.
    let found = pkg_config::Config::new()
        .atleast_version("8.15")
        .cargo_metadata(false)
        .probe("vips");

    match found {
        Ok(library) => {
            for path in &library.link_paths {
                println!("cargo:rustc-link-search=native={}", path.display());
            }
        }
        Err(error) => {
            // Not a hard failure: a machine may have libvips somewhere pkg-config
            // does not know about, and a plain `-L` in RUSTFLAGS should still work.
            println!("cargo:warning=libvips not found via pkg-config ({error}). Install it, or put its .pc file on PKG_CONFIG_PATH.");
        }
    }
}

/// `build/link` for a release build, and nothing for a debug one.
///
/// Searched *before* the prefix, so a release binary records
/// `@rpath/libvips.42.dylib` and finds the bundled closure. `cargo xtask closure`
/// writes the directory; if it has not run, this falls through to the prefix and
/// the resulting binary is a development one - which `cargo xtask verify` will
/// refuse to accept as a bundle.
fn release_link_directory() -> Option<std::path::PathBuf> {
    let release = std::env::var("PROFILE").is_ok_and(|profile| profile == "release");
    repo_root()
        .map(|root| root.join("build/link"))
        .filter(|dir| release && dir.is_dir())
}

/// Puts `build/libvips-prefix` at the front of `PKG_CONFIG_PATH` when it exists.
///
/// Done here rather than left to the environment so that "the build links against
/// the libvips we built" is a property of the repo and not of how the shell
/// happened to be set up. Homebrew's copy remains the fallback for a fresh clone,
/// and `bin/verify-bundle` is what refuses a *bundle* built against it.
fn prefer_the_built_prefix() {
    let prefix = repo_root().map(|root| root.join("build/libvips-prefix/lib/pkgconfig"));

    let Some(prefix) = prefix.filter(|path| path.is_dir()) else { return };

    let existing = std::env::var("PKG_CONFIG_PATH").unwrap_or_default();
    let combined = if existing.is_empty() {
        prefix.display().to_string()
    } else {
        format!("{}:{existing}", prefix.display())
    };
    // SAFETY: build scripts are single-threaded, and this runs before the probe.
    std::env::set_var("PKG_CONFIG_PATH", combined);
}

/// The workspace root, from this crate's own location.
fn repo_root() -> Option<std::path::PathBuf> {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(std::path::Path::to_path_buf)
}
