//! The snappr command line.
//!
//! Two commands that matter: `watch` for a kiosk with no window, and
//! `overlay PATH...` which is the loop for iterating on an overlay - it takes
//! paths straight off the command line, so nothing has to be AirDropped to see a
//! change. Pass one landscape and one portrait photo to check both variants at
//! once.
//!
//! `inspect` is the third, and it exists for `bin/verify-bundle`: it reports what
//! the libvips *this binary is linked against* makes of a file, which is how the
//! bundle gets checked with the copy it actually ships rather than with whatever
//! is installed on the machine.
//!
//! **Every message an operator sees comes from the catalogue in `snappr-core`.**
//! A catalogued problem prints its sentence alone, with no backtrace: a backtrace
//! over "that folder does not exist" reads as a crash rather than as something to
//! go and fix. Anything that is *not* catalogued keeps its trace, because that
//! one is a bug in snappr and the trace is the useful part.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use snappr_core::config::{self, PipelineConfig};
use snappr_core::overlay::{Outcome, Overlay};
use snappr_core::problem::Problem;
use snappr_core::watcher::{self, Arrival};
use snappr_core::vips;

#[derive(Parser)]
#[command(name = "snappr", about = "AirDrop-to-print kiosk pipeline", version)]
struct Cli {
    /// The config file to read instead of ~/.config/snappr/config.toml.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Print the pipeline's progress. Errors and warnings show either way.
    #[arg(long, short, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Watch the drop folder and overlay each photo that lands in it.
    Watch {
        /// The folder to watch, instead of the configured one.
        #[arg(value_name = "DIR")]
        dir: Option<PathBuf>,
    },
    /// Overlay the given photos now, then open them.
    Overlay {
        #[arg(value_name = "PATH", required = false)]
        paths: Vec<PathBuf>,

        /// Report each outcome as one JSON object per line.
        #[arg(long)]
        json: bool,

        /// Do not hand the finished print to the system opener.
        #[arg(long)]
        no_open: bool,
    },
    /// Report what libvips makes of a file, as JSON.
    Inspect {
        #[arg(value_name = "PATH")]
        path: PathBuf,

        /// Also report the mean of this region, as `left,top,width,height`.
        /// `bin/verify-bundle` uses it to prove the HEVC pixels really decoded
        /// rather than only the header having been read.
        #[arg(long, value_name = "L,T,W,H")]
        mean_of: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    logger::install(cli.verbose);

    match run(&cli) {
        Ok(code) => code,
        Err(problem) => {
            // The sentence alone. The console half has already been logged.
            eprintln!("{}", problem.ui());
            log::debug!("{}", problem.log_line());
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<ExitCode, Problem> {
    let path = config::config_path(cli.config.clone());
    let config = config::load(&path)?;
    log::debug!("config: {}", path.display());

    vips::init().map_err(|detail| Problem::ImageToolsUnavailable { detail })?;

    match &cli.command {
        Command::Watch { dir } => watch(config, dir.clone()),
        Command::Overlay { paths, json, no_open } => {
            overlay(config, paths, *json, *no_open)
        }
        Command::Inspect { path, mean_of } => inspect(path, mean_of.as_deref()),
    }
}

fn watch(config: PipelineConfig, dir: Option<PathBuf>) -> Result<ExitCode, Problem> {
    let dir = dir.unwrap_or_else(|| config.watch_dir.clone());
    if !dir.is_dir() {
        return Err(Problem::NotAFolder { path: dir });
    }

    let config = PipelineConfig { watch_dir: dir.clone(), ..config };
    let overlay = Overlay::new(config.clone());
    let open_after = config.open_after;

    log::info!("watching {} for {} files", dir.display(), config.extensions.join(", "));

    // The watcher must be kept alive; dropping it stops the watch.
    let _watcher = watcher::spawn(dir, config, move |arrival| match arrival {
        Arrival::Settled(path) => match overlay.process(&path) {
            Ok(outcome) => {
                for warning in &outcome.warnings {
                    log::warn!("{}", warning.message);
                }
                if open_after {
                    open(&outcome.destination);
                }
            }
            // One bad photo must not take the daemon down. There is no retry and
            // no reject folder: the original is still sitting in the drop folder,
            // so `snappr overlay` on it is the retry.
            Err(problem) => log::error!("{}", problem.log_line()),
        },
        // Not an error - a stalled transfer is not a broken photo - so one calm
        // line rather than a failure.
        Arrival::Abandoned(path) => log::warn!(
            "{}",
            Problem::TransferStalled {
                file: snappr_core::problem::name_of(&path),
                path,
                secs: 30,
            }
            .log_line()
        ),
    })?;

    // Nothing to do but stay alive. SIGINT ends the process and the watch with
    // it; there is no state to flush, since every photo is either finished on
    // disk or was never started.
    loop {
        std::thread::park();
    }
}

fn overlay(
    config: PipelineConfig,
    paths: &[PathBuf],
    json: bool,
    no_open: bool,
) -> Result<ExitCode, Problem> {
    if paths.is_empty() {
        return Err(Problem::NoPathsGiven);
    }

    let open_after = config.open_after && !no_open;
    let overlay = Overlay::new(config);
    let mut failed = 0;

    log::info!("overlaying {} photo{}", paths.len(), if paths.len() == 1 { "" } else { "s" });

    for path in paths {
        match overlay.process(path) {
            Ok(outcome) => {
                if json {
                    report(&outcome);
                } else {
                    println!("{}", outcome.destination.display());
                }
                for warning in &outcome.warnings {
                    log::warn!("{}", warning.message);
                }
                if open_after {
                    open(&outcome.destination);
                }
            }
            Err(problem) => {
                failed += 1;
                // Logged and counted rather than returned, so one bad photo in a
                // batch does not abandon the rest.
                log::error!("{}", problem.log_line());
                if json {
                    let payload = problem.payload();
                    println!(
                        "{}",
                        serde_json::json!({
                            "ok": false,
                            "source": path,
                            "error": { "code": payload.code, "message": payload.message },
                        })
                    );
                }
            }
        }
    }

    Ok(if failed == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

fn report(outcome: &Outcome) {
    let mut value = serde_json::to_value(outcome).expect("an Outcome serialises");
    if let Some(object) = value.as_object_mut() {
        object.insert("ok".into(), true.into());
    }
    println!("{value}");
}

fn inspect(path: &PathBuf, mean_of: Option<&str>) -> Result<ExitCode, Problem> {
    let report = snappr_core::inspect::describe(path, mean_of)?;
    println!("{}", serde_json::to_string(&report).expect("a Report serialises"));
    Ok(ExitCode::SUCCESS)
}

/// Hands the print to the system opener, which is the CLI's whole output story.
fn open(destination: &std::path::Path) {
    log::info!("{}: opening", snappr_core::problem::name_of(destination));
    let opened = std::process::Command::new("open").arg(destination).status();

    match opened {
        Ok(status) if status.success() => {}
        _ => log::warn!(
            "{}",
            Problem::OpenFailed {
                file: snappr_core::problem::name_of(destination),
                path: destination.to_path_buf(),
            }
            .log_line()
        ),
    }
}

/// **Logs go to stderr, always.**
///
/// Under the old sidecar this was load-bearing: stdout carried protocol frames
/// and one stray line desynchronised the caller's parser. There is no protocol
/// any more, but the rule is still right for a different reason - `overlay --json`
/// writes machine-readable output to stdout, and `bin/verify-bundle` parses it.
mod logger {
    use std::io::Write;

    struct Stderr {
        level: log::LevelFilter,
    }

    impl log::Log for Stderr {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.level() <= self.level
        }

        fn log(&self, record: &log::Record) {
            if !self.enabled(record.metadata()) {
                return;
            }
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(stderr, "{:<5} {}", record.level(), record.args());
        }

        fn flush(&self) {
            let _ = std::io::stderr().flush();
        }
    }

    pub fn install(verbose: bool) {
        let level = if verbose {
            log::LevelFilter::Debug
        } else {
            std::env::var("SNAPPR_LOG_LEVEL")
                .ok()
                .and_then(|value| value.parse().ok())
                // Warnings and errors by default: a degraded overlay is something
                // the operator needs to see, progress is not.
                .unwrap_or(log::LevelFilter::Info)
        };

        let logger = Box::new(Stderr { level });
        let _ = log::set_boxed_logger(logger);
        log::set_max_level(level);
    }
}
