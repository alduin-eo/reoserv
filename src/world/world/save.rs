use super::World;

impl World {
    /// Periodic save: runs in its own task so the world keeps answering requests (connection
    /// counts, parties, ...) while maps save their characters. Shutdown still awaits save_async.
    pub fn save_in_background(&self) {
        let Some(maps) = self.maps.as_ref() else {
            return;
        };
        let map_handles: Vec<_> = maps.values().cloned().collect();
        tokio::spawn(async move {
            const SAVE_BATCH_SIZE: usize = 4;
            for batch in map_handles.chunks(SAVE_BATCH_SIZE) {
                let saves = batch.iter().map(|map| map.save_async());
                for result in futures::future::join_all(saves).await {
                    if let Err(e) = result {
                        tracing::error!("Failed to save map: {}", e);
                    }
                }
            }
        });
    }

    pub async fn save_async(&mut self) {
        if let Some(maps) = self.maps.as_ref() {
            const SAVE_BATCH_SIZE: usize = 4;

            let map_handles: Vec<_> = maps.values().collect();
            for batch in map_handles.chunks(SAVE_BATCH_SIZE) {
                let saves = batch.iter().map(|map| map.save_async());
                let results = futures::future::join_all(saves).await;
                for result in results {
                    if let Err(e) = result {
                        tracing::error!("Failed to save map: {}", e);
                    }
                }
            }
        }
    }
}
