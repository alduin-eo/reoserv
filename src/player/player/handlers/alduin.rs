use std::cmp;

use eolib::{
    data::{EoReader, EoSerialize},
    protocol::net::{
        PacketAction, PacketFamily, TransactionAction,
        client::{
            AlduinAddClientPacket, AlduinRemoveClientPacket, AlduinRequestClientPacket,
            AlduinSpecClientPacket,
        },
        server::{
            AlduinReply, AlduinReplyServerPacket, AlduinReplyServerPacketReplyData,
            AlduinReplyServerPacketReplyDataWallet, TransactionEntry, TransactionStatus,
        },
    },
};

use crate::{
    SETTINGS,
    db::{DbHandle, insert_params},
    player::PlayerHandle,
};

use super::super::Player;

async fn send_wallet_reply(
    db: DbHandle,
    player: PlayerHandle,
    character_id: i32,
    balance: i32,
    page: i32,
) {
    let config = SETTINGS.load().alduin.clone();
    let per_page = cmp::max(config.transactions_per_page, 1);
    let page = cmp::max(page, 1);

    let total = match db
        .query_one(&insert_params(
            "SELECT COUNT(*) FROM character_transaction WHERE character_id = :character_id",
            &[("character_id", &character_id)],
        ))
        .await
    {
        Ok(Some(row)) => row.get_int(0).unwrap_or(0),
        _ => 0,
    };

    let total_pages = cmp::max(1, (total as f64 / per_page as f64).ceil() as i32);
    let current_page = cmp::min(page, total_pages);
    let offset = (current_page - 1) * per_page;

    let entries = match db
        .query(&insert_params(
            "SELECT id, created_at, action_id, amount, wallet_address, status_id \
             FROM character_transaction WHERE character_id = :character_id \
             ORDER BY created_at DESC LIMIT :limit OFFSET :offset",
            &[
                ("character_id", &character_id),
                ("limit", &per_page),
                ("offset", &offset),
            ],
        ))
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|row| TransactionEntry {
                id: row.get_int(0).unwrap_or(0),
                timestamp: row.get_int(1).unwrap_or(0),
                action: TransactionAction::from(row.get_int(2).unwrap_or(0)),
                amount: row.get_int(3).unwrap_or(0),
                wallet_address: row.get_string(4).unwrap_or_default(),
                status: TransactionStatus::from(row.get_int(5).unwrap_or(0)),
            })
            .collect(),
        Err(_) => Vec::new(),
    };

    let packet = AlduinReplyServerPacket {
        reply: AlduinReply::Wallet,
        reply_data: Some(AlduinReplyServerPacketReplyData::Wallet(
            AlduinReplyServerPacketReplyDataWallet {
                balance,
                deposit_wallet: config.deposit_wallet,
                deposit_min: config.deposit_min,
                deposit_max: config.deposit_max,
                withdraw_min: config.withdraw_min,
                withdraw_max: config.withdraw_max,
                page: current_page,
                total_pages,
                transactions: entries,
            },
        )),
    };

    player.send(PacketAction::Reply, PacketFamily::Alduin, &packet);
}

fn is_valid_solana_address(addr: &str) -> bool {
    if addr.len() < 32 || addr.len() > 44 {
        return false;
    }
    addr.chars()
        .all(|c| c.is_ascii_alphanumeric() && c != '0' && c != 'O' && c != 'I' && c != 'l')
}

impl Player {
    async fn send_reply(&mut self, reply: AlduinReply) {
        let _ = self
            .bus
            .send(
                PacketAction::Reply,
                PacketFamily::Alduin,
                AlduinReplyServerPacket {
                    reply,
                    reply_data: None,
                },
            )
            .await;
    }

    async fn alduin_request(&mut self, reader: EoReader) {
        let packet = match AlduinRequestClientPacket::deserialize(&reader) {
            Ok(packet) => packet,
            Err(e) => {
                tracing::error!("Failed to deserialize AlduinRequestClientPacket: {}", e);
                return;
            }
        };

        let map = match &self.map {
            Some(map) => map.to_owned(),
            None => return,
        };

        let player_id = self.id;
        let page = cmp::max(packet.page, 1);
        let db = self.db.clone();

        tokio::spawn(async move {
            let character = match map
                .get_character(player_id)
                .await
                .expect("Failed to get character. Timeout")
            {
                Some(character) => character,
                None => return,
            };

            let player = match &character.player {
                Some(player) => player.clone(),
                None => return,
            };

            let config = SETTINGS.load().alduin.clone();
            let balance = character.get_item_amount(config.alduin_item_id);

            send_wallet_reply(db, player, character.id, balance, page).await;
        });
    }

