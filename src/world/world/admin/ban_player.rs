use super::super::World;
use crate::{LANG, db::insert_params};

impl World {
    pub async fn ban_player(
        &mut self,
        victim_name: String,
        admin_name: String,
        duration: String,
        silent: bool,
    ) {
        if let Some(player_id) = self.characters.get(&victim_name)
            && let Some(player) = self.players.get(player_id)
        {
            player.close("Player banned".to_string());
        }

        if !silent {
            self.broadcast_server_message(&get_lang_string!(
                &LANG.load().announce_remove,
                victim = victim_name,
                name = admin_name,
                method = "banned"
            ));
        }

        let db = self.db.clone();
        tokio::spawn(async move {
            let row = match db
                .query_one(&insert_params(
                    include_str!("../../../sql/get_character_account.sql"),
                    &[("character_name", &victim_name)],
                ))
                .await
            {
                Ok(Some(row)) => row,
                Err(err) => {
                    tracing::error!("Failed to get account from database: {}", err);
                    return;
                }
                _ => return,
            };

            let account_id = match row.get_int(0) {
                Some(account_id) => account_id,
                None => return,
            };

            let ip = match row.get_string(1) {
                Some(ip) => ip,
                None => return,
            };
            // Players behind a proxy (nginx for the web client, Docker NAT) all share a private
            // address: an IP ban on it would ban everyone, so such bans are account-only.
            let ip = if is_shared_address(&ip) {
                String::new()
            } else {
                ip
            };

            let duration = duration_str::parse(&duration);

            match db
                .execute(&insert_params(
                    include_str!("../../../sql/create_ban.sql"),
                    &[
                        ("account_id", &account_id),
                        ("ip", &ip),
                        (
                            "duration",
                            &match duration {
                                Ok(duration) => Some(format!("{}", duration.as_secs() / 60)),
                                Err(_) => None,
                            },
                        ),
                    ],
                ))
                .await
            {
                Ok(_) => {}
                Err(err) => {
                    tracing::error!("Failed to ban player: {}", err);
                }
            }
        });
    }
}

fn is_shared_address(ip: &str) -> bool {
    match ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        Ok(std::net::IpAddr::V6(v6)) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::is_shared_address;

    #[test]
    fn proxy_addresses_are_not_ip_banned() {
        assert!(is_shared_address("172.18.0.1"));
        assert!(is_shared_address("127.0.0.1"));
        assert!(is_shared_address("10.1.2.3"));
        assert!(is_shared_address("192.168.1.10"));
        assert!(!is_shared_address("203.0.113.9"));
    }
}
