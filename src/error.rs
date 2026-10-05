use rmcp::{ErrorData, model::CallToolResult};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("The channel is not allowed")]
    ForbiddenChannel,
    #[error("The channel type is not supported")]
    UnsupportedChannel,
    #[error("The requested resource was not found")]
    NotFound,
    #[error("Discord Bot authentication failed; check the server Bot token")]
    DiscordUnauthorized,
    #[error("Discord denied access")]
    PermissionDenied,
    #[error("Only JPEG and PNG attachments are supported")]
    UnsupportedImage,
    #[error("The image or encoded response exceeds the configured size limit")]
    TooLarge,
    #[error("The operation timed out; a write may have completed. Check history before retrying")]
    Timeout,
    #[error("Image download capacity is exhausted; retry later")]
    Busy,
    #[error("Discord request failed; a write may have completed. Check history before retrying")]
    Discord,
    #[error("The attachment could not be downloaded")]
    Download,
}

impl ToolError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid_input",
            Self::ForbiddenChannel => "channel_not_allowed",
            Self::UnsupportedChannel => "unsupported_channel",
            Self::NotFound => "not_found",
            Self::DiscordUnauthorized => "discord_unauthorized",
            Self::PermissionDenied => "permission_denied",
            Self::UnsupportedImage => "unsupported_image",
            Self::TooLarge => "size_limit_exceeded",
            Self::Timeout => "timeout",
            Self::Busy => "busy",
            Self::Discord => "discord_error",
            Self::Download => "download_failed",
        }
    }

    pub fn into_result(self) -> Result<CallToolResult, ErrorData> {
        if let Self::Invalid(message) = self {
            Err(ErrorData::invalid_params(message, None))
        } else {
            Ok(CallToolResult::structured_error(
                json!({"code": self.code(), "message": self.to_string()}),
            ))
        }
    }
}

impl From<serenity::Error> for ToolError {
    fn from(error: serenity::Error) -> Self {
        if let serenity::Error::Http(error) = &error {
            if let serenity::http::HttpError::Request(error) = error
                && error.is_timeout()
            {
                return Self::Timeout;
            }
            match error.status_code().map(|status| status.as_u16()) {
                Some(404) => return Self::NotFound,
                Some(401) => return Self::DiscordUnauthorized,
                Some(403) => return Self::PermissionDenied,
                _ => {}
            }
        }
        Self::Discord
    }
}

impl From<reqwest::Error> for ToolError {
    fn from(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout
        } else {
            Self::Download
        }
    }
}

// GatewayとClientのDisplayは固定文言だけを返す。他のSDKエラーの本文やURLは出力しない。
pub fn discord_failure(error: serenity::Error) -> anyhow::Error {
    if let serenity::Error::Gateway(error) = &error {
        return anyhow::anyhow!("Discord Gateway: {error}");
    }
    if let serenity::Error::Client(error) = &error {
        return anyhow::anyhow!("Discord client: {error}");
    }
    let error = ToolError::from(error);
    anyhow::anyhow!("{}: {error}", error.code())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_gateway_causes_without_exposing_other_sdk_details() {
        for (error, expected) in [
            (
                serenity::gateway::GatewayError::InvalidAuthentication,
                "Sent invalid authentication",
            ),
            (
                serenity::gateway::GatewayError::DisallowedGatewayIntents,
                "Disallowed gateway intents were provided",
            ),
        ] {
            assert!(
                discord_failure(serenity::Error::Gateway(error))
                    .to_string()
                    .contains(expected)
            );
        }
        let error = discord_failure(serenity::Error::Other("private response and URL"));
        assert!(error.to_string().contains("discord_error"));
        assert!(!error.to_string().contains("private response"));
    }
}
