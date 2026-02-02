//! roz CLI - Quality gate for Claude Code.

use clap::{Parser, Subcommand};
use roz::cli;
use std::process::ExitCode;

/// Get the version string.
///
/// - Release builds (on a git tag): "0.1.2"
/// - Development builds: "0.1.2-dev (abc1234)"
/// - Dirty working directory: "0.1.2-dev (abc1234-dirty)"
fn version() -> &'static str {
    const VERSION: &str = env!("CARGO_PKG_VERSION");
    const GIT_HASH: &str = env!("ROZ_GIT_HASH");
    const IS_RELEASE: &str = env!("ROZ_IS_RELEASE");

    // Use a static to avoid repeated allocations
    static VERSION_STRING: std::sync::OnceLock<String> = std::sync::OnceLock::new();

    VERSION_STRING.get_or_init(|| {
        if IS_RELEASE == "true" {
            VERSION.to_string()
        } else {
            format!("{VERSION}-dev ({GIT_HASH})")
        }
    })
}

#[derive(Parser)]
#[command(name = "roz")]
#[command(author, version = version(), about = "Quality gate for Claude Code", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// [Internal] Run a hook (JSON stdin/stdout). Called by Claude Code hooks.
    Hook {
        /// Hook name (session-start, user-prompt, stop, subagent-stop).
        name: String,
    },

    /// [Agent] Post a review decision. Used by the roz:roz reviewer agent.
    Decide {
        /// Session ID.
        session_id: String,

        /// Decision type (COMPLETE or ISSUES).
        decision: String,

        /// Summary of findings.
        summary: String,

        /// Message to agent (required for ISSUES).
        #[arg(short, long)]
        message: Option<String>,

        /// Record of second opinions obtained (optional, for COMPLETE).
        #[arg(short, long)]
        opinions: Option<String>,
    },

    /// [Agent] Show user prompts for review. Used by the roz:roz reviewer agent.
    Context {
        /// Session ID.
        session_id: String,
    },

    /// [User] List recent sessions.
    List {
        /// Maximum number of sessions to show. Defaults to 20.
        #[arg(short, long)]
        limit: Option<usize>,
    },

    /// [User] Show full session state for debugging.
    Debug {
        /// Session ID.
        session_id: String,
    },

    /// [User] Show trace events for a session.
    Trace {
        /// Session ID.
        session_id: String,

        /// Show verbose output with payloads.
        #[arg(short, long)]
        verbose: bool,
    },

    /// [User] Remove old sessions.
    Clean {
        /// Duration (e.g., "7d", "30d", "24h"). Defaults to 7d.
        #[arg(long, default_value = "7d")]
        before: String,

        /// Remove all sessions.
        #[arg(long)]
        all: bool,
    },

    /// [User] Show template A/B test statistics.
    Stats {
        /// Number of days to look back. Defaults to 30.
        #[arg(long, default_value = "30")]
        days: u32,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Hook { name } => cli::hook::run(&name),
        Commands::Decide {
            session_id,
            decision,
            summary,
            message,
            opinions,
        } => cli::decide::run(
            &session_id,
            &decision,
            &summary,
            message.as_deref(),
            opinions.as_deref(),
        ),
        Commands::Context { session_id } => cli::context::run(&session_id),
        Commands::List { limit } => cli::list::run(limit),
        Commands::Debug { session_id } => cli::debug::run(&session_id),
        Commands::Trace {
            session_id,
            verbose,
        } => cli::trace::run(&session_id, verbose),
        Commands::Clean { before, all } => cli::clean::run(&before, all),
        Commands::Stats { days } => cli::stats::run(days),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("roz: error: {e}");
            ExitCode::FAILURE
        }
    }
}
