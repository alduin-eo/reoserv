use std::cmp;

use eolib::protocol::net::server::{
    AlduinReply, AlduinReplyServerPacket, AlduinReplyServerPacketReplyData,
    AlduinReplyServerPacketReplyDataNotify, TransactionStatus,
};

use crate::{
    SETTINGS,
    character::Character,
    db::{DbHandle, insert_params},
    world::WorldHandle,
};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ResolutionResult {
    pub tx_id: i32,
    pub character_id: i32,
    pub character_name: String,
    pub action: i32,
    pub amount: i32,
    pub old_status: TransactionStatus,
    pub new_status: TransactionStatus,
    pub was_online: bool,
    pub items_modified: bool,
}

pub async fn resolve_transaction(
    db: &DbHandle,
    world: &WorldHandle,
    tx_id: i32,
    new_status: TransactionStatus,
    resolved_by: &str,
) -> anyhow::Result<ResolutionResult> {
    let row = db
        .query_one(&insert_params(
            "SELECT ct.character_id, ct.status_id, ct.action_id, ct.amount, c.name \
             FROM character_transaction ct \
             JOIN characters c ON c.id = ct.character_id \
             WHERE ct.id = :id",
            &[("id", &tx_id)],
        ))
        .await?
        .ok_or_else(|| anyhow::anyhow!("Transaction {} not found", tx_id))?;

    let character_id = row.get_int(0).unwrap_or(0);
    let old_status_id = row.get_int(1).unwrap_or(0);
    let action_id = row.get_int(2).unwrap_or(0);
    let amount = row.get_int(3).unwrap_or(0);
    let character_name = row.get_string(4).unwrap_or_default();

    let old_status = TransactionStatus::from(old_status_id);
    if old_status != TransactionStatus::Pending {
        anyhow::bail!(
            "Transaction {} is not pending (status: {:?})",
            tx_id,
            old_status
        );
    }

    if character_id == 0 || amount <= 0 {
        anyhow::bail!("Invalid transaction data for tx {}", tx_id);
    }

    let alduin_item_id = SETTINGS.load().alduin.alduin_item_id;
    if alduin_item_id <= 0 {
        anyhow::bail!("alduin_item_id is not configured");
    }

    let should_give_items = matches!(
        (action_id, new_status),
        (0, TransactionStatus::Approved) | (1, TransactionStatus::Cancelled)
    );

    let was_online = world.get_character_by_name(&character_name).await.is_ok();

    if should_give_items && alduin_item_id > 0 {
        if was_online {
            let character = world
                .get_character_by_name(&character_name)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            let player_id = character.player_id.unwrap_or(0);
            let map_id = character.map_id;

            if let Ok(map) = world.get_map(map_id).await {
                let max_item = SETTINGS.load().limits.max_item;
                let current = character.get_item_amount(alduin_item_id);
                let capped = cmp::min(max_item - current, amount);
                if capped > 0 {
                    map.give_item(player_id, alduin_item_id, capped);
                }
            }

            let balance = character.get_item_amount(alduin_item_id)
                + if should_give_items { amount } else { 0 };

            let notify_packet = AlduinReplyServerPacket {
                reply: AlduinReply::Notify,
                reply_data: Some(AlduinReplyServerPacketReplyData::Notify(
                    AlduinReplyServerPacketReplyDataNotify {
                        transaction_id: tx_id,
                        status: new_status,
                        transaction_amount: amount,
                        total_alduin: balance,
                    },
                )),
            };

            if let Some(player) = character.player.as_ref() {
                use eolib::protocol::net::{PacketAction, PacketFamily};
                player.send(PacketAction::Reply, PacketFamily::Alduin, &notify_packet);
            }

            let new_status_id: i32 = new_status.into();
            let now = chrono::Utc::now().timestamp() as i32;
            db.execute(&insert_params(
                "UPDATE character_transaction \
                 SET status_id = :status, resolved_at = :now, notified = 1, resolved_by_name = :by \
                 WHERE id = :id",
                &[
                    ("status", &new_status_id),
                    ("now", &now),
                    ("by", &resolved_by),
                    ("id", &tx_id),
                ],
            ))
            .await?;
        } else {
            let mut character = Character::load(db, character_id).await?;

            let max_item = SETTINGS.load().limits.max_item;
            let current = character.get_item_amount(alduin_item_id);
            let capped = cmp::min(max_item - current, amount);
            if capped > 0 {
                character.add_item_no_quest_rules(alduin_item_id, capped);
            }

            character
                .update(db)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;

            let new_status_id: i32 = new_status.into();
            let now = chrono::Utc::now().timestamp() as i32;
            db.execute(&insert_params(
                "UPDATE character_transaction \
                 SET status_id = :status, resolved_at = :now, resolved_by_name = :by \
                 WHERE id = :id",
                &[
                    ("status", &new_status_id),
                    ("now", &now),
                    ("by", &resolved_by),
                    ("id", &tx_id),
                ],
            ))
            .await?;
        }
    } else {
        let new_status_id: i32 = new_status.into();
        let now = chrono::Utc::now().timestamp() as i32;
        if was_online {
            let character = world
                .get_character_by_name(&character_name)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            let balance = character.get_item_amount(alduin_item_id);

            let notify_packet = AlduinReplyServerPacket {
                reply: AlduinReply::Notify,
                reply_data: Some(AlduinReplyServerPacketReplyData::Notify(
                    AlduinReplyServerPacketReplyDataNotify {
                        transaction_id: tx_id,
                        status: new_status,
                        transaction_amount: amount,
                        total_alduin: balance,
                    },
                )),
            };

            if let Some(player) = character.player.as_ref() {
                use eolib::protocol::net::{PacketAction, PacketFamily};
                player.send(PacketAction::Reply, PacketFamily::Alduin, &notify_packet);
            }

            db.execute(&insert_params(
                "UPDATE character_transaction \
                 SET status_id = :status, resolved_at = :now, notified = 1, resolved_by_name = :by \
                 WHERE id = :id",
                &[
                    ("status", &new_status_id),
                    ("now", &now),
                    ("by", &resolved_by),
                    ("id", &tx_id),
                ],
            ))
            .await?;
        } else {
            db.execute(&insert_params(
                "UPDATE character_transaction \
                 SET status_id = :status, resolved_at = :now, resolved_by_name = :by \
                 WHERE id = :id",
                &[
                    ("status", &new_status_id),
                    ("now", &now),
                    ("by", &resolved_by),
                    ("id", &tx_id),
                ],
            ))
            .await?;
        }
    }

    Ok(ResolutionResult {
        tx_id,
        character_id,
        character_name,
        action: action_id,
        amount,
        old_status,
        new_status,
        was_online,
        items_modified: should_give_items,
    })
}
