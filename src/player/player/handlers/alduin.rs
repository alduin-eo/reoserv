use std::cmp;

use eolib::{
    data::{EoReader, EoSerialize},
    protocol::net::{
        PacketAction, PacketFamily, TransactionAction,
        client::AlduinRequestClientPacket,
        server::{
            AlduinReply, AlduinReplyServerPacket, AlduinReplyServerPacketReplyData,
            AlduinReplyServerPacketReplyDataWallet, TransactionEntry, TransactionStatus,
        },
    },
};

use crate::{SETTINGS, db::insert_params};

use super::super::Player;

impl Player {
    fn alduin_request(&mut self, reader: EoReader) {
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
                Some(player) => player,
                None => return,
            };

            let config = SETTINGS.load().alduin.clone();
            let per_page = cmp::max(config.transactions_per_page, 1);

            let balance = character.get_item_amount(config.alduin_item_id);
            let character_id = character.id;

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
        });
    }

    pub fn handle_alduin(&mut self, action: PacketAction, reader: EoReader) {
        if self.trading {
            return;
        }

        match action {
            PacketAction::Request => self.alduin_request(reader),
            _ => tracing::error!("Unhandled packet Alduin_{:?}", action),
        }
    }
}
