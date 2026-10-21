use crate::{
    character::Character,
    errors::{CharacterNotFoundError, DataNotFoundError},
};

use super::World;

impl World {
    pub async fn get_character_by_name(
        &self,
        name: &str,
    ) -> Result<Box<Character>, Box<dyn std::error::Error + Sync + Send>> {
        let player_id = self.characters.get(name);

        if player_id.is_none() {
            return Err(Box::new(CharacterNotFoundError::new(name.to_string())));
        }

        let player_id = player_id.unwrap();

        let player = self.players.get(player_id);

        if player.is_none() {
            return Err(Box::new(DataNotFoundError::new(
                "Player".to_string(),
                *player_id,
            )));
        }

        let player = player.unwrap();

        // Can fail when the player is between maps or has just disconnected but
        // not been dropped from the world yet. This runs inside the World actor
        // and release builds abort on panic, so never unwrap here (a whisper to
        // a player who is logging out would take the whole server down unsaved).
        player.get_character().await
    }
}
