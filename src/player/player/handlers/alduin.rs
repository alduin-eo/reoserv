use eolib::{
    data::{EoReader, EoSerialize},
    protocol::net::{
        PacketAction, PacketFamily, TransactionAction,
        client::{
            AlduinAddClientPacket, AlduinRemoveClientPacket, AlduinRequestClientPacket,
            AlduinSpecClientPacket,
        },
    },
};

use crate::{
    SETTINGS,
    db::{
        count_alduin_inventory, count_character_transactions, count_pending_withdrawals,
        create_transaction, get_character_transactions_paginated, get_pending_transaction,
        get_transaction_by_id_for_character, resolve_transaction,
    },
    utils::{
        send_alduin_cancel_webhook, send_alduin_deposit_webhook, send_alduin_withdraw_webhook,
    },
};

use eolib::protocol::net::server::{
    AlduinReply, AlduinReplyServerPacket, AlduinReplyServerPacketReplyData,
    AlduinReplyServerPacketReplyDataNotify, AlduinReplyServerPacketReplyDataWallet,
    TransactionEntry, TransactionStatus,
};

use super::super::Player;

const TRANSACTIONS_PER_PAGE: i32 = 20;

impl Player {
    async fn alduin_request(&mut self, reader: EoReader) {
        let request = match AlduinRequestClientPacket::deserialize(&reader) {
            Ok(request) => request,
            Err(e) => {
                error!("Failed to deserialize AlduinRequestClientPacket: {}", e);
                return;
            }
        };

        let character_id = match self.character_id {
            Some(id) => id,
            None => {
                error!("Alduin::Request received but player has no character_id");
                return;
            }
        };

        // Get wallet balance from inventory (item ID 491)
        let balance = match count_alduin_inventory(&self.db, character_id).await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to count alduin inventory: {}", e);
                return;
            }
        };

        // Count pending outgoing (withdrawal) transactions
        let pending_count = match count_pending_withdrawals(&self.db, character_id).await {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to count pending withdrawals: {}", e);
                return;
            }
        };

        // Get deposit wallet address from config
        let deposit_wallet = SETTINGS.alduin.deposit_wallet.clone();

        // Paginated transaction history
        let page = if request.page >= 1 { request.page } else { 1 };

        let (transactions, total_pages) = if page >= 1 {
            let total_count = match count_character_transactions(&self.db, character_id).await {
                Ok(c) => c,
                Err(e) => {
                    error!("Failed to count character transactions: {}", e);
                    return;
                }
            };

            let total_pages = if total_count > 0 {
                (total_count + TRANSACTIONS_PER_PAGE - 1) / TRANSACTIONS_PER_PAGE
            } else {
                1
            };

            let rows = match get_character_transactions_paginated(
                &self.db,
                character_id,
                page,
                TRANSACTIONS_PER_PAGE,
            )
            .await
            {
                Ok(rows) => rows,
                Err(e) => {
                    error!("Failed to fetch transaction history: {}", e);
                    return;
                }
            };

            let transactions: Vec<TransactionEntry> = rows
                .into_iter()
                .filter_map(|row| {
                    let id = row.get_int(0).unwrap_or(0);
                    let created_at = row.get_int(7).unwrap_or(0) as i32;
                    let action_str = row.get_string(2)?;
                    let action = match action_str.as_str() {
                        "deposit" => TransactionAction::Deposit,
                        "withdraw" => TransactionAction::Withdraw,
                        _ => TransactionAction::Unrecognized(0),
                    };
                    let amount = row.get_int(3).unwrap_or(0);
                    let wallet_address = row.get_string(5).unwrap_or_default();
                    let status_str = row.get_string(6)?;
                    let status = match status_str.as_str() {
                        "pending" => TransactionStatus::Pending,
                        "approved" => TransactionStatus::Approved,
                        "cancelled" => TransactionStatus::Cancelled,
                        _ => TransactionStatus::Unrecognized(0),
                    };

                    Some(TransactionEntry {
                        id,
                        timestamp: created_at,
                        action,
                        amount,
                        wallet_address,
                        status,
                    })
                })
                .collect();

            (transactions, total_pages)
        } else {
            (Vec::new(), 1)
        };

        let packet = AlduinReplyServerPacket {
            reply: AlduinReply::Wallet,
            reply_data: Some(AlduinReplyServerPacketReplyData::Wallet(
                AlduinReplyServerPacketReplyDataWallet {
                    balance,
                    pending_count,
                    deposit_wallet,
                    deposit_min: SETTINGS.alduin.deposit_min,
                    deposit_max: SETTINGS.alduin.deposit_max,
                    withdraw_min: SETTINGS.alduin.withdraw_min,
                    withdraw_max: SETTINGS.alduin.withdraw_max,
                    page,
                    total_pages,
                    transactions,
                },
            )),
        };

        let _ = self
            .bus
            .send(PacketAction::Reply, PacketFamily::Alduin, packet)
            .await;
    }

    pub async fn handle_alduin(&mut self, action: PacketAction, reader: EoReader) {
        // Shared rate limit: 5-second cooldown across all Alduin requests
        if let Some(last) = self.last_alduin_request {
            if last.elapsed().as_secs() < 5 {
                return; // Silently ignore
            }
        }
        self.last_alduin_request = Some(std::time::Instant::now());

        match action {
            PacketAction::Request => self.alduin_request(reader).await,
            PacketAction::Add => self.alduin_add(reader).await,
            PacketAction::Remove => self.alduin_remove(reader).await,
            PacketAction::Spec => self.alduin_cancel(reader).await,
            _ => error!("Unhandled packet Alduin_{:?}", action),
        }
    }

    async fn alduin_add(&mut self, reader: EoReader) {
        let add = match AlduinAddClientPacket::deserialize(&reader) {
            Ok(add) => add,
            Err(e) => {
                error!("Failed to deserialize AlduinAddClientPacket: {}", e);
                return;
            }
        };

        let character_id = match self.character_id {
            Some(id) => id,
            None => {
                error!("Alduin::Add received but player has no character_id");
                return;
            }
        };

        let character_name = match self.character_name.as_ref() {
            Some(name) => name,
            None => {
                error!("Alduin::Add received but player has no character_name");
                return;
            }
        };

        // Validate amount >= deposit_min
        if add.amount < SETTINGS.alduin.deposit_min {
            let packet = AlduinReplyServerPacket {
                reply: AlduinReply::AmountBelowMin,
                reply_data: None,
            };
            let _ = self
                .bus
                .send(PacketAction::Reply, PacketFamily::Alduin, packet)
                .await;
            return;
        }

        // Validate amount <= deposit_max
        if add.amount > SETTINGS.alduin.deposit_max {
            let packet = AlduinReplyServerPacket {
                reply: AlduinReply::AmountAboveMax,
                reply_data: None,
            };
            let _ = self
                .bus
                .send(PacketAction::Reply, PacketFamily::Alduin, packet)
                .await;
            return;
        }

        // Validate wallet address is non-empty
        if add.wallet_address.is_empty() {
            let packet = AlduinReplyServerPacket {
                reply: AlduinReply::InvalidWalletAddress,
                reply_data: None,
            };
            let _ = self
                .bus
                .send(PacketAction::Reply, PacketFamily::Alduin, packet)
                .await;
            return;
        }

        // Check for existing pending deposit
        let existing = match get_pending_transaction(&self.db, character_id, "deposit").await {
            Ok(row) => row,
            Err(e) => {
                error!("Failed to check for pending deposit: {}", e);
                return;
            }
        };
        if existing.is_some() {
            let packet = AlduinReplyServerPacket {
                reply: AlduinReply::AlreadyHasPending,
                reply_data: None,
            };
            let _ = self
                .bus
                .send(PacketAction::Reply, PacketFamily::Alduin, packet)
                .await;
            return;
        }

        // Create pending transaction record
        let transaction_id = match create_transaction(
            &self.db,
            character_id,
            "deposit",
            add.amount,
            &add.wallet_address,
        )
        .await
        {
            Ok(Some(id)) => id,
            Ok(None) => {
                error!("Failed to create deposit transaction: no insert ID returned");
                return;
            }
            Err(e) => {
                error!("Failed to create deposit transaction: {}", e);
                return;
            }
        };

        // Get current alduin balance for the Notify reply
        let total_alduin = match count_alduin_inventory(&self.db, character_id).await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to count alduin inventory: {}", e);
                return;
            }
        };

        // Send Notify reply
        let packet = AlduinReplyServerPacket {
            reply: AlduinReply::Notify,
            reply_data: Some(AlduinReplyServerPacketReplyData::Notify(
                AlduinReplyServerPacketReplyDataNotify {
                    transaction_id: transaction_id as i32,
                    status: TransactionStatus::Pending,
                    total_alduin,
                },
            )),
        };
        let _ = self
            .bus
            .send(PacketAction::Reply, PacketFamily::Alduin, packet)
            .await;

        // Fire Discord webhook asynchronously (don't block)
        let webhook_url = SETTINGS.alduin.discord_webhook.clone();
        let mention = SETTINGS.alduin.discord_mention.clone();
        let char_name = character_name.clone();
        let wallet = add.wallet_address.clone();
        let amount = add.amount;
        tokio::spawn(async move {
            send_alduin_deposit_webhook(
                &webhook_url,
                &mention,
                &char_name,
                character_id,
                &wallet,
                amount,
                transaction_id,
            )
            .await;
        });
    }

    async fn alduin_remove(&mut self, reader: EoReader) {
        let remove = match AlduinRemoveClientPacket::deserialize(&reader) {
            Ok(remove) => remove,
            Err(e) => {
                error!("Failed to deserialize AlduinRemoveClientPacket: {}", e);
                return;
            }
        };

        let character_id = match self.character_id {
            Some(id) => id,
            None => {
                error!("Alduin::Remove received but player has no character_id");
                return;
            }
        };

        let character_name = match self.character_name.as_ref() {
            Some(name) => name,
            None => {
                error!("Alduin::Remove received but player has no character_name");
                return;
            }
        };

        // Validate amount >= withdraw_min
        if remove.amount < SETTINGS.alduin.withdraw_min {
            let packet = AlduinReplyServerPacket {
                reply: AlduinReply::AmountBelowMin,
                reply_data: None,
            };
            let _ = self
                .bus
                .send(PacketAction::Reply, PacketFamily::Alduin, packet)
                .await;
            return;
        }

        // Validate amount <= withdraw_max
        if remove.amount > SETTINGS.alduin.withdraw_max {
            let packet = AlduinReplyServerPacket {
                reply: AlduinReply::AmountAboveMax,
                reply_data: None,
            };
            let _ = self
                .bus
                .send(PacketAction::Reply, PacketFamily::Alduin, packet)
                .await;
            return;
        }

        // Check player has enough Alduin items (ID 491) in inventory
        let balance = match count_alduin_inventory(&self.db, character_id).await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to count alduin inventory: {}", e);
                return;
            }
        };
        if balance < remove.amount {
            let packet = AlduinReplyServerPacket {
                reply: AlduinReply::InsufficientFunds,
                reply_data: None,
            };
            let _ = self
                .bus
                .send(PacketAction::Reply, PacketFamily::Alduin, packet)
                .await;
            return;
        }

        // Validate wallet address is non-empty
        if remove.wallet_address.is_empty() {
            let packet = AlduinReplyServerPacket {
                reply: AlduinReply::InvalidWalletAddress,
                reply_data: None,
            };
            let _ = self
                .bus
                .send(PacketAction::Reply, PacketFamily::Alduin, packet)
                .await;
            return;
        }

        // Check for existing pending withdrawal
        let existing = match get_pending_transaction(&self.db, character_id, "withdraw").await {
            Ok(row) => row,
            Err(e) => {
                error!("Failed to check for pending withdrawal: {}", e);
                return;
            }
        };
        if existing.is_some() {
            let packet = AlduinReplyServerPacket {
                reply: AlduinReply::AlreadyHasPending,
                reply_data: None,
            };
            let _ = self
                .bus
                .send(PacketAction::Reply, PacketFamily::Alduin, packet)
                .await;
            return;
        }

        // Remove Alduin items from inventory IMMEDIATELY
        if let Some(character) = self.character.as_mut() {
            character.remove_item(491, remove.amount);
        } else {
            error!("Alduin::Remove received but player has no character in memory");
            return;
        }

        // Persist inventory changes
        if let Some(character) = self.character.as_mut() {
            if let Err(e) = character.save(&self.db).await {
                error!(
                    "Failed to save character after removing Alduin items: {}",
                    e
                );
                return;
            }
        }

        // Create pending transaction record
        let transaction_id = match create_transaction(
            &self.db,
            character_id,
            "withdraw",
            remove.amount,
            &remove.wallet_address,
        )
        .await
        {
            Ok(Some(id)) => id,
            Ok(None) => {
                error!("Failed to create withdraw transaction: no insert ID returned");
                return;
            }
            Err(e) => {
                error!("Failed to create withdraw transaction: {}", e);
                return;
            }
        };

        // Get current alduin balance for the Notify reply (after removal)
        let total_alduin = match count_alduin_inventory(&self.db, character_id).await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to count alduin inventory: {}", e);
                return;
            }
        };

        // Send Notify reply
        let packet = AlduinReplyServerPacket {
            reply: AlduinReply::Notify,
            reply_data: Some(AlduinReplyServerPacketReplyData::Notify(
                AlduinReplyServerPacketReplyDataNotify {
                    transaction_id: transaction_id as i32,
                    status: TransactionStatus::Pending,
                    total_alduin,
                },
            )),
        };
        let _ = self
            .bus
            .send(PacketAction::Reply, PacketFamily::Alduin, packet)
            .await;

        // Fire Discord webhook asynchronously (don't block)
        let webhook_url = SETTINGS.alduin.discord_webhook.clone();
        let mention = SETTINGS.alduin.discord_mention.clone();
        let char_name = character_name.clone();
        let wallet = remove.wallet_address.clone();
        let amount = remove.amount;
        tokio::spawn(async move {
            send_alduin_withdraw_webhook(
                &webhook_url,
                &mention,
                &char_name,
                character_id,
                &wallet,
                amount,
                transaction_id,
            )
            .await;
        });
    }

    async fn alduin_cancel(&mut self, reader: EoReader) {
        let spec = match AlduinSpecClientPacket::deserialize(&reader) {
            Ok(spec) => spec,
            Err(e) => {
                error!("Failed to deserialize AlduinSpecClientPacket: {}", e);
                return;
            }
        };

        let character_id = match self.character_id {
            Some(id) => id,
            None => {
                error!("Alduin::Spec received but player has no character_id");
                return;
            }
        };

        let character_name = match self.character_name.as_ref() {
            Some(name) => name,
            None => {
                error!("Alduin::Spec received but player has no character_name");
                return;
            }
        };

        let transaction_id = spec.transaction_id as i64;

        // Validate: transaction exists and belongs to this character
        let row = match get_transaction_by_id_for_character(&self.db, transaction_id, character_id)
            .await
        {
            Ok(Some(row)) => row,
            Ok(None) => {
                // Transaction not found or doesn't belong to this character
                let packet = AlduinReplyServerPacket {
                    reply: AlduinReply::TransactionNotFound,
                    reply_data: None,
                };
                let _ = self
                    .bus
                    .send(PacketAction::Reply, PacketFamily::Alduin, packet)
                    .await;
                return;
            }
            Err(e) => {
                error!("Failed to look up transaction for cancel: {}", e);
                return;
            }
        };

        // Validate: transaction is in pending status
        let status = match row.get_string(6) {
            Some(s) => s,
            None => {
                error!("Transaction row missing status field");
                return;
            }
        };
        if status != "pending" {
            let packet = AlduinReplyServerPacket {
                reply: AlduinReply::TransactionNotPending,
                reply_data: None,
            };
            let _ = self
                .bus
                .send(PacketAction::Reply, PacketFamily::Alduin, packet)
                .await;
            return;
        }

        // Get the action type and amount
        let action = match row.get_string(2) {
            Some(a) => a,
            None => {
                error!("Transaction row missing action field");
                return;
            }
        };
        let amount = match row.get_int(3) {
            Some(a) => a,
            None => {
                error!("Transaction row missing amount field");
                return;
            }
        };

        // If withdraw: refund Alduin items (ID 491) back to inventory
        if action == "withdraw" {
            if let Some(character) = self.character.as_mut() {
                character.add_item(491, amount);
            } else {
                error!("Alduin::Spec received but player has no character in memory");
                return;
            }

            // Persist inventory changes
            if let Some(character) = self.character.as_mut() {
                if let Err(e) = character.save(&self.db).await {
                    error!(
                        "Failed to save character after refunding Alduin items: {}",
                        e
                    );
                    return;
                }
            }
        }

        // Mark transaction as cancelled
        if let Err(e) =
            resolve_transaction(&self.db, transaction_id, "cancelled", character_id).await
        {
            error!("Failed to resolve transaction as cancelled: {}", e);
            return;
        }

        // Get current alduin balance for the Notify reply
        let total_alduin = match count_alduin_inventory(&self.db, character_id).await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to count alduin inventory: {}", e);
                return;
            }
        };

        // Send Notify reply
        let packet = AlduinReplyServerPacket {
            reply: AlduinReply::Notify,
            reply_data: Some(AlduinReplyServerPacketReplyData::Notify(
                AlduinReplyServerPacketReplyDataNotify {
                    transaction_id: transaction_id as i32,
                    status: TransactionStatus::Cancelled,
                    total_alduin,
                },
            )),
        };
        let _ = self
            .bus
            .send(PacketAction::Reply, PacketFamily::Alduin, packet)
            .await;

        // Fire Discord webhook asynchronously (don't block)
        let webhook_url = SETTINGS.alduin.discord_webhook.clone();
        let mention = SETTINGS.alduin.discord_mention.clone();
        let char_name = character_name.clone();
        let txn_id = transaction_id;
        let action_str = action.clone();
        tokio::spawn(async move {
            send_alduin_cancel_webhook(
                &webhook_url,
                &mention,
                &char_name,
                character_id,
                amount,
                &action_str,
                txn_id,
            )
            .await;
        });
    }
}
