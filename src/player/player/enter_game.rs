use eolib::protocol::net::{
    PacketAction, PacketFamily,
    server::{
        AlduinReply, AlduinReplyServerPacket, AlduinReplyServerPacketReplyData,
        AlduinReplyServerPacketReplyDataNotify, SitState, TransactionStatus, WelcomeCode,
        WelcomeReplyServerPacket, WelcomeReplyServerPacketWelcomeCodeData,
        WelcomeReplyServerPacketWelcomeCodeDataEnterGame,
    },
};
use std::{io::Cursor, path::Path};
use tokio::io::{AsyncBufReadExt, AsyncReadExt};

use crate::{
    SETTINGS, db::insert_params, errors::WrongSessionIdError, player::ClientState, utils::is_deep,
};

use super::Player;

impl Player {
    pub async fn enter_game(&mut self, session_id: i32) -> bool {
        let actual_session_id = match self.take_session_id() {
            Ok(session_id) => session_id,
            Err(e) => {
                self.close(format!("Error getting session id: {}", e)).await;
                return false;
            }
        };

        if actual_session_id != session_id {
            self.close(format!(
                "{}",
                WrongSessionIdError::new(actual_session_id, session_id)
            ))
            .await;
            return false;
        }

        let mut character = match self.character.take() {
            Some(character) => character,
            None => {
                self.close("Player attempted to enter game with no character selected".to_string())
                    .await;
                return false;
            }
        };

        let map = match self.world.get_map(character.map_id).await {
            Ok(map) => map,
            Err(e) => {
                self.close(format!("Error getting map: {}", e)).await;
                return false;
            }
        };

        self.map = Some(map.clone());
        self.state = ClientState::InGame;

        let items = character.items.clone();
        let spells = character.spells.clone();
        let weight = character.get_weight();

        if let Ok(Some(relog_coords)) = map.get_relog_coords().await {
            character.coords = relog_coords;
            character.sit_state = SitState::Stand;
        }

        character.is_deep = is_deep(&self.version);

        map.enter(Box::new(character), None)
            .await
            .expect("Failed to enter map. Timeout");

        if let Some(character_id) = self.character_id {
            let alduin_item_id = SETTINGS.load().alduin.alduin_item_id;
            if alduin_item_id > 0 {
                let current_balance = self
                    .db
                    .query_int(&insert_params(
                        "SELECT quantity FROM character_inventory \
                         WHERE character_id = :cid AND item_id = :iid",
                        &[("cid", &character_id), ("iid", &alduin_item_id)],
                    ))
                    .await
                    .unwrap_or(None)
                    .unwrap_or(0);

                let rows = self
                    .db
                    .query(&insert_params(
                        "SELECT id, action_id, amount, status_id \
                         FROM character_transaction \
                         WHERE character_id = :cid AND notified = 0 AND status_id IN (1, 2)",
                        &[("cid", &character_id)],
                    ))
                    .await
                    .unwrap_or_default();

                for row in &rows {
                    let tx_id = row.get_int(0).unwrap_or(0);
                    let _action_id = row.get_int(1).unwrap_or(0);
                    let amount = row.get_int(2).unwrap_or(0);
                    let status_id = row.get_int(3).unwrap_or(0);

                    let notify_packet = AlduinReplyServerPacket {
                        reply: AlduinReply::Notify,
                        reply_data: Some(AlduinReplyServerPacketReplyData::Notify(
                            AlduinReplyServerPacketReplyDataNotify {
                                transaction_id: tx_id,
                                status: TransactionStatus::from(status_id),
                                transaction_amount: amount,
                                total_alduin: current_balance,
                            },
                        )),
                    };

                    let _ = self
                        .bus
                        .send(PacketAction::Reply, PacketFamily::Alduin, notify_packet)
                        .await;

                    let _ = self
                        .db
                        .execute(&insert_params(
                            "UPDATE character_transaction SET notified = 1 WHERE id = :id",
                            &[("id", &tx_id)],
                        ))
                        .await;
                }
            }
        }

        let nearby_info = map
            .get_nearby_info(self.id)
            .await
            .expect("Failed to get nearby info. Timeout");

        let _ = self
            .bus
            .send(
                PacketAction::Reply,
                PacketFamily::Welcome,
                WelcomeReplyServerPacket {
                    welcome_code: WelcomeCode::EnterGame,
                    welcome_code_data: Some(WelcomeReplyServerPacketWelcomeCodeData::EnterGame(
                        WelcomeReplyServerPacketWelcomeCodeDataEnterGame {
                            news: get_news().await,
                            weight,
                            items,
                            spells,
                            nearby: nearby_info,
                        },
                    )),
                },
            )
            .await;
        true
    }
}

async fn get_news() -> [String; 9] {
    match tokio::fs::File::open(Path::new("data/news.txt")).await {
        Ok(mut file) => {
            let mut buf: Vec<u8> = Vec::new();
            file.read_to_end(&mut buf).await.unwrap();
            let cursor = Cursor::new(buf);
            let mut lines = cursor.lines();
            let mut news = [
                String::default(),
                String::default(),
                String::default(),
                String::default(),
                String::default(),
                String::default(),
                String::default(),
                String::default(),
                String::default(),
            ];
            for entry in &mut news {
                if let Ok(Some(line)) = lines.next_line().await {
                    *entry = line
                }
            }
            news
        }
        Err(e) => {
            tracing::error!("Failed to open data/news.txt: {}", e);
            [
                String::default(),
                String::default(),
                String::default(),
                String::default(),
                String::default(),
                String::default(),
                String::default(),
                String::default(),
                String::default(),
            ]
        }
    }
}
