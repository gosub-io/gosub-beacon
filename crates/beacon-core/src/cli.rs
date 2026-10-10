//! Command-line surface.
//!
//! Parsed once in `main` and stashed in [`Cli::global`], because argv was previously read in
//! three unrelated places (GTK hand-off, startup tabs, the private-window check) and each
//! re-scan invented its own rules — most visibly `filter(|a| !a.starts_with('-'))`, which
//! silently swallowed every flag.

use clap::Parser;
use std::path::PathBuf;
use std::sync::OnceLock;

/// The Gosub Beacon browser.
#[derive(Parser, Debug, Default, Clone)]
#[command(name = "gosub-beacon", version, about, long_about = None)]
pub struct Cli {
    /// URLs to open as startup tabs. Without any, the previous session is restored.
    #[arg(value_name = "URL")]
    pub urls: Vec<String>,

    /// Open a private window: ephemeral cookies and storage, nothing written to the session.
    #[arg(long)]
    pub private: bool,

    /// Profile directory to use instead of the XDG default (cookies, history, settings,
    /// session). Useful for a scratch profile that leaves the real one alone.
    #[arg(long, value_name = "DIR")]
    pub user_data_dir: Option<PathBuf>,

    /// Run the engine's component processes (network stack, cookie vault, localStorage
    /// service, image decoders, per-site page renderers). Linux only; elsewhere the engine
    /// says what cannot apply and runs in-process. The default in an `isolation` build. For this run only: the stored
    /// `security.process_isolation` setting is left alone.
    #[arg(long, conflicts_with = "single_process")]
    pub isolated: bool,

    /// Run everything in this one process, as the default build does. The way out of an
    /// `isolation` build's default.
    #[arg(long)]
    pub single_process: bool,
}

static CLI: OnceLock<Cli> = OnceLock::new();

impl Cli {
    /// Parse argv and publish the result. Call once, early in `main`.
    pub fn init() -> &'static Cli {
        CLI.get_or_init(Cli::parse)
    }

    /// The parsed command line. Falls back to defaults if `init` was never called, so tests
    /// and any stray caller get sane values rather than a panic.
    pub fn global() -> &'static Cli {
        CLI.get_or_init(Cli::default)
    }

    /// Whether this run uses the engine's component processes: asked for with
    /// `--isolated`, or the default of an `isolation` build unless `--single-process`.
    pub fn isolated(&self) -> bool {
        if self.isolated {
            return true;
        }
        cfg!(feature = "isolated-by-default") && !self.single_process
    }
}
