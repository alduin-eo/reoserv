use eolib::protocol::net::{PacketAction, PacketFamily, server::TalkServerServerPacket};

use super::super::World;
use crate::{
    db::{DbHandle, insert_params},
    player::PlayerHandle,
};

const USAGE: &str =
    "Usage: $bots on N | off | add N | remove N | status | list | inspect NAME | pause | resume";
const MAX_BOTS: i32 = 200;

fn reply(player: &PlayerHandle, message: impl Into<String>) {
    player.send(
        PacketAction::Server,
        PacketFamily::Talk,
        &TalkServerServerPacket {
            message: message.into(),
        },
    );
}

async fn queue(db: &DbHandle, command: &str, arg: &str, admin_name: &str) -> anyhow::Result<()> {
    db.execute(&insert_params(
        "INSERT INTO `bot_control` (`command`, `arg`, `issued_by`) VALUES (:command, :arg, :issued_by)",
        &[
            ("command", &command.to_string()),
            ("arg", &arg.to_string()),
            ("issued_by", &admin_name.to_string()),
        ],
    ))
    .await
}

async fn status(db: &DbHandle, player: &PlayerHandle) -> anyhow::Result<()> {
    let manager = db
        .query_one(
            "SELECT `desired`, `paused`, `heartbeat`, `summary` FROM `bot_manager` WHERE `id` = 1",
        )
        .await?;
    let Some(manager) = manager else {
        reply(player, "Bot manager has never run.");
        return Ok(());
    };
    let age = manager
        .get_date(2)
        .map(|hb| (chrono::Utc::now().naive_utc() - hb).num_seconds());
    let online = db
        .query_int("SELECT COUNT(1) FROM `bot_status` WHERE `online` = 1")
        .await?
        .unwrap_or(0);
    let state = match age {
        Some(age) if age <= 15 => format!("running (heartbeat {}s ago)", age),
        Some(age) => format!("NOT RESPONDING (last heartbeat {}s ago)", age),
        None => "never started".to_string(),
    };
    reply(
        player,
        format!(
            "Bots: {} online / {} target, {}{}",
            online,
            manager.get_int(0).unwrap_or(0),
            state,
            if manager.get_int(1).unwrap_or(0) == 1 {
                ", PAUSED"
            } else {
                ""
            }
        ),
    );
    if let Some(summary) = manager.get_string(3) {
        for line in summary.lines().take(6) {
            reply(player, line);
        }
    }
    Ok(())
}

async fn list(db: &DbHandle, player: &PlayerHandle) -> anyhow::Result<()> {
    let rows = db
        .query(
            "SELECT `character_name`, `level`, `personality`, `activity` FROM `bot_status` \
             WHERE `online` = 1 ORDER BY `level` DESC, `character_name` LIMIT 20",
        )
        .await?;
    if rows.is_empty() {
        reply(player, "No bots online.");
    }
    for row in rows {
        reply(
            player,
            format!(
                "{} lvl {} [{}] {}",
                row.get_string(0).unwrap_or_default(),
                row.get_int(1).unwrap_or(0),
                row.get_string(2).unwrap_or_default(),
                row.get_string(3).unwrap_or_default()
            ),
        );
    }
    Ok(())
}

async fn inspect(db: &DbHandle, player: &PlayerHandle, name: &str) -> anyhow::Result<()> {
    let row = db
        .query_one(&insert_params(
            "SELECT `character_name`, `account_name`, `personality`, `level`, `map`, `x`, `y`, \
             `activity`, `goal`, `online` FROM `bot_status` WHERE `character_name` = :name",
            &[("name", &name.to_lowercase())],
        ))
        .await?;
    let Some(row) = row else {
        reply(player, format!("No bot named {}", name));
        return Ok(());
    };
    reply(
        player,
        format!(
            "{} ({}) [{}] lvl {} map {} ({},{}) {}",
            row.get_string(0).unwrap_or_default(),
            row.get_string(1).unwrap_or_default(),
            row.get_string(2).unwrap_or_default(),
            row.get_int(3).unwrap_or(0),
            row.get_int(4).unwrap_or(0),
            row.get_int(5).unwrap_or(0),
            row.get_int(6).unwrap_or(0),
            if row.get_int(9).unwrap_or(0) == 1 {
                "online"
            } else {
                "offline"
            }
        ),
    );
    reply(
        player,
        format!("Doing: {}", row.get_string(7).unwrap_or_default()),
    );
    reply(
        player,
        format!("Goal: {}", row.get_string(8).unwrap_or_default()),
    );
    Ok(())
}

impl World {
    pub fn bots_command(&mut self, player_id: i32, admin_name: String, args: Vec<String>) {
        let player = match self.players.get(&player_id) {
            Some(player) => player.to_owned(),
            None => return,
        };
        let db = self.db.clone();

        tokio::spawn(async move {
            let action = args.first().map(|a| a.to_lowercase()).unwrap_or_default();
            let value = args.get(1).cloned().unwrap_or_default();
            let result = match action.as_str() {
                "on" | "add" | "remove" => match value.parse::<i32>() {
                    Ok(count) if (0..=MAX_BOTS).contains(&count) => {
                        let result = queue(&db, &action, &count.to_string(), &admin_name).await;
                        if result.is_ok() {
                            reply(&player, format!("Queued: bots {} {}", action, count));
                        }
                        result
                    }
                    _ => {
                        reply(&player, format!("Usage: $bots {} <0-{}>", action, MAX_BOTS));
                        Ok(())
                    }
                },
                "off" | "pause" | "resume" => {
                    let result = queue(&db, &action, "", &admin_name).await;
                    if result.is_ok() {
                        reply(&player, format!("Queued: bots {}", action));
                    }
                    result
                }
                "status" => status(&db, &player).await,
                "list" => list(&db, &player).await,
                "inspect" if !value.is_empty() => inspect(&db, &player, &value).await,
                _ => {
                    reply(&player, USAGE);
                    Ok(())
                }
            };

            if let Err(e) = result {
                tracing::error!("$bots {} failed: {}", action, e);
                reply(&player, "Bot command failed (see server log).");
            }
        });
    }
}
