use anyhow::Result;
use clap::{Parser, Subcommand};
use freemodel_workbuddy_proxy::{
    config::Config,
    server::{AppState, serve},
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "freemodel-workbuddy-proxy",
    version,
    about = "OpenAI-compatible Freemodel proxy using official WorkBuddy ACP"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    Server,
    Tui,
    Key {
        #[command(subcommand)]
        command: KeyCommand,
    },
}
#[derive(Subcommand)]
enum KeyCommand {
    Set { value: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "freemodel_workbuddy_proxy=info,tower_http=info".into()),
        )
        .init();
    let cli = Cli::parse();
    let root = std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
                .map(PathBuf::from)
        })
        .filter(|p| p.join("config.json").exists())
        .unwrap_or(std::env::current_dir()?);
    let config = Config::load(root)?;
    match cli.command.unwrap_or(Command::Tui) {
        Command::Server => serve(AppState::new(config)?).await?,
        Command::Tui => freemodel_workbuddy_proxy::tui::run(&config).await?,
        Command::Key {
            command: KeyCommand::Set { value },
        } => {
            config.save_api_key(&value)?;
            println!("API key saved.");
        }
    }
    Ok(())
}