    async fn alduin_add(&mut self, reader: EoReader) {
        let packet = match AlduinAddClientPacket::deserialize(&reader) {
            Ok(packet) => packet,
            Err(e) => {
                tracing::error!("Failed to deserialize AlduinAddClientPacket: {}", e);
                return;
            }
        };

        if !is_valid_solana_address(&packet.wallet_address) {
            self.send_reply(AlduinReply::InvalidWalletAddress).await;
            return;
        }

        let config = SETTINGS.load().alduin.clone();
        if packet.amount < config.deposit_min {
            self.send_reply(AlduinReply::AmountBelowMin).await;
            return;
        }
        if packet.amount > config.deposit_max {
            self.send_reply(AlduinReply::AmountAboveMax).await;
            return;
        }

        let map = match &self.map {
            Some(map) => map.to_owned(),
            None => return,
        };

        let player_id = self.id;
        let db = self.db.clone();

        tokio::spawn(async move {
            let character = match map
                .get_character(player_id)
                .await
                .expect("Failed to get character. Timeout")
            {
                Some(character) => character,
                None => return,
            };

            let player = match &character.player {
                Some(player) => player.clone(),
                None => return,
            };

            let current_amount = character.get_item_amount(config.alduin_item_id);
            let max_item = SETTINGS.load().limits.max_item;
            if current_amount + packet.amount > max_item {
                player.send(
                    PacketAction::Reply,
                    PacketFamily::Alduin,
                    &AlduinReplyServerPacket {
                        reply: AlduinReply::InsufficientFunds,
                        reply_data: None,
                    },
                );
                return;
            }

            let status_pending: i32 = TransactionStatus::Pending.into();
            let has_pending = match db
                .query_one(&insert_params(
                    "SELECT COUNT(*) FROM character_transaction \
                     WHERE character_id = :character_id AND status_id = :status_pending",
                    &[
                        ("character_id", &character.id),
                        ("status_pending", &status_pending),
                    ],
                ))
                .await
            {
                Ok(Some(row)) => row.get_int(0).unwrap_or(0) > 0,
                _ => false,
            };

            if has_pending {
                player.send(
                    PacketAction::Reply,
                    PacketFamily::Alduin,
                    &AlduinReplyServerPacket {
                        reply: AlduinReply::AlreadyHasPending,
                        reply_data: None,
                    },
                );
                return;
            }

            let action_id: i32 = TransactionAction::Deposit.into();
            let status_pending: i32 = TransactionStatus::Pending.into();
            let now = chrono::Utc::now().timestamp() as i32;
            if db
                .execute(&insert_params(
                    "INSERT INTO character_transaction \
                     (character_id, action_id, amount, wallet_address, status_id, created_at) \
                     VALUES (:character_id, :action_id, :amount, :wallet_address, :status_pending, :created_at)",
                    &[
                        ("character_id", &character.id),
                        ("action_id", &action_id),
                        ("amount", &packet.amount),
                        ("wallet_address", &packet.wallet_address),
                        ("status_pending", &status_pending),
                        ("created_at", &now),
                    ],
                ))
                .await
                .is_err()
            {
                return;
            }

            send_wallet_reply(db, player, character.id, current_amount, 1).await;
        });
    }

    async fn alduin_remove(&mut self, reader: EoReader) {
        let packet = match AlduinRemoveClientPacket::deserialize(&reader) {
            Ok(packet) => packet,
            Err(e) => {
                tracing::error!("Failed to deserialize AlduinRemoveClientPacket: {}", e);
                return;
            }
        };

        if !is_valid_solana_address(&packet.wallet_address) {
            self.send_reply(AlduinReply::InvalidWalletAddress).await;
            return;
        }

        let config = SETTINGS.load().alduin.clone();
        if packet.amount < config.withdraw_min {
            self.send_reply(AlduinReply::AmountBelowMin).await;
            return;
        }
        if packet.amount > config.withdraw_max {
            self.send_reply(AlduinReply::AmountAboveMax).await;
            return;
        }

        let map = match &self.map {
            Some(map) => map.to_owned(),
            None => return,
        };

        let player_id = self.id;
        let db = self.db.clone();

        tokio::spawn(async move {
            let character = match map
                .get_character(player_id)
                .await
                .expect("Failed to get character. Timeout")
            {
                Some(character) => character,
                None => return,
            };

            let player = match &character.player {
                Some(player) => player.clone(),
                None => return,
            };

            let current_amount = character.get_item_amount(config.alduin_item_id);
            if current_amount < packet.amount {
                player.send(
                    PacketAction::Reply,
                    PacketFamily::Alduin,
                    &AlduinReplyServerPacket {
                        reply: AlduinReply::InsufficientFunds,
                        reply_data: None,
                    },
                );
                return;
            }

            let status_pending: i32 = TransactionStatus::Pending.into();
            let has_pending = match db
                .query_one(&insert_params(
                    "SELECT COUNT(*) FROM character_transaction \
                     WHERE character_id = :character_id AND status_id = :status_pending",
                    &[
                        ("character_id", &character.id),
                        ("status_pending", &status_pending),
                    ],
                ))
                .await
            {
                Ok(Some(row)) => row.get_int(0).unwrap_or(0) > 0,
                _ => false,
            };

            if has_pending {
                player.send(
                    PacketAction::Reply,
                    PacketFamily::Alduin,
                    &AlduinReplyServerPacket {
                        reply: AlduinReply::AlreadyHasPending,
                        reply_data: None,
                    },
                );
                return;
            }

            let action_id: i32 = TransactionAction::Withdraw.into();
            let status_pending: i32 = TransactionStatus::Pending.into();
            let now = chrono::Utc::now().timestamp() as i32;
            if db
                .execute(&insert_params(
                    "INSERT INTO character_transaction \
                     (character_id, action_id, amount, wallet_address, status_id, created_at) \
                     VALUES (:character_id, :action_id, :amount, :wallet_address, :status_pending, :created_at)",
                    &[
                        ("character_id", &character.id),
                        ("action_id", &action_id),
                        ("amount", &packet.amount),
                        ("wallet_address", &packet.wallet_address),
                        ("status_pending", &status_pending),
                        ("created_at", &now),
                    ],
                ))
                .await
                .is_err()
            {
                return;
            }

            map.lose_item(player_id, config.alduin_item_id, packet.amount);

            send_wallet_reply(db, player, character.id, current_amount - packet.amount, 1).await;
        });
    }

