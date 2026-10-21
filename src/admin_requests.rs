// Admin control panel → game server. The website queues rows in `admin_requests`; this task claims
// them one at a time and carries them out with the game's own code, so the server stays authoritative
// for characters, inventories, bans, mutes and warnings:
// - online characters are changed live through their map (the player sees it immediately)
// - offline characters are loaded, changed and saved while their account is locked against login
// Every outcome is written back to the request (status 2 = done, 3 = failed) and, for moderation and
// item changes, to moderation_actions / item_adjustments.
use std::time::Duration;

use chrono::{NaiveDateTime, Utc};
use eolib::protocol::{
    Coords,
    net::{PacketAction, PacketFamily, server::TalkServerServerPacket},
};
use serde_json::Value;

use crate::{
    ITEM_DB, SETTINGS,
    character::Character,
    db::{DbHandle, insert_params},
    world::WorldHandle,
};

const POLL: Duration = Duration::from_secs(1);
const STATUS_RUNNING: i32 = 1;
const STATUS_DONE: i32 = 2;
const STATUS_FAILED: i32 = 3;

struct Request {
    id: i32,
    action: String,
    account_id: Option<i32>,
    character_id: Option<i32>,
    payload: Value,
    by: String,
}

pub async fn run(db: DbHandle, world: WorldHandle) {
    let mut interval = tokio::time::interval(POLL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        if let Err(e) = process_pending(&db, &world).await {
            tracing::warn!("admin requests: {}", e);
        }
    }
}

async fn process_pending(db: &DbHandle, world: &WorldHandle) -> anyhow::Result<()> {
    let rows = db
        .query(
            "SELECT `id`, `action`, `account_id`, `character_id`, `payload`, `requested_by` \
             FROM `admin_requests` WHERE `status` = 0 ORDER BY `id` LIMIT 20",
        )
        .await?;
    for row in rows {
        let request = Request {
            id: row.get_int(0).unwrap_or(0),
            action: row.get_text(1).unwrap_or_default(),
            account_id: row.get_int(2),
            character_id: row.get_int(3),
            payload: row
                .get_text(4)
                .and_then(|p| serde_json::from_str(&p).ok())
                .unwrap_or(Value::Null),
            by: row.get_text(5).unwrap_or_default(),
        };
        // claim it (a second server instance or a retry never runs it twice)
        let claimed = db
            .execute(&insert_params(
                "UPDATE `admin_requests` SET `status` = :running WHERE `id` = :id AND `status` = 0",
                &[("running", &STATUS_RUNNING), ("id", &request.id)],
            ))
            .await;
        if claimed.is_err() {
            continue;
        }
        let outcome = execute(db, world, &request).await;
        let (status, result) = match outcome {
            Ok(message) => (STATUS_DONE, message),
            Err(e) => (STATUS_FAILED, e.to_string()),
        };
        tracing::info!(
            "admin request #{} {} by {}: {}",
            request.id,
            request.action,
            request.by,
            if status == STATUS_DONE {
                "done"
            } else {
                "failed"
            }
        );
        db.execute(&insert_params(
            "UPDATE `admin_requests` SET `status` = :status, `result` = :result, `completed_at` = CURRENT_TIMESTAMP WHERE `id` = :id",
            &[("status", &status), ("result", &result), ("id", &request.id)],
        ))
        .await?;
    }
    Ok(())
}

// ---------- helpers ----------

fn text(p: &Value, key: &str) -> String {
    p.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn int(p: &Value, key: &str) -> Option<i64> {
    p.get(key).and_then(Value::as_i64)
}

fn required_reason(p: &Value) -> anyhow::Result<String> {
    let reason = text(p, "reason");
    if reason.is_empty() || reason.chars().count() > 1000 {
        anyhow::bail!("a reason (max 1000 characters) is required");
    }
    Ok(reason)
}

/// Minutes from the payload: None = permanent, Some(n) with 1 ≤ n ≤ 10 years.
fn duration_minutes(p: &Value) -> anyhow::Result<Option<i64>> {
    match p.get("minutes") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_i64() {
            Some(n) if (1..=10 * 365 * 24 * 60).contains(&n) => Ok(Some(n)),
            _ => anyhow::bail!("invalid duration"),
        },
    }
}

