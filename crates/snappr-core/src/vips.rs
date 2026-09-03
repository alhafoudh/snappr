//! Process-wide libvips setup. Call [`init`] once, before any image operation.
//!
//! Three things happen here and each one is load-bearing.
//!
//! **The SVG loader is unblocked by name, and the hardening around it is opted
//! into.** Guest photos are semi-trusted input - they arrive by AirDrop from
//! whoever is standing at the kiosk - so libvips' untrusted-loader hardening is
//! switched on deliberately, and then exactly one loader is exempted:
//! `VipsForeignLoadSvg`, because the overlay is our own committed artwork. Under
//! Ruby this was inherited by accident (`image_processing/vips` calls
//! `block_untrusted(true)` at require time) and the codebase had to *undo* half of
//! it; the same end state is now asked for outright. Getting the order wrong, or
//! blocking everything and forgetting the exemption, fails every overlay render
//! with `VipsForeignLoad: buffer is not in a known format`, which points nowhere
//! near the cause.
//!
//! **`LIBHEIF_PLUGIN_PATH` is pointed at a directory that does not exist.** That
//! is not a mistake: libheif otherwise dlopens codec plugins - ffmpeg, x264, vpx -
//! from wherever it was configured to look, and a kiosk should decode the HEVC
//! that libheif already has linked in rather than whatever happens to be on the
//! machine. Setting it before `Vips::init` is what makes it take effect.
//!
//! **libvips finds its own dylibs through the binary's `LC_RPATH`.** There is
//! deliberately no `LD_LIBRARY_PATH` or `DYLD_*` handling here. Ruby needed
//! `LD_LIBRARY_PATH` because its FFI layer read that variable itself and joined
//! the paths; a linked binary does not, and hardened runtime strips `DYLD_*`
//! anyway.

use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::OnceLock;

use rs_vips::error::Error;
use rs_vips::Vips;

use crate::problem::Problem;

/// The one loader that is exempt from the untrusted-loader block, by libvips
/// operation class name (not its nickname, `svgload`).
const SVG_LOADER: &str = "VipsForeignLoadSvg";

// Neither of these is in rs-vips' `Vips` struct, which offers only init,
// shutdown, leak_set, concurrency_set and thread_shutdown. They are plain libvips
// C entry points and we are already linking libvips, so declaring them is cheaper
// than either forking the binding or giving up the hardening.
extern "C" {
    fn vips_block_untrusted_set(state: i32);
    fn vips_operation_block_set(name: *const c_char, state: i32);
}

static READY: OnceLock<Result<(), String>> = OnceLock::new();

/// Initialises libvips, exactly once per process.
///
/// Safe to call from anywhere; every call after the first returns the first
/// call's answer. That matters because both front ends have more than one entry
/// point into the pipeline and none of them should have to know whether it is
/// first.
pub fn init() -> Result<(), String> {
    READY.get_or_init(setup).clone()
}

fn setup() -> Result<(), String> {
    // Before init, so libheif reads it while starting up.
    std::env::set_var("LIBHEIF_PLUGIN_PATH", "/nonexistent/snappr-no-heif-plugins");

    Vips::init("snappr").map_err(|e| e.to_string())?;

    // libvips' own thread pool provides the parallelism inside one operation, and
    // snappr composites one photo at a time on purpose, so this is about how many
    // threads that single job may use.
    Vips::concurrency_set(num_threads());

    unsafe {
        vips_block_untrusted_set(1);
        let name = CString::new(SVG_LOADER).expect("no interior nul in a literal");
        vips_operation_block_set(name.as_ptr(), 0);
    }

    Ok(())
}

fn num_threads() -> i32 {
    std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(4)
}

/// Empties libvips' error buffer. Call this before a job, not after.
///
/// **libvips accumulates error text and nothing clears it on read.** rs-vips
/// appends the whole buffer to every error it hands back, so without this a
/// failure's console half quotes unrelated earlier failures - observed directly
/// while writing `vips_capabilities.rs`, where an ICC assertion came back
/// reporting a *different* test's "buffer is not in a known format". Since
/// degraded paths are expected here (a missing marker, an unusable profile), the
/// buffer is never empty for long, and a message that names the wrong cause is
/// worse than no message.
pub fn clear_errors() {
    Vips::error_clear();
}

/// Turns a libvips failure into the catalogue's stand-in for it, and clears the
/// buffer so the next failure starts clean.
///
/// A libvips error never shows its own text to the operator: it borrows
/// [`Problem::PhotoUnusable`]'s sentence and keeps the original for the log. The
/// mapping is by type, never by matching on the message.
pub fn failed(error: Error) -> Problem {
    let detail = error.to_string();
    clear_errors();
    Problem::PhotoUnusable { detail }
}

/// The first line of a libvips error, for a console half that is appended to a
/// sentence rather than being the whole of it. Clears the buffer too.
pub fn first_line(error: Error) -> String {
    let text = error.to_string();
    clear_errors();
    text.lines().next().unwrap_or_default().trim().to_string()
}
