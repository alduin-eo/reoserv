/// Sends a Discord webhook notification for an Alduin withdrawal request.
/// This function is intended to be spawned with `tokio::spawn` so it doesn't block the handler.
pub async fn send_alduin_withdraw_webhook(
    webhook_url: &str,
    mention: &str,
    character_name: &str,
    character_id: i32,
    wallet_address: &str,
    amount: i32,
    transaction_id: i64,
) {
    let payload = serde_json::json!({
        "content": format!("{} 💸 **Alduin Withdrawal Request**", mention),
        "embeds": [{
            "title": format!("{} requests withdrawal of {} Alduin", character_name, amount),
            "fields": [
                {"name": "Character", "value": format!("{} (ID: {})", character_name, character_id)},
                {"name": "Wallet", "value": wallet_address.to_string()},
                {"name": "Amount", "value": amount.to_string()},
                {"name": "Type", "value": "Withdraw"},
                {"name": "Transaction ID", "value": transaction_id.to_string()}
            ],
            "color": 16744192
        }]
    });

    let body = match serde_json::to_string(&payload) {
        Ok(b) => b,
        Err(e) => {
            warn!("Failed to serialize Discord webhook payload: {}", e);
            return;
        }
    };

    let client = reqwest::Client::new();
    match client
        .post(webhook_url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                warn!(
                    "Discord webhook for withdraw transaction {} returned status: {}",
                    transaction_id,
                    resp.status()
                );
            }
        }
        Err(e) => {
            warn!(
                "Failed to send Discord webhook for withdraw transaction {}: {}",
                transaction_id, e
            );
        }
    }
}

/// Sends a Discord webhook notification for an Alduin deposit request.
/// This function is intended to be spawned with `tokio::spawn` so it doesn't block the handler.
pub async fn send_alduin_deposit_webhook(
    webhook_url: &str,
    mention: &str,
    character_name: &str,
    character_id: i32,
    wallet_address: &str,
    amount: i32,
    transaction_id: i64,
) {
    let payload = serde_json::json!({
        "content": format!("{} 🏦 **Alduin Deposit Request**", mention),
        "embeds": [{
            "title": format!("{} requests deposit of {} Alduin", character_name, amount),
            "fields": [
                {"name": "Character", "value": format!("{} (ID: {})", character_name, character_id)},
                {"name": "Wallet", "value": wallet_address.to_string()},
                {"name": "Amount", "value": amount.to_string()},
                {"name": "Type", "value": "Deposit"},
                {"name": "Transaction ID", "value": transaction_id.to_string()}
            ],
            "color": 5814783
        }]
    });

    let body = match serde_json::to_string(&payload) {
        Ok(b) => b,
        Err(e) => {
            warn!("Failed to serialize Discord webhook payload: {}", e);
            return;
        }
    };

    let client = reqwest::Client::new();
    match client
        .post(webhook_url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                warn!(
                    "Discord webhook for deposit transaction {} returned status: {}",
                    transaction_id,
                    resp.status()
                );
            }
        }
        Err(e) => {
            warn!(
                "Failed to send Discord webhook for deposit transaction {}: {}",
                transaction_id, e
            );
        }
    }
}

/// Sends a Discord webhook notification for an Alduin transaction cancellation.
/// This function is intended to be spawned with `tokio::spawn` so it doesn't block the handler.
pub async fn send_alduin_cancel_webhook(
    webhook_url: &str,
    mention: &str,
    character_name: &str,
    character_id: i32,
    amount: i32,
    action: &str,
    transaction_id: i64,
) {
    let action_label = if action == "deposit" {
        "Deposit"
    } else {
        "Withdraw"
    };
    let payload = serde_json::json!({
        "content": format!("{} 🔕 **Alduin Transaction Cancelled**", mention),
        "embeds": [{
            "title": "Player cancelled their own pending transaction",
            "fields": [
                {"name": "Character", "value": format!("{} (ID: {})", character_name, character_id)},
                {"name": "Amount", "value": amount.to_string()},
                {"name": "Type", "value": action_label},
                {"name": "Transaction ID", "value": transaction_id.to_string()}
            ],
            "color": 16744192
        }]
    });

    let body = match serde_json::to_string(&payload) {
        Ok(b) => b,
        Err(e) => {
            warn!("Failed to serialize Discord webhook payload: {}", e);
            return;
        }
    };

    let client = reqwest::Client::new();
    match client
        .post(webhook_url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                warn!(
                    "Discord webhook for cancel transaction {} returned status: {}",
                    transaction_id,
                    resp.status()
                );
            }
        }
        Err(e) => {
            warn!(
                "Failed to send Discord webhook for cancel transaction {}: {}",
                transaction_id, e
            );
        }
    }
}
