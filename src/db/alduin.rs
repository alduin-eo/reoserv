use super::{DbHandle, Row, insert_params};

/// Creates a new character transaction and returns its ID.
pub async fn create_transaction(
    db: &DbHandle,
    character_id: i32,
    action: &str,
    amount: i32,
    wallet_address: &str,
) -> anyhow::Result<Option<i64>> {
    let now = chrono::Utc::now().timestamp() as i64;
    db.execute(&insert_params(
        "INSERT INTO `character_transaction` (`character_id`, `action`, `amount`, `settled_amount`, `wallet_address`, `status`, `created_at`, `notified`) \
         VALUES (:character_id, :action, :amount, :settled_amount, :wallet_address, :status, :created_at, :notified)",
        &[
            ("character_id", &character_id),
            ("action", &action),
            ("amount", &amount),
            ("settled_amount", &0i32),
            ("wallet_address", &wallet_address),
            ("status", &"pending"),
            ("created_at", &now),
            ("notified", &false),
        ],
    ))
    .await?;
    Ok(db.get_last_insert_id().await.map(|id| id as i64))
}

/// Finds a pending transaction for the given character and action type.
pub async fn get_pending_transaction(
    db: &DbHandle,
    character_id: i32,
    action: &str,
) -> anyhow::Result<Option<Row>> {
    db.query_one(&insert_params(
        "SELECT `id`, `character_id`, `action`, `amount`, `settled_amount`, `wallet_address`, `status`, `created_at`, `resolved_at`, `resolved_by`, `notified` \
         FROM `character_transaction` \
         WHERE `character_id` = :character_id AND `action` = :action AND `status` = 'pending' \
         ORDER BY `id` ASC LIMIT 1",
        &[
            ("character_id", &character_id),
            ("action", &action),
        ],
    ))
    .await
}

/// Gets a transaction by its ID.
pub async fn get_transaction_by_id(
    db: &DbHandle,
    transaction_id: i64,
) -> anyhow::Result<Option<Row>> {
    db.query_one(&insert_params(
        "SELECT `id`, `character_id`, `action`, `amount`, `settled_amount`, `wallet_address`, `status`, `created_at`, `resolved_at`, `resolved_by`, `notified` \
         FROM `character_transaction` \
         WHERE `id` = :transaction_id",
        &[("transaction_id", &transaction_id)],
    ))
    .await
}

/// Gets a transaction by ID with an ownership check for the given character.
pub async fn get_transaction_by_id_for_character(
    db: &DbHandle,
    transaction_id: i64,
    character_id: i32,
) -> anyhow::Result<Option<Row>> {
    db.query_one(&insert_params(
        "SELECT `id`, `character_id`, `action`, `amount`, `settled_amount`, `wallet_address`, `status`, `created_at`, `resolved_at`, `resolved_by`, `notified` \
         FROM `character_transaction` \
         WHERE `id` = :transaction_id AND `character_id` = :character_id",
        &[
            ("transaction_id", &transaction_id),
            ("character_id", &character_id),
        ],
    ))
    .await
}

/// Resolves a transaction by updating its status, resolved_at, and resolved_by fields.
pub async fn resolve_transaction(
    db: &DbHandle,
    transaction_id: i64,
    status: &str,
    resolved_by_character_id: i32,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp() as i64;
    db.execute(&insert_params(
        "UPDATE `character_transaction` \
         SET `status` = :status, `resolved_at` = :resolved_at, `resolved_by` = :resolved_by \
         WHERE `id` = :transaction_id",
        &[
            ("status", &status),
            ("resolved_at", &now),
            ("resolved_by", &resolved_by_character_id),
            ("transaction_id", &transaction_id),
        ],
    ))
    .await
}

/// Gets the most recent wallet address used by a character in their transactions.
pub async fn get_last_wallet_address(
    db: &DbHandle,
    character_id: i32,
) -> anyhow::Result<Option<String>> {
    db.query_string(&insert_params(
        "SELECT `wallet_address` \
         FROM `character_transaction` \
         WHERE `character_id` = :character_id AND `wallet_address` != '' \
         ORDER BY `id` DESC LIMIT 1",
        &[("character_id", &character_id)],
    ))
    .await
}

/// Counts item ID 491 (Alduin item) in a character's inventory (not bank).
/// The character_inventory table only holds non-bank items; bank items are in character_bank.
pub async fn count_alduin_inventory(db: &DbHandle, character_id: i32) -> anyhow::Result<i32> {
    Ok(db
        .query_int(&insert_params(
            "SELECT COALESCE(SUM(`quantity`), 0) \
             FROM `character_inventory` \
             WHERE `character_id` = :character_id AND `item_id` = 491",
            &[("character_id", &character_id)],
        ))
        .await?
        .unwrap_or(0))
}

/// Gets all resolved (approved or cancelled) transactions that haven't been notified yet
/// for a specific character.
pub async fn get_unnotified_resolved_transactions(
    db: &DbHandle,
    character_id: i32,
) -> anyhow::Result<Vec<Row>> {
    db.query(&insert_params(
        "SELECT `id`, `character_id`, `action`, `amount`, `settled_amount`, `wallet_address`, `status`, `created_at`, `resolved_at`, `resolved_by`, `notified` \
         FROM `character_transaction` \
         WHERE `character_id` = :character_id AND `status` IN ('approved', 'cancelled') AND `notified` = 0",
        &[("character_id", &character_id)],
    ))
    .await
}

/// Marks a transaction as notified.
pub async fn mark_transaction_notified(
    db: &DbHandle,
    transaction_id: i64,
) -> anyhow::Result<()> {
    db.execute(&insert_params(
        "UPDATE `character_transaction` SET `notified` = 1 WHERE `id` = :transaction_id",
        &[("transaction_id", &transaction_id)],
    ))
    .await
}