async fn account_of_character(db: &DbHandle, character_id: i32) -> anyhow::Result<(i32, String)> {
    let row = db
        .query_one(&insert_params(
            "SELECT `account_id`, `name` FROM `characters` WHERE `id` = :id",
            &[("id", &character_id)],
        ))
        .await?
        .ok_or_else(|| anyhow::anyhow!("character {} does not exist", character_id))?;
    Ok((
        row.get_int(0).unwrap_or(0),
        row.get_string(1).unwrap_or_default(),
    ))
}

async fn account_exists(db: &DbHandle, account_id: i32) -> anyhow::Result<()> {
    let n = db
        .query_int(&insert_params(
            "SELECT COUNT(*) FROM `accounts` WHERE `id` = :id",
            &[("id", &account_id)],
        ))
        .await?
        .unwrap_or(0);
    if n == 0 {
        anyhow::bail!("account {} does not exist", account_id);
    }
    Ok(())
}

async fn character_names(db: &DbHandle, account_id: i32) -> anyhow::Result<Vec<String>> {
    let rows = db
        .query(&insert_params(
            "SELECT `name` FROM `characters` WHERE `account_id` = :id",
            &[("id", &account_id)],
        ))
        .await?;
    Ok(rows.iter().filter_map(|r| r.get_text(0)).collect())
}

/// Characters of the account that are in the game right now.
async fn online_characters(
    db: &DbHandle,
    world: &WorldHandle,
    account_id: i32,
) -> anyhow::Result<Vec<Box<Character>>> {
    let mut online = Vec::new();
    for name in character_names(db, account_id).await? {
        if let Ok(character) = world.get_character_by_name(&name).await {
            online.push(character);
        }
    }
    Ok(online)
}

async fn tell(character: &Character, message: String) {
    if let Some(player) = character.player.as_ref() {
        player.send(
            PacketAction::Server,
            PacketFamily::Talk,
            &TalkServerServerPacket { message },
        );
    }
}

#[allow(clippy::too_many_arguments)]
async fn record_action(
    db: &DbHandle,
    request: &Request,
    account_id: i32,
    character_id: Option<i32>,
    action: &str,
    reason: &str,
    expires_at: Option<NaiveDateTime>,
    details: &str,
) -> anyhow::Result<()> {
    let report_id = int(&request.payload, "report_id").map(|v| v as i32);
    db.execute(&insert_params(
        "INSERT INTO `moderation_actions` (`account_id`, `character_id`, `action`, `reason`, `issued_by`, \
         `report_id`, `request_id`, `expires_at`, `details`) VALUES (:account_id, :character_id, :action, \
         :reason, :by, :report_id, :request_id, :expires_at, :details)",
        &[
            ("account_id", &account_id),
            ("character_id", &character_id),
            ("action", &action),
            ("reason", &reason),
            ("by", &request.by),
            ("report_id", &report_id),
            ("request_id", &request.id),
            ("expires_at", &expires_at),
            ("details", &details),
        ],
    ))
    .await
}

fn target_account(request: &Request) -> anyhow::Result<i32> {
    request
        .account_id
        .ok_or_else(|| anyhow::anyhow!("no account given"))
}

fn target_character(request: &Request) -> anyhow::Result<i32> {
    request
        .character_id
        .ok_or_else(|| anyhow::anyhow!("no character given"))
}

// ---------- actions ----------

