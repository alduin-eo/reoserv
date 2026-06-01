mod bot;

use std::sync::OnceLock;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::{db::DbHandle, world::WorldHandle};

pub use bot::spawn_bot;

#[derive(Debug, Clone)]
pub enum DiscordCommand {
    NewTransaction {
        tx_id: i32,
        character_name: String,
        action: i32,
        amount: i32,
        wallet_address: String,
    },
    TransactionCancelled {
        tx_id: i32,
    },
}

pub type DiscordTx = UnboundedSender<DiscordCommand>;

static DISCORD_TX: OnceLock<DiscordTx> = OnceLock::new();

pub fn get_discord_tx() -> Option<&'static DiscordTx> {
    DISCORD_TX.get()
}

pub fn init_channel() -> (DiscordTx, UnboundedReceiver<DiscordCommand>) {
    let (tx, rx) = unbounded_channel();
    let _ = DISCORD_TX.set(tx.clone());
    (tx, rx)
}

pub async fn start(rx: UnboundedReceiver<DiscordCommand>, db: DbHandle, world: WorldHandle) {
    spawn_bot(rx, db, world).await;
}
