use std::{collections::HashSet, sync::Arc};

use serenity::{
    all::{Context, EventHandler, Message, Ready, ResumedEvent, ShardStageUpdateEvent},
    async_trait,
};

pub struct Gateway {
    pub channels: Arc<HashSet<u64>>,
}

#[async_trait]
impl EventHandler for Gateway {
    async fn ready(&self, _context: Context, _ready: Ready) {
        tracing::info!("Discord Gateway ready");
    }

    async fn resume(&self, _context: Context, _event: ResumedEvent) {
        tracing::info!("Discord Gateway resumed");
    }

    async fn shard_stage_update(&self, _context: Context, event: ShardStageUpdateEvent) {
        tracing::info!(stage = ?event.new, "Discord Gateway connection changed");
    }

    async fn message(&self, _context: Context, message: Message) {
        // 許可リストは起動時に通常のサーバーチャンネルだけに検証済み。
        if message.guild_id.is_some() && self.channels.contains(&message.channel_id.get()) {
            tracing::info!(channel_id = %message.channel_id, message_id = %message.id, "Discord message received");
        }
    }
}
