// Alduin moderation for a connected player: server-side mutes, official warnings (shown until the
// player acknowledges them, delivered again after reconnects/restarts) and in-game player reports.
// Website staff actions reach the game through admin_requests (see crate::admin_requests).
use chrono::{NaiveDateTime, Utc};
use eolib::{
    data::{EoReader, EoSerialize},
    protocol::net::{PacketAction, PacketFamily, server::TalkServerServerPacket},
};
use rand::RngExt;

use crate::{
    db::insert_params,
    deep::{
        FAMILY_MODERATION, ReportPlayerClientPacket, ReportReplyServerPacket,
        WarningAcceptClientPacket, WarningOpenServerPacket,
    },
};

use super::{Player, account::generate_password_hash};

/// Report categories accepted from the game client (same list as the website form).
pub const REPORT_CATEGORIES: &[&str] = &[
    "Text harassment",
    "Hate speech / racism",
    "Cheating / hacking",
    "Bug exploitation",
    "Scamming",
    "Advertising / spam",
    "Impersonation",
    "Threats",
    "Inappropriate character name",
    "Griefing",
    "Real-world money trading",
    "Other",
];
const REPORTS_PER_HOUR: i32 = 5;
const MAX_DESCRIPTION: usize = 1000;
const MIN_DESCRIPTION: usize = 10;

/// Unambiguous characters for report numbers and tracking codes.
const CODE_CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

pub fn random_code(len: usize) -> String {
    let mut rng = rand::rng();
    (0..len)
        .map(|_| CODE_CHARS[rng.random_range(0..CODE_CHARS.len())] as char)
        .collect()
}

/// Hash of a report tracking code (argon2id PHC string, verified by the website).
pub fn hash_tracking_code(code: &str) -> String {
    generate_password_hash("", code)
}

impl Player {
    /// Active mute: None = not muted, Some(None) = permanent, Some(Some(t)) = until t (UTC).
    pub async fn load_mute(&mut self) {
        self.mute = None;
        let rows = self
            .db
            .query(&insert_params(
                "SELECT `expires_at` FROM `mutes` WHERE `account_id` = :account_id AND `revoked_at` IS NULL",
                &[("account_id", &self.account_id)],
            ))
            .await
            .unwrap_or_default();
        let now = Utc::now().naive_utc();
        for row in &rows {
            match row.get_date(0) {
                None => {
                    self.mute = Some(None);
                    return;
                }
                // keep the latest expiry among overlapping temporary mutes
                Some(expires)
                    if expires > now && !matches!(self.mute, Some(Some(t)) if t > expires) =>
                {
                    self.mute = Some(Some(expires));
                }
                _ => {}
            }
        }
    }

    pub fn set_mute(&mut self, mute: Option<Option<NaiveDateTime>>) {
        self.mute = mute;
    }

    /// True (and tells the player) when chat is blocked by a server-side mute.
    pub async fn chat_blocked(&mut self) -> bool {
        let until = match self.mute {
            None => return false,
            Some(until) => until,
        };
        if let Some(t) = until
            && t <= Utc::now().naive_utc()
        {
            self.mute = None;
            return false;
        }
        let message = match until {
            None => "You are muted and cannot chat.".to_string(),
            Some(t) => format!(
                "You are muted until {} UTC and cannot chat.",
                t.format("%Y-%m-%d %H:%M")
            ),
        };
        let _ = self
            .bus
            .send(
                PacketAction::Server,
                PacketFamily::Talk,
                TalkServerServerPacket { message },
            )
            .await;
        true
    }

    /// Send every warning the account has not acknowledged yet (on entering the game, and when
    /// staff issue one while the player is online).
    pub async fn deliver_pending_warnings(&mut self) {
        let rows = self
            .db
            .query(&insert_params(
                "SELECT `id`, `category`, `reason`, `created_at` FROM `warnings` \
                 WHERE `account_id` = :account_id AND `acknowledged_at` IS NULL ORDER BY `id`",
                &[("account_id", &self.account_id)],
            ))
            .await
            .unwrap_or_default();
        for row in &rows {
            let warning_id = row.get_int(0).unwrap_or(0);
            let packet = WarningOpenServerPacket {
                warning_id,
                category: row.get_string(1).unwrap_or_default(),
                reason: row.get_string(2).unwrap_or_default(),
                issued_at: row
                    .get_date(3)
                    .map(|d| d.format("%B %-d, %Y").to_string())
                    .unwrap_or_default(),
            };
            let _ = self
                .bus
                .send(
                    PacketAction::Open,
                    PacketFamily::Unrecognized(FAMILY_MODERATION),
                    packet,
                )
                .await;
            let _ = self
                .db
                .execute(&insert_params(
                    "UPDATE `warnings` SET `delivered_at` = CURRENT_TIMESTAMP WHERE `id` = :id AND `delivered_at` IS NULL",
                    &[("id", &warning_id)],
                ))
                .await;
        }
    }