    async fn alduin_spec(&mut self, reader: EoReader) {
        let packet = match AlduinSpecClientPacket::deserialize(&reader) {
            Ok(packet) => packet,
            Err(e) => {
                tracing::error!("Failed to deserialize AlduinSpecClientPacket: {}", e);
                return;
            }
        };

        let map = match &self.map {
            Some(map) => map.to_owned(),
            None => return,
        };

        let player_id = self.id;
        let db = self.db.clone();

        tokio::spawn(async move {
            let character = match map
                .get_character(player_id)
                .await
                .expect("Failed to get character. Timeout")
            {
                Some(character) => character,
                None => return,
            };

            let player = match &character.player {
                Some(player) => player.clone(),
                None => return,
            };

            let config = SETTINGS.load().alduin.clone();

            let transaction = match db
                .query_one(&insert_params(
                    "SELECT character_id, status_id, action_id, amount \
                     FROM character_transaction WHERE id = :id",
                    &[("id", &packet.transaction_id)],
                ))
                .await
            {
                Ok(Some(row)) => row,
                _ => {
                    player.send(
                        PacketAction::Reply,
                        PacketFamily::Alduin,
                        &AlduinReplyServerPacket {
                            reply: AlduinReply::TransactionNotFound,
                            reply_data: None,
                        },
                    );
                    return;
                }
            };

            let tx_character_id = transaction.get_int(0).unwrap_or(0);
            if tx_character_id != character.id {
                player.send(
                    PacketAction::Reply,
                    PacketFamily::Alduin,
                    &AlduinReplyServerPacket {
                        reply: AlduinReply::NotYourTransaction,
                        reply_data: None,
                    },
                );
                return;
            }

            let status_id = transaction.get_int(1).unwrap_or(0);
            if TransactionStatus::from(status_id) != TransactionStatus::Pending {
                player.send(
                    PacketAction::Reply,
                    PacketFamily::Alduin,
                    &AlduinReplyServerPacket {
                        reply: AlduinReply::TransactionNotPending,
                        reply_data: None,
                    },
                );
                return;
            }

            let action_id = transaction.get_int(2).unwrap_or(0);
            let tx_amount = transaction.get_int(3).unwrap_or(0);

            let status_cancelled: i32 = TransactionStatus::Cancelled.into();
            let now = chrono::Utc::now().timestamp() as i32;
            if db
                .execute(&insert_params(
                    "UPDATE character_transaction \
                     SET status_id = :status_cancelled, resolved_at = :now, resolved_by = :character_id \
                     WHERE id = :id",
                    &[
                        ("status_cancelled", &status_cancelled),
                        ("now", &now),
                        ("character_id", &character.id),
                        ("id", &packet.transaction_id),
                    ],
                ))
                .await
                .is_err()
            {
                return;
            }

            let current_amount = character.get_item_amount(config.alduin_item_id);
            let new_balance = if TransactionAction::from(action_id) == TransactionAction::Withdraw {
                map.give_item(player_id, config.alduin_item_id, tx_amount);
                current_amount + tx_amount
            } else {
                current_amount
            };

            send_wallet_reply(db, player, character.id, new_balance, 1).await;
        });
    }

    pub async fn handle_alduin(&mut self, action: PacketAction, reader: EoReader) {
        if self.trading {
            return;
        }

        match action {
            PacketAction::Request => self.alduin_request(reader).await,
            PacketAction::Add => self.alduin_add(reader).await,
            PacketAction::Remove => self.alduin_remove(reader).await,
            PacketAction::Spec => self.alduin_spec(reader).await,
            _ => tracing::error!("Unhandled packet Alduin_{:?}", action),
        }
    }
}
