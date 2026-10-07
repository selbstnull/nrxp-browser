use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "nrb", version, about = "netrunner-browser")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Заглушка до I1: покажет, что каркас собирается.
    Version,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Version) | None => println!("nrb {}", env!("CARGO_PKG_VERSION")),
    }
    Ok(())
}
