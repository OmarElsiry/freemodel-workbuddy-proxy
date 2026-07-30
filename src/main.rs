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
    Set { value: Option<String> },
}

fn ensure_tui_api_key(
    root: &std::path::Path,
    mut config: Config,
    read_key: impl FnOnce() -> Result<String>,
) -> Result<Config> {
    if !config.api_key.is_empty() {
        return Ok(config);
    }
    println!(
        "No usable Freemodel API key was found. Enter it once to save it privately in config.json."
    );
    let value = read_key()?;
    if value.trim().is_empty() {
        anyhow::bail!("API key was empty; configuration was not changed");
    }
    config.save_api_key(&value)?;
    config = Config::load(root)?;
    println!("API key saved.");
    Ok(config)
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
        .filter(|p| p.join("Cargo.toml").is_file() || p.join("config.json").is_file())
        .unwrap_or(std::env::current_dir()?);
    let command = cli.command.unwrap_or(Command::Tui);
    let mut config = Config::load(&root)?;
    if matches!(command, Command::Tui) {
        config = ensure_tui_api_key(&root, config, || {
            freemodel_workbuddy_proxy::tui::setup::read_secret("Freemodel API key")
        })?;
    }
    match command {
        Command::Server => serve(AppState::new(config)?).await?,
        Command::Tui => freemodel_workbuddy_proxy::tui::run(&config).await?,
        Command::Key {
            command: KeyCommand::Set { value },
        } => {
            let value = match value {
                Some(value) => value,
                None => freemodel_workbuddy_proxy::tui::setup::read_secret("Freemodel API key")?,
            };
            if value.trim().is_empty() {
                anyhow::bail!("API key was empty; configuration was not changed");
            }
            config.save_api_key(&value)?;
            println!("API key saved.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ensure_tui_api_key;
    use freemodel_workbuddy_proxy::config::Config;
    use std::{collections::HashMap, os::unix::fs::PermissionsExt};

    fn config(root: &std::path::Path) -> Config {
        Config::load_with_env(
            root,
            &HashMap::from([("HOME".into(), root.to_string_lossy().to_string())]),
        )
        .unwrap()
    }

    #[test]
    fn first_tui_start_saves_private_key_and_reloads_it() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("config.json"),
            r#"{"UNRELATED":{"keep":true}}"#,
        )
        .unwrap();
        let loaded = ensure_tui_api_key(root.path(), config(root.path()), || {
            Ok("  first-run-key  ".into())
        })
        .unwrap();
        assert_eq!(loaded.api_key, "first-run-key");
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.path().join("config.json")).unwrap())
                .unwrap();
        assert_eq!(saved["FREEMODEL_API_KEY"], "first-run-key");
        assert_eq!(saved["UNRELATED"]["keep"], true);
        assert_eq!(
            std::fs::metadata(root.path().join("config.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn existing_key_skips_prompt_and_empty_key_does_not_write() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("config.json"),
            r#"{"FREEMODEL_API_KEY":"existing-key"}"#,
        )
        .unwrap();
        let loaded = ensure_tui_api_key(root.path(), config(root.path()), || {
            panic!("existing key must skip the prompt")
        })
        .unwrap();
        assert_eq!(loaded.api_key, "existing-key");

        let missing = tempfile::tempdir().unwrap();
        let error = ensure_tui_api_key(missing.path(), config(missing.path()), || Ok("   ".into()))
            .unwrap_err();
        assert!(error.to_string().contains("API key was empty"));
        assert!(!missing.path().join("config.json").exists());
    }
}
