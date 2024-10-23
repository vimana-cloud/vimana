//! Entrypoint to the Vimana CLI.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use user::{login, logout};

#[derive(Parser)]
#[command(
    version,
    about = "The Vimana CLI provides a convenient interface to the Vimana API."
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

/// Top-level subcommands.
#[derive(Subcommand)]
enum Command {
    /// Log in or out.
    User {
        #[command(subcommand)]
        command: UserCommand,
    },

    /// Claim or administer domains.
    Domain,
}

/// Subcommands under `user`.
#[derive(Subcommand)]
enum UserCommand {
    /// Refresh authentication.
    ///
    /// Opens a web browser to perform an OIDC login flow.
    Login {
        /// Manually copy and paste the login URI and ID token.
        #[arg(short, long)]
        manual: bool,
    },

    /// Forget authentication session, if any.
    Logout,
}

fn main() -> ExitCode {
    let args = Args::parse();

    match args.command {
        Command::User { command } => match command {
            UserCommand::Login { manual } => login(manual),
            UserCommand::Logout => logout(),
        },
        Command::Domain => todo!(),
    }
}
