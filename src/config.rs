use std::{collections::HashSet, env, net::SocketAddr, time::Duration};

use anyhow::{Context, Result, bail, ensure};

pub struct Config {
    pub discord_token: String,
    pub bearer_token: String,
    pub channels: HashSet<u64>,
    pub bind: SocketAddr,
    pub image_bytes: usize,
    pub response_bytes: usize,
    pub image_concurrency: usize,
    pub image_timeout: Duration,
    pub allowed_hosts: Vec<String>,
    pub allowed_origins: Vec<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let required = |name: &str| -> Result<String> {
            let value = get(name).with_context(|| format!("{name} is required"))?;
            ensure!(!value.trim().is_empty(), "{name} must not be empty");
            Ok(value)
        };
        let number = |name: &str, default: usize, maximum: usize| -> Result<usize> {
            let value = get(name)
                .map_or(Ok(default), |value| value.parse::<usize>())
                .with_context(|| format!("{name} must be an integer"))?;
            ensure!(
                (1..=maximum).contains(&value),
                "{name} is outside its supported range"
            );
            Ok(value)
        };
        let discord_token = required("DISCORD_BOT_TOKEN")?;
        let bearer_token = required("MCP_BEARER_TOKEN")?;
        ensure!(
            bearer_token.len() >= 32
                && bearer_token.len() <= 256
                && bearer_token.bytes().all(|byte| byte.is_ascii_graphic()),
            "MCP_BEARER_TOKEN must contain 32 to 256 visible ASCII characters"
        );
        ensure!(
            discord_token != bearer_token,
            "Discord and MCP tokens must differ"
        );
        let channels = required("DISCORD_CHANNEL_IDS")?
            .split(',')
            .map(|value| {
                parse_id(value.trim()).context("DISCORD_CHANNEL_IDS contains an invalid ID")
            })
            .collect::<Result<HashSet<_>>>()?;
        ensure!(
            !channels.is_empty(),
            "DISCORD_CHANNEL_IDS must not be empty"
        );
        let list = |name: &str, default: &str| -> Vec<String> {
            get(name)
                .unwrap_or_else(|| default.to_owned())
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect()
        };
        let allowed_hosts = list("MCP_ALLOWED_HOSTS", "localhost,127.0.0.1,[::1]");
        ensure!(
            !allowed_hosts.is_empty(),
            "MCP_ALLOWED_HOSTS must not be empty"
        );
        Ok(Self {
            discord_token,
            bearer_token,
            channels,
            bind: get("HTTP_BIND")
                .unwrap_or_else(|| "0.0.0.0:8080".to_owned())
                .parse()
                .context("HTTP_BIND must be an IP address and port")?,
            image_bytes: number("IMAGE_MAX_BYTES", 20 * 1024 * 1024, 100 * 1024 * 1024)?,
            response_bytes: number(
                "IMAGE_RESPONSE_MAX_BYTES",
                28 * 1024 * 1024,
                140 * 1024 * 1024,
            )?,
            image_concurrency: number("IMAGE_CONCURRENCY", 2, 16)?,
            image_timeout: Duration::from_secs(u64::try_from(number(
                "IMAGE_TIMEOUT_SECONDS",
                30,
                300,
            )?)?),
            allowed_hosts,
            allowed_origins: list("MCP_ALLOWED_ORIGINS", ""),
        })
    }
}

pub fn parse_id(value: &str) -> Result<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("IDs must be nonzero decimal strings");
    }
    let id = value.parse::<u64>()?;
    ensure!(id > 0, "IDs must be nonzero decimal strings");
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configuration(name: &str) -> Option<String> {
        match name {
            "DISCORD_BOT_TOKEN" => Some("discord-test-token".to_owned()),
            "MCP_BEARER_TOKEN" => Some("a".repeat(32)),
            "DISCORD_CHANNEL_IDS" => Some("123,456".to_owned()),
            _ => None,
        }
    }

    #[test]
    fn rejects_missing_secrets_empty_allowlist_and_invalid_limits() {
        for name in [
            "DISCORD_BOT_TOKEN",
            "MCP_BEARER_TOKEN",
            "DISCORD_CHANNEL_IDS",
        ] {
            assert!(
                Config::from_lookup(|key| if key == name {
                    None
                } else {
                    configuration(key)
                })
                .is_err()
            );
            assert!(
                Config::from_lookup(|key| if key == name {
                    Some(String::new())
                } else {
                    configuration(key)
                })
                .is_err()
            );
        }
        for name in [
            "IMAGE_MAX_BYTES",
            "IMAGE_RESPONSE_MAX_BYTES",
            "IMAGE_CONCURRENCY",
            "IMAGE_TIMEOUT_SECONDS",
        ] {
            assert!(
                Config::from_lookup(|key| if key == name {
                    Some("0".to_owned())
                } else {
                    configuration(key)
                })
                .is_err()
            );
        }
    }

    #[test]
    fn parses_defaults_and_large_ids_without_rounding() -> Result<()> {
        let config = Config::from_lookup(configuration)?;
        assert_eq!(config.image_bytes, 20 * 1024 * 1024);
        assert_eq!(config.channels.len(), 2);
        assert_eq!(parse_id("18446744073709551615")?, u64::MAX);
        for value in ["0", "-1", "+1", "1.0", "", "18446744073709551616"] {
            assert!(parse_id(value).is_err());
        }
        Ok(())
    }
}
