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
                Some(401 | 403) => return Self::PermissionDenied,
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
