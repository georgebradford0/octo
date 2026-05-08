//! Merged binary that runs either the lair (parent / orchestrator) or server
//! (child / repo-scoped agentic loop) role. The Docker image ships one binary;
//! the entrypoint scripts pick which role to run by passing `--role`.

use clap::{Parser, ValueEnum};

mod bootstrap;
mod lair;
mod server;

#[derive(Clone, Copy, ValueEnum)]
pub enum Role {
    Lair,
    Server,
}

#[derive(Parser)]
#[command(version, about = "octo merged app — pick role with --role")]
struct Args {
    /// Which role to run.
    #[arg(long, value_enum)]
    role: Role,

    /// Print the Noise static pubkey (base32) for the picked role and exit.
    /// Used by entrypoint scripts to embed the pubkey in the QR code before
    /// the server starts listening.
    #[arg(long)]
    print_pubkey: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    match args.role {
        Role::Lair   => lair::run(args.print_pubkey).await,
        Role::Server => server::run(args.print_pubkey).await,
    }
}
