//! Where this binary looks for libvips at runtime.
//!
//! The same two `LC_RPATH` entries the app gets, and for the same reasons - see
//! `src-tauri/build.rs`, which is the copy with the argument written out. They
//! cannot be shared from `snappr-core`: `cargo:rustc-link-arg` applies only to
//! the crate being built, so each binary has to ask for its own.
//!
//! The CLI gets the bundle path too, because `bin/verify-bundle` runs a copy of it
//! from inside `Snappr.app` to prove the shipped closure is what answers.

fn main() {
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");

    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|crates| crates.parent())
        .expect("the crate sits two levels under the workspace root")
        .join("build/vips");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", repo.display());
}
