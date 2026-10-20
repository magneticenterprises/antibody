//! `antibody`: small hook command for coding agents and CI.
//!
//! Hooks fail open: any error results in no output and exit code 0 so the
//! agent is never blocked by Antibody being slow or unreachable.

mod ci;
mod client;
mod config;
mod doctor;
mod git;
mod prompt;
mod session;
mod upload;

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "antibody",
    version,
    about = "Antibody hook command for coding agents and CI"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Agent {
    #[value(name = "claude-code")]
    ClaudeCode,
    Codex,
}

#[derive(Subcommand)]
enum Command {
    /// Agent lifecycle hooks (read JSON on stdin).
    Hook {
        #[command(subcommand)]
        hook: HookCommand,
    },
    /// Upload a spooled session request (spawned by `hook session-end`).
    Upload {
        #[arg(long)]
        file: PathBuf,
    },
    /// Prepare and upload a privately spooled hook (internal detached worker).
    #[command(hide = true)]
    PrepareUpload {
        #[arg(long)]
        file: PathBuf,
    },
    /// Search prior solutions manually.
    Search(SearchArgs),
    /// CI helpers.
    Ci {
        #[command(subcommand)]
        ci: CiCommand,
    },
    /// Check configuration and connectivity.
    Doctor,
}

#[derive(Subcommand)]
enum HookCommand {
    /// SessionEnd: spool the transcript and upload it in a detached process.
    SessionEnd {
        #[arg(long, value_enum)]
        agent: Agent,
    },
    /// UserPromptSubmit: print relevant prior solutions as hook context.
    UserPrompt {
        #[arg(long, value_enum)]
        agent: Agent,
    },
}

#[derive(Args)]
struct SearchArgs {
    /// What you are about to work on.
    task: String,
    /// File containing an error message or output.
    #[arg(long)]
    error_file: Option<PathBuf>,
    /// Print the raw JSON response.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum CiCommand {
    /// Report a failed check and print known solutions. Always exits 0.
    Report {
        #[arg(long)]
        check: String,
        #[arg(long)]
        revision: String,
        #[arg(long)]
        excerpt_file: Option<PathBuf>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        component: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Hook {
            hook: HookCommand::SessionEnd { agent },
        } => {
            if let Err(e) = session::run_session_end(agent) {
                eprintln!("antibody: session-end skipped: {e:#}");
            }
            0
        }
        Command::Hook {
            hook: HookCommand::UserPrompt { agent },
        } => {
            match prompt::run_user_prompt(agent) {
                Ok(Some(out)) => println!("{out}"),
                Ok(None) => {}
                Err(e) => eprintln!("antibody: prompt hook skipped: {e:#}"),
            }
            0
        }
        Command::Upload { file } => {
            upload::run_upload(&file);
            0
        }
        Command::PrepareUpload { file } => {
            session::run_prepare_upload(&file);
            0
        }
        Command::Search(a) => match prompt::run_search(&a.task, a.error_file.as_deref(), a.json) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("antibody: {e:#}");
                1
            }
        },
        Command::Ci {
            ci:
                CiCommand::Report {
                    check,
                    revision,
                    excerpt_file,
                    url,
                    component,
                },
        } => {
            if let Err(e) =
                ci::run_report(&check, &revision, excerpt_file.as_deref(), url, component)
            {
                eprintln!("antibody: ci report failed (not blocking): {e:#}");
            }
            0
        }
        Command::Doctor => doctor::run(),
    };
    std::process::exit(code);
}
