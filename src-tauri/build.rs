//! Where this binary looks for libvips at runtime.
//!
//! Two `LC_RPATH` entries, tried in order by dyld, so one binary works in both
//! layouts:
//!
//! - `@executable_path/../Frameworks` is the bundle. The executable sits in
//!   `Snappr.app/Contents/MacOS`, the closure in `Contents/Frameworks`, and
//!   `bin/build-vips` gives `libvips.42.dylib` an `@rpath/` install name so this
//!   is what resolves it. Its siblings stay `@loader_path`-relative and resolve
//!   against each other.
//! - the repo's `build/vips` is development, where the binary is somewhere under
//!   `target/`. Absolute, because a relative path from a `target/debug` binary
//!   would break the moment a profile or a target triple changed the depth.
//!
//! **No `DYLD_*` variable is involved, and that is the point.** Hardened runtime
//! strips them, and `entitlements.plist` deliberately does not ask for
//! `com.apple.security.cs.allow-dyld-environment-variables`. The old Ruby child
//! got away with `LD_LIBRARY_PATH` only because its FFI layer read that variable
//! itself and joined the paths by hand; a linked binary has no such hook.

fn main() {
    // The bundle first: a shipped app must never prefer a stale build/vips that
    // happens to exist on a developer's machine.
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");

    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent")
        .join("build/vips");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", repo.display());

    tauri_build::build()
}
