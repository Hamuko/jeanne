use clap::Parser;
use simple_logger::SimpleLogger;
use std::borrow::Cow;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use tokio::{task, time};

mod config;
mod qbittorrent;

const UNLIMITED: &str = "unlimited";
const GLOBAL: &str = "global";

#[derive(Parser)]
#[command(name = "jeanne", version)]
struct Cli {
    /// Path to the configuration Yaml file.
    config: PathBuf,
}

async fn handle_torrent(
    config: &config::Config,
    client: &qbittorrent::Client,
    hash: &str,
    torrent: &qbittorrent::Torrent,
) -> Option<Result<(), qbittorrent::ClientError>> {
    if let Some(rule) = config.rules.find(torrent) {
        if rule.needs_update(torrent) {
            log::info!(
                "Applying matched rule to {}; ratio: {} => {}; total minutes: {} => {}",
                torrent.name,
                if torrent.max_ratio == -1.0 {
                    Cow::from(UNLIMITED)
                } else {
                    Cow::from(torrent.max_ratio.to_string())
                },
                match rule.limits.ratio {
                    Some(ratio) => Cow::from(ratio.to_string()),
                    None => Cow::from(GLOBAL),
                },
                if torrent.max_seeding_time == -1 {
                    Cow::from(UNLIMITED)
                } else {
                    Cow::from(torrent.max_seeding_time.to_string())
                },
                match rule.limits.minutes {
                    Some(minutes) => Cow::from(minutes.to_string()),
                    None => Cow::from(GLOBAL),
                },
            );
            return Some(client.apply_rule_limits(hash, &rule.limits).await);
        }
    } else if torrent.is_limited() {
        log::info!(
            "Torrent {} is limited despite not being matched: setting to global limits",
            torrent.name
        );
        return Some(client.apply_global_limits(hash).await);
    }
    None
}

async fn run(
    config: &config::Config,
    client: &mut qbittorrent::Client,
) -> Result<(), qbittorrent::ClientError> {
    client.update().await?;
    for (hash, torrent) in &client.torrents {
        if let Some(result) = handle_torrent(config, client, hash, torrent).await {
            match result {
                Ok(()) => log::debug!("Successfully updated {}", hash),
                Err(error) => log::warn!("Couldn't update {}: {:?}", hash, error),
            }
        };
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .env()
        .init()
        .expect("Could not set up logger");

    let cli = Cli::parse();

    log::debug!("Using configuration at {}", cli.config.display());
    let mut config = match config::Config::load(&cli.config) {
        Ok(config) => config,
        Err(config::ConfigError::Deserialization(error)) => {
            log::error!("Could not parse configuration file: {}", error);
            return ExitCode::FAILURE;
        }
        Err(config::ConfigError::Io(error)) => {
            log::error!("Could not load configuration file: {}", error);
            return ExitCode::FAILURE;
        }
    };
    log::info!("Loaded configuration with {} rules", &config.rules.len());
    for (i, rule) in config.rules.iter().enumerate() {
        log::info!("Rule #{}: {}", i + 1, rule);
    }

    let mut client = match qbittorrent::Client::new(std::mem::take(&mut config.server)) {
        Ok(client) => client,
        Err(error) => {
            match error {
                qbittorrent::ClientError::Reqwest(reqwest_error) => {
                    log::error!("HTTP client error: {}", reqwest_error)
                }
                _ => {
                    log::error!("Unknown error error: {:?}", error)
                }
            }
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = client.login().await {
        match error {
            qbittorrent::AuthenticationError::MissingCredentials => {
                log::info!("No login: username and password are not set")
            }
            _ => {
                log::error!("{}", error);
                return ExitCode::FAILURE;
            }
        }
    };

    let forever = task::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(60));

        loop {
            interval.tick().await;
            if let Err(error) = run(&config, &mut client).await {
                match error {
                    qbittorrent::ClientError::Authentication => {
                        log::warn!("No permission to access server");
                        match client.login().await {
                            Ok(()) => log::info!("Reauthenticated"),
                            Err(error) => {
                                log::error!("{}", error);
                                return ExitCode::FAILURE;
                            }
                        };
                    }
                    qbittorrent::ClientError::InvalidUrl => {
                        log::error!("Configuration did not contain a valid base URL")
                    }
                    qbittorrent::ClientError::Reqwest(reqwest_error) => {
                        log::error!("HTTP client error: {}", reqwest_error)
                    }
                    _ => log::warn!("Unknown error while updating"),
                }
            };
        }
    });

    forever.await.unwrap()
}
