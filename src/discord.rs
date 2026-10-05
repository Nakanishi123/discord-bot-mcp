use std::{collections::HashSet, sync::Arc, time::Duration};

use serde::Serialize;
use serenity::{
    all::{
        Channel, ChannelId, ChannelType, CreateAllowedMentions, CreateMessage, Message, MessageId,
        ReactionType,
    },
    http::{Http, MessagePagination},
};

use crate::{config::parse_id, error::ToolError};

#[derive(Clone)]
pub struct Discord {
    pub http: Arc<Http>,
    pub channels: Arc<HashSet<u64>>,
}

#[derive(Serialize)]
pub struct AttachmentView {
    pub attachment_id: String,
    pub filename: String,
    pub media_type: Option<String>,
    pub size: u32,
}

#[derive(Serialize)]
pub struct MessageView {
    pub channel_id: String,
    pub message_id: String,
    pub author_id: String,
    pub content: String,
    pub content_truncated: bool,
    pub timestamp: String,
    pub attachments: Vec<AttachmentView>,
}

impl From<Message> for MessageView {
    fn from(message: Message) -> Self {
        Self {
            channel_id: message.channel_id.to_string(),
            message_id: message.id.to_string(),
            author_id: message.author.id.to_string(),
            content: message.content,
            content_truncated: false,
            timestamp: message.timestamp.to_string(),
            attachments: message
                .attachments
                .into_iter()
                .map(|attachment| AttachmentView {
                    attachment_id: attachment.id.to_string(),
                    filename: attachment.filename,
                    media_type: attachment.content_type,
                    size: attachment.size,
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
pub struct MessagePage {
    pub messages: Vec<MessageView>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub has_more: bool,
}

pub fn id(value: &str) -> Result<u64, ToolError> {
    parse_id(value).map_err(|_| ToolError::Invalid("IDs must be nonzero decimal strings"))
}

pub async fn request<T>(
    future: impl Future<Output = Result<T, serenity::Error>>,
) -> Result<T, ToolError> {
    tokio::time::timeout(Duration::from_secs(30), future)
        .await
        .map_err(|_| ToolError::Timeout)?
        .map_err(Into::into)
}

impl Discord {
    pub fn authorize(&self, channel: &str) -> Result<ChannelId, ToolError> {
        let channel = id(channel)?;
        if !self.channels.contains(&channel) {
            return Err(ToolError::ForbiddenChannel);
        }
        Ok(ChannelId::new(channel))
    }

    pub async fn validate_channel(&self, channel: ChannelId) -> Result<(), ToolError> {
        let channel = request(self.http.get_channel(channel)).await?;
        if let Channel::Guild(channel) = channel
            && matches!(channel.kind, ChannelType::Text | ChannelType::News)
        {
            return Ok(());
        }
        Err(ToolError::UnsupportedChannel)
    }

    pub async fn validate_channels(&self) -> Result<(), ToolError> {
        for &channel in self.channels.iter() {
            self.validate_channel(ChannelId::new(channel)).await?;
        }
        Ok(())
    }

    pub async fn messages(
        &self,
        channel: &str,
        before: Option<&str>,
        after: Option<&str>,
        limit: Option<u8>,
    ) -> Result<MessagePage, ToolError> {
        let channel = self.authorize(channel)?;
        let limit = limit.unwrap_or(20);
        if !(1..=100).contains(&limit) {
            return Err(ToolError::Invalid("limit must be between 1 and 100"));
        }
        let target = match (before, after) {
            (Some(_), Some(_)) => {
                return Err(ToolError::Invalid(
                    "before and after are mutually exclusive",
                ));
            }
            (Some(before), None) => Some(MessagePagination::Before(MessageId::new(id(before)?))),
            (None, Some(after)) => Some(MessagePagination::After(MessageId::new(id(after)?))),
            (None, None) => None,
        };
        let mut messages = request(self.http.get_messages(channel, target, Some(limit))).await?;
        messages.sort_by_key(|message| std::cmp::Reverse(message.id));
        Ok(MessagePage {
            before: messages.last().map(|message| message.id.to_string()),
            after: messages.first().map(|message| message.id.to_string()),
            has_more: messages.len() == usize::from(limit),
            messages: messages.into_iter().map(Into::into).collect(),
        })
    }

    pub async fn message(&self, channel: &str, message: &str) -> Result<Message, ToolError> {
        let channel = self.authorize(channel)?;
        let message = MessageId::new(id(message)?);
        request(self.http.get_message(channel, message)).await
    }

    pub async fn send(
        &self,
        channel: &str,
        reply: Option<&str>,
        content: &str,
    ) -> Result<String, ToolError> {
        let channel = self.authorize(channel)?;
        if content.trim().is_empty() || content.chars().count() > 2000 {
            return Err(ToolError::Invalid(
                "content must contain 1 to 2000 characters",
            ));
        }
        let mut builder = CreateMessage::new().content(content).allowed_mentions(
            CreateAllowedMentions::new()
                .all_users(false)
                .all_roles(false)
                .everyone(false)
                .replied_user(false),
        );
        if let Some(reply) = reply {
            builder = builder.reference_message((channel, MessageId::new(id(reply)?)));
        }
        let message = request(channel.send_message(&self.http, builder)).await?;
        Ok(message.id.to_string())
    }

    pub async fn react(&self, channel: &str, message: &str, emoji: &str) -> Result<(), ToolError> {
        let channel = self.authorize(channel)?;
        let message = MessageId::new(id(message)?);
        if emoji.trim().is_empty() || emoji.chars().count() > 100 {
            return Err(ToolError::Invalid(
                "emoji must be Unicode or <:name:id> / <a:name:id>",
            ));
        }
        let reaction: ReactionType = emoji
            .parse()
            .map_err(|_| ToolError::Invalid("Invalid emoji"))?;
        request(channel.create_reaction(&self.http, message, reaction)).await
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use super::*;
    use crate::test_support::{discord, message, mock_server};

    #[tokio::test]
    async fn rejects_unauthorized_channels_and_invalid_input_before_rest() -> anyhow::Result<()> {
        let server = mock_server(vec![]).await?;
        let discord = discord(&server)?;
        assert!(matches!(
            discord.message("999", "1").await,
            Err(ToolError::ForbiddenChannel)
        ));
        assert!(matches!(
            discord.send("999", None, "hello").await,
            Err(ToolError::ForbiddenChannel)
        ));
        assert!(matches!(
            discord.react("999", "1", "👍").await,
            Err(ToolError::ForbiddenChannel)
        ));
        assert!(matches!(
            discord.messages("999", None, None, None).await,
            Err(ToolError::ForbiddenChannel)
        ));
        assert!(matches!(
            discord.messages("123", Some("1"), Some("2"), None).await,
            Err(ToolError::Invalid(_))
        ));
        assert!(matches!(
            discord.messages("123", None, None, Some(101)).await,
            Err(ToolError::Invalid(_))
        ));
        assert!(matches!(
            discord.messages("123", None, None, Some(0)).await,
            Err(ToolError::Invalid(_))
        ));
        assert!(matches!(
            discord.send("123", None, &"あ".repeat(2001)).await,
            Err(ToolError::Invalid(_))
        ));
        assert!(matches!(
            discord.send("123", Some("0"), "hello").await,
            Err(ToolError::Invalid(_))
        ));
        assert!(server.requests.lock().await.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn paginates_and_serializes_large_ids_without_urls() -> anyhow::Result<()> {
        let mut first = message("10000000000000000001");
        first["attachments"] = json!([{"id":"10000000000000000002", "filename":"receipt.png", "size":8,
            "url":"https://cdn.discordapp.com/attachments/secret", "proxy_url":"https://media.discordapp.net/attachments/secret",
            "content_type":"image/png"}]);
        let server = mock_server(vec![
            (
                StatusCode::OK,
                json!([message("10000000000000000000"), first]),
            ),
            (StatusCode::OK, json!([])),
            (StatusCode::OK, json!([])),
        ])
        .await?;
        let discord = discord(&server)?;
        let page = discord
            .messages("123", Some("10000000000000000003"), None, Some(2))
            .await?;
        assert_eq!(page.before.as_deref(), Some("10000000000000000000"));
        assert_eq!(page.after.as_deref(), Some("10000000000000000001"));
        assert!(page.has_more);
        let value = serde_json::to_value(page)?;
        assert_eq!(value["messages"][0]["author_id"], "18446744073709551615");
        assert_eq!(
            value["messages"][0]["attachments"][0]["attachment_id"],
            "10000000000000000002"
        );
        assert!(!value.to_string().contains("https://"));
        let empty = discord
            .messages("123", None, Some("10000000000000000001"), None)
            .await?;
        assert!(!empty.has_more);
        assert!(empty.before.is_none() && empty.after.is_none());
        discord.messages("123", None, None, None).await?;
        let requests = server.requests.lock().await;
        assert!(requests[0].uri.contains("before=10000000000000000003"));
        assert!(requests[0].uri.contains("limit=2"));
        assert!(requests[1].uri.contains("after=10000000000000000001"));
        assert!(requests[2].uri.contains("limit=20"));
        Ok(())
    }

    #[tokio::test]
    async fn sends_replies_without_mentions_and_adds_reactions() -> anyhow::Result<()> {
        let server = mock_server(vec![
            (StatusCode::OK, message("10")),
            (StatusCode::OK, message("11")),
            (StatusCode::NO_CONTENT, Value::Null),
        ])
        .await?;
        let discord = discord(&server)?;
        assert_eq!(discord.send("123", None, "@everyone hello").await?, "10");
        assert_eq!(discord.send("123", Some("9"), "<@456> hello").await?, "11");
        discord.react("123", "9", "<:test:789>").await?;
        let requests = server.requests.lock().await;
        for request in &requests[..2] {
            assert_eq!(request.method, "POST");
            assert_eq!(request.body["allowed_mentions"]["parse"], json!([]));
            assert_eq!(request.body["allowed_mentions"]["replied_user"], false);
        }
        assert_eq!(requests[1].body["message_reference"]["message_id"], "9");
        assert_eq!(requests[1].body["message_reference"]["channel_id"], "123");
        assert_eq!(requests[2].method, "PUT");
        assert!(requests[2].uri.contains("/messages/9/reactions/"));
        Ok(())
    }

    #[tokio::test]
    async fn classifies_discord_errors_without_retrying_writes() -> anyhow::Result<()> {
        for (status, code) in [
            (StatusCode::NOT_FOUND, "not_found"),
            (StatusCode::UNAUTHORIZED, "discord_unauthorized"),
            (StatusCode::FORBIDDEN, "permission_denied"),
            (StatusCode::INTERNAL_SERVER_ERROR, "discord_error"),
        ] {
            let server = mock_server(vec![(
                status,
                json!({"code":0,"message":"private response"}),
            )])
            .await?;
            let discord = discord(&server)?;
            let result = discord.send("123", None, "hello").await;
            assert_eq!(result.as_ref().err().map(ToolError::code), Some(code));
            assert!(!format!("{result:?}").contains("private response"));
            assert_eq!(server.requests.lock().await.len(), 1);
        }
        Ok(())
    }

    #[tokio::test]
    async fn validates_server_channel_types() -> anyhow::Result<()> {
        for (kind, accepted) in [
            (0, true),
            (5, true),
            (1, false),
            (2, false),
            (4, false),
            (11, false),
            (12, false),
            (15, false),
        ] {
            let channel = if kind == 1 {
                json!({"id":"123", "type":kind, "recipients":[]})
            } else {
                json!({"id":"123", "type":kind, "guild_id":"456", "name":"test", "position":0, "permission_overwrites":[]})
            };
            let server = mock_server(vec![(StatusCode::OK, channel)]).await?;
            let discord = discord(&server)?;
            assert_eq!(
                discord.validate_channels().await.is_ok(),
                accepted,
                "channel kind {kind}"
            );
        }
        Ok(())
    }

    use serde_json::Value;
}
