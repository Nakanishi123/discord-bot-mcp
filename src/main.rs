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

use crate::{config::Config, discord::Discord, gateway::Gateway, image::Images, mcp::Mcp};

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
        .map_err(|_| anyhow::anyhow!("Could not initialize Discord client"))?;
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
        _ = &mut gateway => Err(anyhow::anyhow!("Discord Gateway stopped unexpectedly")),
        _ = &mut http => Err(anyhow::anyhow!("HTTP server stopped unexpectedly")),
    };
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
