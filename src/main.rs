mod config;
mod discord;
mod error;
mod gateway;
mod http;
mod image;
mod mcp;

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use serenity::{all::GatewayIntents, client::ClientBuilder, http::HttpBuilder};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

use crate::{
    config::Config, discord::Discord, error::discord_failure, gateway::Gateway, image::Images,
    mcp::Mcp,
};

#[tokio::main]
async fn main() -> Result<()> {
    // 外部SDKのログには本文やURLが含まれ得るため、アプリの運用ログだけを出力する。
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("off,discord_bot_mcp=info"))
        .init();
    let config = Config::from_env()?;
    let rest = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let http = HttpBuilder::new(&config.discord_token).client(rest).build();
    let channels = Arc::new(config.channels.clone());
    let intents =
        GatewayIntents::GUILDS | GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;
    let mut client = ClientBuilder::new_with_http(http, intents)
        .event_handler(Gateway {
            channels: Arc::clone(&channels),
        })
        .await
        .map_err(discord_failure)
        .context("Could not initialize Discord client")?;
    let discord = Discord {
        http: Arc::clone(&client.http),
        channels,
    };
    discord
        .validate_channels()
        .await
        .context("Configured Discord channels could not be validated")?;
    let server = Mcp::new(discord, Images::new(&config)?);
    let cancellation = CancellationToken::new();
    let app = http::router(server, &config, cancellation.clone());
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    tracing::info!(bind = %config.bind, "HTTP server ready");
    let manager = Arc::clone(&client.shard_manager);
    let mut gateway = tokio::spawn(async move { client.start().await });
    let stopped = cancellation.clone();
    let mut http = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(stopped.cancelled_owned())
            .await
    });
    let result = tokio::select! {
        result = shutdown_signal() => result,
        result = &mut gateway => Err(task_failure("Discord Gateway", result.map(|result| result.map_err(discord_failure)))),
        result = &mut http => Err(task_failure("HTTP server", result.map(|result| result.map_err(|error| anyhow::anyhow!("HTTP I/O error: {:?}", error.kind()))))),
    };
    if let Err(error) = &result {
        tracing::error!(reason = %format_args!("{error:#}"), "Server stopped");
    }
    tracing::info!("Shutting down");
    cancellation.cancel();
    let cleanup = async {
        manager.shutdown_all().await;
        if !gateway.is_finished() {
            let _ = (&mut gateway).await;
        }
        if !http.is_finished() {
            let _ = (&mut http).await;
        }
    };
    if tokio::time::timeout(Duration::from_secs(10), cleanup)
        .await
        .is_err()
    {
        gateway.abort();
        http.abort();
        tracing::warn!("Shutdown grace period expired");
    }
    result
}

fn task_failure(task: &str, result: Result<Result<()>, tokio::task::JoinError>) -> anyhow::Error {
    match result {
        Ok(Ok(())) => anyhow::anyhow!("{task} stopped unexpectedly without an error"),
        Ok(Err(error)) => error.context(format!("{task} failed")),
        Err(error) => {
            // panicのペイロードには秘密情報が含まれ得るため、終了種別だけを記録する。
            if error.is_panic() {
                anyhow::anyhow!("{task} task panicked")
            } else {
                anyhow::anyhow!("{task} task was cancelled")
            }
        }
    }
}

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result?,
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn distinguishes_task_completion_failure_and_cancellation() -> Result<()> {
        assert!(
            task_failure("HTTP server", Ok(Ok(())))
                .to_string()
                .contains("without an error")
        );
        let error = task_failure(
            "Discord Gateway",
            Ok(Err(discord_failure(serenity::Error::Gateway(
                serenity::gateway::GatewayError::DisallowedGatewayIntents,
            )))),
        );
        let reason = format!("{error:#}");
        assert!(reason.contains("Discord Gateway failed"));
        assert!(reason.contains("Disallowed gateway intents"));
        let handle = tokio::spawn(std::future::pending::<Result<()>>());
        handle.abort();
        assert!(
            task_failure("HTTP server", handle.await)
                .to_string()
                .contains("was cancelled")
        );
        Ok(())
    }
}