async fn execute(db: &DbHandle, world: &WorldHandle, r: &Request) -> anyhow::Result<String> {
    match r.action.as_str() {
        "kick" => kick(db, world, r).await,
        "ban" => ban(db, world, r).await,
        "unban" => unban(db, r).await,
        "mute" => mute(db, world, r).await,
        "unmute" => unmute(db, world, r).await,
        "warn" => warn(db, world, r).await,
        "announce" => announce(world, r).await,
        "give_item" => adjust_item(db, world, r, 1).await,
        "remove_item" => adjust_item(db, world, r, -1).await,
        "warp" => warp(db, world, r, false).await,
        "unstick" => warp(db, world, r, true).await,
        "reset_password" => reset_password(db, world, r).await,
        "revoke_sessions" => revoke_sessions(db, r).await,
        "maintenance" => maintenance(db, r).await,
        other => anyhow::bail!("unknown action {}", other),
    }
}

async fn kick(db: &DbHandle, world: &WorldHandle, r: &Request) -> anyhow::Result<String> {
    let account_id = target_account(r)?;
    let reason = required_reason(&r.payload)?;
    let online = online_characters(db, world, account_id).await?;
    for character in &online {
        world.kick_player(character.name.clone(), r.by.clone(), true);
    }
    record_action(
        db,
        r,
        account_id,
        online.first().map(|c| c.id),
        "kick",
        &reason,
        None,
        "",
    )
    .await?;
    Ok(if online.is_empty() {
        "not online (recorded)".to_string()
    } else {
        format!(
            "kicked {}",
            online
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

async fn ban(db: &DbHandle, world: &WorldHandle, r: &Request) -> anyhow::Result<String> {
    let account_id = target_account(r)?;
    account_exists(db, account_id).await?;
    let reason = required_reason(&r.payload)?;
    let minutes = duration_minutes(&r.payload)?;
    // account ban only: every web player reaches the server from the same proxy address
    db.execute(&insert_params(
        "INSERT INTO `bans` (`account_id`, `ip`, `duration`) VALUES (:account_id, '', :duration)",
        &[
            ("account_id", &account_id),
            ("duration", &(minutes.unwrap_or(0) as i32)),
        ],
    ))
    .await?;
    for character in online_characters(db, world, account_id).await? {
        world.kick_player(character.name.clone(), r.by.clone(), true);
    }
    let expires = minutes.map(|m| (Utc::now() + chrono::Duration::minutes(m)).naive_utc());
    record_action(
        db,
        r,
        account_id,
        r.character_id,
        "ban",
        &reason,
        expires,
        "",
    )
    .await?;
    Ok(match minutes {
        None => "banned permanently".to_string(),
        Some(m) => format!("banned for {} minutes", m),
    })
}

async fn unban(db: &DbHandle, r: &Request) -> anyhow::Result<String> {
    let account_id = target_account(r)?;
    let reason = required_reason(&r.payload)?;
    db.execute(&insert_params(
        "DELETE FROM `bans` WHERE `account_id` = :account_id",
        &[("account_id", &account_id)],
    ))
    .await?;
    record_action(db, r, account_id, None, "unban", &reason, None, "").await?;
    Ok("unbanned".to_string())
}

async fn mute(db: &DbHandle, world: &WorldHandle, r: &Request) -> anyhow::Result<String> {
    let account_id = target_account(r)?;
    account_exists(db, account_id).await?;
    let reason = required_reason(&r.payload)?;
    let minutes = duration_minutes(&r.payload)?;
    let expires = minutes.map(|m| (Utc::now() + chrono::Duration::minutes(m)).naive_utc());
    db.execute(&insert_params(
        "INSERT INTO `mutes` (`account_id`, `reason`, `issued_by`, `expires_at`) VALUES (:account_id, :reason, :by, :expires)",
        &[("account_id", &account_id), ("reason", &reason), ("by", &r.by), ("expires", &expires)],
    ))
    .await?;
    for character in online_characters(db, world, account_id).await? {
        if let Some(player) = character.player.as_ref() {
            player.set_mute(Some(expires));
        }
        tell(
            &character,
            match expires {
                None => "You have been muted by a staff member.".to_string(),
                Some(t) => format!(
                    "You have been muted until {} UTC.",
                    t.format("%Y-%m-%d %H:%M")
                ),
            },
        )
        .await;
    }
    record_action(
        db,
        r,
        account_id,
        r.character_id,
        "mute",
        &reason,
        expires,
        "",
    )
    .await?;
    Ok(match minutes {
        None => "muted permanently".to_string(),
        Some(m) => format!("muted for {} minutes", m),
    })
}

async fn unmute(db: &DbHandle, world: &WorldHandle, r: &Request) -> anyhow::Result<String> {
    let account_id = target_account(r)?;
    let reason = required_reason(&r.payload)?;
    db.execute(&insert_params(
        "UPDATE `mutes` SET `revoked_at` = CURRENT_TIMESTAMP, `revoked_by` = :by WHERE `account_id` = :account_id AND `revoked_at` IS NULL",
        &[("by", &r.by), ("account_id", &account_id)],
    ))
    .await?;
    for character in online_characters(db, world, account_id).await? {
        if let Some(player) = character.player.as_ref() {
            player.set_mute(None);
        }
        tell(&character, "You can chat again.".to_string()).await;
    }
    record_action(db, r, account_id, None, "unmute", &reason, None, "").await?;
    Ok("unmuted".to_string())
}

async fn warn(db: &DbHandle, world: &WorldHandle, r: &Request) -> anyhow::Result<String> {
    let account_id = target_account(r)?;
    account_exists(db, account_id).await?;
    let reason = required_reason(&r.payload)?;
    let category = text(&r.payload, "category");
    if category.is_empty() || category.chars().count() > 64 {
        anyhow::bail!("a warning category is required");
    }
    let report_id = int(&r.payload, "report_id").map(|v| v as i32);
    db.execute(&insert_params(
        "INSERT INTO `warnings` (`account_id`, `character_id`, `category`, `reason`, `issued_by`, `report_id`) \
         VALUES (:account_id, :character_id, :category, :reason, :by, :report_id)",
        &[
            ("account_id", &account_id),
            ("character_id", &r.character_id),
            ("category", &category),
            ("reason", &reason),
            ("by", &r.by),
            ("report_id", &report_id),
        ],
    ))
    .await?;
    let online = online_characters(db, world, account_id).await?;
    for character in &online {
        if let Some(player) = character.player.as_ref() {
            player.deliver_warnings();
        }
    }
    record_action(
        db,
        r,
        account_id,
        r.character_id,
        "warn",
        &reason,
        None,
        &category,
    )
    .await?;
    Ok(if online.is_empty() {
        "warning stored; shown at the next login".to_string()
    } else {
        "warning delivered in game".to_string()
    })
}

async fn announce(world: &WorldHandle, r: &Request) -> anyhow::Result<String> {
    let message = text(&r.payload, "message");
    if message.is_empty() || message.chars().count() > 200 {
        anyhow::bail!("announcement must be 1-200 characters");
    }
    world.broadcast_announcement("Server".to_string(), message);
    Ok("announced".to_string())
}

/// Give (sign 1) or remove (sign -1) items/gold. Online: through the character's map. Offline: while
/// the account is locked against logging in, load → change → save.
async fn adjust_item(
    db: &DbHandle,
    world: &WorldHandle,
    r: &Request,
    sign: i32,
) -> anyhow::Result<String> {
    let character_id = target_character(r)?;
    let reason = required_reason(&r.payload)?;
    let item_id = int(&r.payload, "item_id").unwrap_or(0) as i32;
    let amount = int(&r.payload, "amount").unwrap_or(0);
    let max_item = SETTINGS.load().limits.max_item;
    if amount < 1 || amount > max_item as i64 {
        anyhow::bail!("amount must be between 1 and {}", max_item);
    }
    let amount = amount as i32;
    let item_name = match ITEM_DB.load().items.get((item_id - 1).max(0) as usize) {
        Some(item) if item_id >= 1 && !item.name.is_empty() => item.name.clone(),
        _ => anyhow::bail!("item {} does not exist", item_id),
    };
    let (account_id, name) = account_of_character(db, character_id).await?;

    let (before, after) = if let Ok(character) = world.get_character_by_name(&name).await {
        let before = character.get_item_amount(item_id);
        if sign < 0 && before < amount {
            anyhow::bail!("{} only has {} {}", name, before, item_name);
        }
        let map = world
            .get_map(character.map_id)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        let player_id = character.player_id.unwrap_or(0);
        if sign > 0 {
            map.give_item(player_id, item_id, amount);
        } else {
            map.lose_item(player_id, item_id, amount);
        }
        // the map handles commands in order, so this read sees the change
        let after = map
            .get_character(player_id)
            .await
            .map_err(|e| anyhow::anyhow!(e))?
            .map(|c| c.get_item_amount(item_id))
            .unwrap_or(before);
        (before, after)
    } else {
        if !world
            .lock_account_for_admin(account_id)
            .await
            .map_err(|e| anyhow::anyhow!(e))?
        {
            anyhow::bail!("{} is logging in right now; try again in a moment", name);
        }
        let result = async {
            let mut character = Character::load(db, character_id)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            let before = character.get_item_amount(item_id);
            if sign > 0 {
                let add = amount.min(max_item - before);
                if add > 0 {
                    character.add_item_no_quest_rules(item_id, add);
                }
            } else {
                if before < amount {
                    anyhow::bail!("{} only has {} {}", name, before, item_name);
                }
                character.remove_item_no_quest_rules(item_id, amount);
            }
            let after = character.get_item_amount(item_id);
            character
                .update(db)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            Ok((before, after))
        }
        .await;
        world.unlock_account_for_admin(account_id);
        result?
    };

    db.execute(&insert_params(
        "INSERT INTO `item_adjustments` (`character_id`, `item_id`, `delta`, `before_amount`, `after_amount`, \
         `reason`, `issued_by`, `request_id`) VALUES (:character_id, :item_id, :delta, :before, :after, :reason, :by, :request_id)",
        &[
            ("character_id", &character_id),
            ("item_id", &item_id),
            ("delta", &(after - before)),
            ("before", &before),
            ("after", &after),
            ("reason", &reason),
            ("by", &r.by),
            ("request_id", &r.id),
        ],
    ))
    .await?;
    let verb = if sign > 0 { "gave" } else { "removed" };
    Ok(format!(
        "{} {} {} ({} → {})",
        verb,
        (after - before).abs(),
        item_name,
        before,
        after
    ))
}

async fn warp(
    db: &DbHandle,
    world: &WorldHandle,
    r: &Request,
    unstick: bool,
) -> anyhow::Result<String> {
    let character_id = target_character(r)?;
    let reason = required_reason(&r.payload)?;
    let (map_id, coords) = if unstick {
        let settings = SETTINGS.load();
        (
            settings.rescue.map,
            Coords {
                x: settings.rescue.x,
                y: settings.rescue.y,
            },
        )
    } else {
        let map_id = int(&r.payload, "map").unwrap_or(0) as i32;
        let coords = Coords {
            x: int(&r.payload, "x").unwrap_or(-1) as i32,
            y: int(&r.payload, "y").unwrap_or(-1) as i32,
        };
        let map = world
            .get_map(map_id)
            .await
            .map_err(|_| anyhow::anyhow!("map {} does not exist", map_id))?;
        let size = map.get_dimensions().await.map_err(|e| anyhow::anyhow!(e))?;
        if coords.x < 0 || coords.y < 0 || coords.x > size.x || coords.y > size.y {
            anyhow::bail!("coordinates outside map {} ({}x{})", map_id, size.x, size.y);
        }
        (map_id, coords)
    };
    let (account_id, name) = account_of_character(db, character_id).await?;
    let place = format!("map {} ({}, {})", map_id, coords.x, coords.y);
    if let Ok(character) = world.get_character_by_name(&name).await {
        if let Some(player) = character.player.as_ref() {
            player.request_warp(map_id, coords, character.map_id == map_id, None);
        }
    } else {
        if !world
            .lock_account_for_admin(account_id)
            .await
            .map_err(|e| anyhow::anyhow!(e))?
        {
            anyhow::bail!("{} is logging in right now; try again in a moment", name);
        }
        let result = async {
            let mut character = Character::load(db, character_id)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            character.map_id = map_id;
            character.coords = coords;
            character
                .update(db)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
        }
        .await;
        world.unlock_account_for_admin(account_id);
        result?;
    }
    record_action(
        db,
        r,
        account_id,
        Some(character_id),
        if unstick { "unstick" } else { "warp" },
        &reason,
        None,
        &place,
    )
    .await?;
    Ok(format!("{} moved to {}", name, place))
}

async fn reset_password(db: &DbHandle, world: &WorldHandle, r: &Request) -> anyhow::Result<String> {
    let account_id = target_account(r)?;
    account_exists(db, account_id).await?;
    let reason = required_reason(&r.payload)?;
    // the panel sends only an argon2id hash; the plaintext never reaches the game server or its logs
    let hash = text(&r.payload, "password_hash");
    if !hash.starts_with("$argon2id$") || hash.len() > 200 {
        anyhow::bail!("invalid password hash");
    }
    db.execute(&insert_params(
        "UPDATE `accounts` SET `password_hash` = :hash WHERE `id` = :id",
        &[("hash", &hash), ("id", &account_id)],
    ))
    .await?;
    db.execute(&insert_params(
        "DELETE FROM `account_sessions` WHERE `account_id` = :id",
        &[("id", &account_id)],
    ))
    .await?;
    for character in online_characters(db, world, account_id).await? {
        world.kick_player(character.name.clone(), r.by.clone(), true);
    }
    // drop the hash from the stored request
    db.execute(&insert_params(
        "UPDATE `admin_requests` SET `payload` = :payload WHERE `id` = :id",
        &[
            (
                "payload",
                &serde_json::json!({ "reason": reason }).to_string(),
            ),
            ("id", &r.id),
        ],
    ))
    .await?;
    record_action(db, r, account_id, None, "password_reset", &reason, None, "").await?;
    Ok("password reset; sessions revoked".to_string())
}

async fn revoke_sessions(db: &DbHandle, r: &Request) -> anyhow::Result<String> {
    let account_id = target_account(r)?;
    let reason = required_reason(&r.payload)?;
    db.execute(&insert_params(
        "DELETE FROM `account_sessions` WHERE `account_id` = :id",
        &[("id", &account_id)],
    ))
    .await?;
    record_action(
        db,
        r,
        account_id,
        None,
        "revoke_sessions",
        &reason,
        None,
        "",
    )
    .await?;
    Ok("remembered sessions revoked".to_string())
}

async fn maintenance(db: &DbHandle, r: &Request) -> anyhow::Result<String> {
    let on = r
        .payload
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let value = if on { "1" } else { "0" };
    db.execute("DELETE FROM `server_flags` WHERE `name` = 'maintenance'")
        .await?;
    db.execute(&insert_params(
        "INSERT INTO `server_flags` (`name`, `value`, `updated_by`) VALUES ('maintenance', :value, :by)",
        &[("value", &value), ("by", &r.by)],
    ))
    .await?;
    Ok(if on {
        "maintenance on: only staff can log in"
    } else {
        "maintenance off"
    }
    .to_string())
}
