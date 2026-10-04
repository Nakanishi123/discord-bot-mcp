use std::{sync::Arc, time::Duration};

use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::{Client, Url};
use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::json;
use tokio::sync::Semaphore;

use crate::{
    config::Config,
    discord::{Discord, id},
    error::ToolError,
};

#[derive(Clone)]
pub struct Images {
    client: Client,
    permits: Arc<Semaphore>,
    maximum_bytes: usize,
    response_bytes: usize,
    timeout: Duration,
}

impl Images {
    pub fn new(config: &Config) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(config.image_timeout)
                .https_only(true)
                .build()?,
            permits: Arc::new(Semaphore::new(config.image_concurrency)),
            maximum_bytes: config.image_bytes,
            response_bytes: config.response_bytes,
            timeout: config.image_timeout,
        })
    }

    pub async fn read(
        &self,
        discord: &Discord,
        channel: &str,
        message: &str,
        attachment: &str,
    ) -> Result<CallToolResult, ToolError> {
        discord.authorize(channel)?;
        let attachment_id = id(attachment)?;
        let _permit = self.permits.try_acquire().map_err(|_| ToolError::Busy)?;
        tokio::time::timeout(self.timeout, async {
            let message = discord.message(channel, message).await?;
            let attachment = message
                .attachments
                .iter()
                .find(|attachment| attachment.id.get() == attachment_id)
                .ok_or(ToolError::NotFound)?;
            let media_type = attachment
                .content_type
                .as_deref()
                .ok_or(ToolError::UnsupportedImage)?;
            if !matches!(media_type, "image/jpeg" | "image/png") {
                return Err(ToolError::UnsupportedImage);
            }
            if u64::from(attachment.size) > self.maximum_bytes as u64 {
                return Err(ToolError::TooLarge);
            }
            let url = attachment_url(&attachment.url)?;
            let bytes = self.download(url).await?;
            self.encode(
                &bytes,
                media_type,
                &message.channel_id.to_string(),
                &message.id.to_string(),
                &attachment.id.to_string(),
            )
        })
        .await
        .map_err(|_| ToolError::Timeout)?
    }

    fn encode(
        &self,
        bytes: &[u8],
        media_type: &str,
        channel: &str,
        message: &str,
        attachment: &str,
    ) -> Result<CallToolResult, ToolError> {
        validate_signature(bytes, media_type)?;
        if bytes.len() > self.maximum_bytes
            || bytes
                .len()
                .div_ceil(3)
                .saturating_mul(4)
                .saturating_add(65_536)
                > self.response_bytes
        {
            return Err(ToolError::TooLarge);
        }
        let result = CallToolResult::success(vec![
            ContentBlock::text(
                json!({"channel_id":channel, "message_id":message, "attachment_id":attachment})
                    .to_string(),
            ),
            ContentBlock::image(STANDARD.encode(bytes), media_type),
        ]);
        // JSON-RPCの包みとリクエストID用に余裕を確保する。
        let length = serde_json::to_vec(&result)
            .map_err(|_| ToolError::Download)?
            .len();
        if length.saturating_add(65_536) > self.response_bytes {
            return Err(ToolError::TooLarge);
        }
        Ok(result)
    }

    async fn download(&self, url: Url) -> Result<Vec<u8>, ToolError> {
        let mut response = self.client.get(url).send().await.map_err(ToolError::from)?;
        if !response.status().is_success() {
            return Err(ToolError::Download);
        }
        if response
            .content_length()
            .is_some_and(|length| length > self.maximum_bytes as u64)
        {
            return Err(ToolError::TooLarge);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(ToolError::from)? {
            if chunk.len() > self.maximum_bytes.saturating_sub(bytes.len()) {
                return Err(ToolError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

fn attachment_url(value: &str) -> Result<Url, ToolError> {
    let url = Url::parse(value).map_err(|_| ToolError::Download)?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("cdn.discordapp.com" | "media.discordapp.net")
        )
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || !url.path().starts_with("/attachments/")
    {
        return Err(ToolError::Download);
    }
    Ok(url)
}

fn validate_signature(bytes: &[u8], media_type: &str) -> Result<(), ToolError> {
    if (media_type == "image/png" && bytes.starts_with(b"\x89PNG\r\n\x1a\n"))
        || (media_type == "image/jpeg" && bytes.starts_with(&[0xff, 0xd8, 0xff]))
    {
        Ok(())
    } else {
        Err(ToolError::UnsupportedImage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_untrusted_attachment_locations_and_mislabeled_images() {
        for url in [
            "http://cdn.discordapp.com/attachments/1",
            "https://example.com/attachments/1",
            "https://cdn.discordapp.com.evil.test/attachments/1",
            "https://cdn.discordapp.com:444/attachments/1",
            "https://user@cdn.discordapp.com/attachments/1",
            "https://cdn.discordapp.com/api",
        ] {
            assert!(attachment_url(url).is_err());
        }
        assert!(
            attachment_url("https://cdn.discordapp.com/attachments/1/2/image.png?ex=123").is_ok()
        );
        assert!(validate_signature(b"GIF89a", "image/png").is_err());
        assert!(validate_signature(b"\x89PNG\r\n\x1a\n", "image/png").is_ok());
        assert!(validate_signature(&[0xff, 0xd8, 0xff], "image/jpeg").is_ok());
    }

    #[tokio::test]
    async fn downloads_original_bytes_and_rejects_streamed_overflow_and_redirects()
    -> anyhow::Result<()> {
        use crate::test_support::{config, image_server};
        use axum::http::{HeaderMap, StatusCode};

        let mut images = Images::new(&config())?;
        images.client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let bytes: &'static [u8] = b"\x89PNG\r\n\x1a\noriginal bytes";
        let server = image_server(StatusCode::OK, HeaderMap::new(), vec![bytes]).await?;
        assert_eq!(images.download(Url::parse(&server.url)?).await?, bytes);
        images.maximum_bytes = 8;
        assert!(matches!(
            images.download(Url::parse(&server.url)?).await,
            Err(ToolError::TooLarge)
        ));
        let server = image_server(StatusCode::FOUND, HeaderMap::new(), vec![]).await?;
        assert!(matches!(
            images.download(Url::parse(&server.url)?).await,
            Err(ToolError::Download)
        ));
        let mut headers = HeaderMap::new();
        headers.insert("content-length", bytes.len().to_string().parse()?);
        let server = image_server(StatusCode::OK, headers, vec![bytes]).await?;
        assert!(matches!(
            images.download(Url::parse(&server.url)?).await,
            Err(ToolError::TooLarge)
        ));
        Ok(())
    }

    #[test]
    fn encodes_unchanged_images_and_enforces_response_size() -> anyhow::Result<()> {
        let mut images = Images::new(&crate::test_support::config())?;
        for (bytes, media_type) in [
            (b"\x89PNG\r\n\x1a\noriginal bytes".as_slice(), "image/png"),
            ([0xff, 0xd8, 0xff, 0x01].as_slice(), "image/jpeg"),
        ] {
            let result =
                serde_json::to_value(images.encode(bytes, media_type, "123", "456", "789")?)?;
            assert_eq!(result["content"][1]["mimeType"], media_type);
            let data = result["content"][1]["data"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing image data"))?;
            assert_eq!(STANDARD.decode(data)?, bytes);
            let metadata: serde_json::Value = serde_json::from_str(
                result["content"][0]["text"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing metadata"))?,
            )?;
            assert_eq!(
                metadata,
                json!({"channel_id":"123", "message_id":"456", "attachment_id":"789"})
            );
        }
        images.response_bytes = 10;
        assert!(matches!(
            images.encode(b"\x89PNG\r\n\x1a\n", "image/png", "1", "2", "3"),
            Err(ToolError::TooLarge)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn rejects_forbidden_channels_missing_attachments_and_exhausted_capacity()
    -> anyhow::Result<()> {
        use crate::test_support::{config, discord, message, mock_server};
        use axum::http::StatusCode;

        let server = mock_server(vec![(StatusCode::OK, message("456"))]).await?;
        let discord = discord(&server)?;
        let images = Images::new(&config())?;
        assert!(matches!(
            images.read(&discord, "999", "456", "789").await,
            Err(ToolError::ForbiddenChannel)
        ));
        assert!(server.requests.lock().await.is_empty());
        assert!(matches!(
            images.read(&discord, "123", "456", "789").await,
            Err(ToolError::NotFound)
        ));
        let _permits = images.permits.acquire_many(2).await?;
        assert!(matches!(
            images.read(&discord, "123", "456", "789").await,
            Err(ToolError::Busy)
        ));
        assert_eq!(server.requests.lock().await.len(), 1);
        Ok(())
    }
    #[tokio::test]
    async fn times_out_message_lookup_and_releases_capacity() -> anyhow::Result<()> {
        use crate::test_support::{config, discord, stalled_server};
        let server = stalled_server().await?;
        let discord = discord(&server)?;
        let mut configuration = config();
        configuration.image_timeout = Duration::from_millis(20);
        let images = Images::new(&configuration)?;
        assert!(matches!(
            images.read(&discord, "123", "456", "789").await,
            Err(ToolError::Timeout)
        ));
        assert_eq!(images.permits.available_permits(), 2);
        Ok(())
    }
}