    async fn warning_accept(&mut self, reader: EoReader) {
        let packet = match WarningAcceptClientPacket::deserialize(&reader) {
            Ok(packet) => packet,
            Err(e) => {
                tracing::error!("Error deserializing WarningAcceptClientPacket {}", e);
                return;
            }
        };
        // only the warned account can acknowledge its own warning
        let _ = self
            .db
            .execute(&insert_params(
                "UPDATE `warnings` SET `acknowledged_at` = CURRENT_TIMESTAMP \
                 WHERE `id` = :id AND `account_id` = :account_id AND `acknowledged_at` IS NULL",
                &[("id", &packet.warning_id), ("account_id", &self.account_id)],
            ))
            .await;
    }

    async fn report_reply(&mut self, code: i32, reference: String, tracking_code: String) {
        let _ = self
            .bus
            .send(
                PacketAction::Reply,
                PacketFamily::Unrecognized(FAMILY_MODERATION),
                ReportReplyServerPacket {
                    code,
                    reference,
                    tracking_code,
                },
            )
            .await;
    }

    async fn report_player(&mut self, reader: EoReader) {
        let packet = match ReportPlayerClientPacket::deserialize(&reader) {
            Ok(packet) => packet,
            Err(e) => {
                tracing::error!("Error deserializing ReportPlayerClientPacket {}", e);
                return;
            }
        };
        let description = packet.description.trim().to_string();
        let category = match REPORT_CATEGORIES.iter().find(|c| **c == packet.category) {
            Some(c) => c.to_string(),
            None => return self.report_reply(1, String::new(), String::new()).await,
        };
        if description.chars().count() < MIN_DESCRIPTION
            || description.chars().count() > MAX_DESCRIPTION
        {
            return self.report_reply(1, String::new(), String::new()).await;
        }

        // the reporter is this session's own character, never something the client claims
        let map = match &self.map {
            Some(map) => map.clone(),
            None => return,
        };
        let reporter = match map.get_character(self.id).await {
            Ok(Some(character)) => character,
            _ => return,
        };
        // the reported character must be online right now; its ids come from the server
        let reported = match self.world.get_player(packet.player_id).await {
            Ok(Some(player)) if packet.player_id != self.id => match player.get_character().await {
                Ok(character) => character,
                Err(_) => return self.report_reply(3, String::new(), String::new()).await,
            },
            _ => return self.report_reply(3, String::new(), String::new()).await,
        };

        let since = (Utc::now() - chrono::Duration::hours(1)).naive_utc();
        let recent = self
            .db
            .query_int(&insert_params(
                "SELECT COUNT(*) FROM `reports` WHERE `reporter_account_id` = :account_id AND `created_at` > :since",
                &[("account_id", &self.account_id), ("since", &since)],
            ))
            .await
            .ok()
            .flatten()
            .unwrap_or(0);
        if recent >= REPORTS_PER_HOUR {
            return self.report_reply(2, String::new(), String::new()).await;
        }

        let public_id = format!("R-{}", random_code(8));
        let tracking_code = random_code(12);
        let token_hash = hash_tracking_code(&tracking_code);
        let result = self
            .db
            .execute(&insert_params(
                "INSERT INTO `reports` (`public_id`, `token_hash`, `source`, `reporter_account_id`, \
                 `reporter_character_id`, `reporter_name`, `reported_character_id`, `reported_name`, \
                 `category`, `description`, `incident_at`, `map_id`, `x`, `y`, `reporter_ip`) VALUES \
                 (:public_id, :token_hash, 'game', :reporter_account_id, :reporter_character_id, :reporter_name, \
                 :reported_character_id, :reported_name, :category, :description, CURRENT_TIMESTAMP, :map_id, :x, :y, :ip)",
                &[
                    ("public_id", &public_id),
                    ("token_hash", &token_hash),
                    ("reporter_account_id", &self.account_id),
                    ("reporter_character_id", &reporter.id),
                    ("reporter_name", &reporter.name),
                    ("reported_character_id", &reported.id),
                    ("reported_name", &reported.name),
                    ("category", &category),
                    ("description", &description),
                    ("map_id", &reporter.map_id),
                    ("x", &reporter.coords.x),
                    ("y", &reporter.coords.y),
                    ("ip", &self.ip),
                ],
            ))
            .await;
        match result {
            Ok(_) => {
                tracing::info!(
                    "{} reported {} ({}) as {}",
                    reporter.name,
                    reported.name,
                    public_id,
                    category
                );
                self.report_reply(0, public_id, tracking_code).await;
            }
            Err(e) => {
                tracing::error!("Failed to store report: {}", e);
                self.report_reply(1, String::new(), String::new()).await;
            }
        }
    }

    pub async fn handle_moderation(&mut self, action: PacketAction, reader: EoReader) {
        match action {
            PacketAction::Accept => self.warning_accept(reader).await,
            PacketAction::Report => self.report_player(reader).await,
            _ => tracing::error!("Unhandled packet Moderation_{:?}", action),
        }
    }
}
