use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use config::ConfigError;
use notify::{
    Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
    event::{AccessKind, AccessMode, ModifyKind},
};
use once_cell::sync::Lazy;
use tokio::sync::mpsc;

use crate::{SETTINGS, world::WorldHandle};

const DEBOUNCE_DURATION: Duration = Duration::from_millis(500);

pub fn spawn_dir_watcher<T: Send + Sync + 'static>(
    dir: PathBuf,
    arc_swap: &'static Lazy<ArcSwap<T>>,
    reload_fn: fn() -> Result<T, ConfigError>,
    name: &'static str,
) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Event>();

    let mut watcher: RecommendedWatcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        notify::Config::default(),
    )
    .expect("Failed to create config dir watcher");

    if dir.exists() {
        watcher
            .watch(&dir, RecursiveMode::NonRecursive)
            .unwrap_or_else(|_| panic!("Failed to watch config dir: {:?}", dir));
        tracing::debug!("Watching config dir: {:?}", dir);
    } else {
        tracing::warn!("Config dir not found: {:?}", dir);
    }

    tokio::spawn(async move {
        let _watcher = watcher;
        let mut last_event: Option<Instant> = None;
        let mut debounce_timer = tokio::time::interval(DEBOUNCE_DURATION);
        debounce_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        debounce_timer.tick().await;

        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    let relevant = event.paths.iter().any(|p| {
                        p.extension().is_some_and(|e| e == "toml" || e == "ron")
                    });
                    let has_modify = event.kind.is_modify() || event.kind.is_create();
                    if relevant && has_modify {
                        last_event = Some(Instant::now());
                    }
                }
                _ = debounce_timer.tick() => {
                    if let Some(last) = last_event
                        && last.elapsed() >= DEBOUNCE_DURATION {
                            last_event = None;
                            match reload_fn() {
                                Ok(new_val) => {
                                    arc_swap.store(Arc::new(new_val));
                                    tracing::debug!("Reloaded config: {}", name);
                                }
                                Err(e) => {
                                    tracing::error!("Failed to reload {}: {}", name, e);
                                }
                            }
                        }
                }
            }
        }
    });
}

pub fn spawn_lang_watcher(arc_swap: &'static Lazy<ArcSwap<crate::lang::Lang>>) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Event>();

    let mut watcher: RecommendedWatcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        notify::Config::default(),
    )
    .expect("Failed to create lang dir watcher");

    let lang_dir = PathBuf::from("config/lang");
    if lang_dir.exists() {
        watcher
            .watch(&lang_dir, RecursiveMode::NonRecursive)
            .unwrap_or_else(|_| panic!("Failed to watch lang dir: {:?}", lang_dir));
        tracing::debug!("Watching lang dir: {:?}", lang_dir);
    }

    let mut current_lang = SETTINGS.load().server.lang.clone();

    tokio::spawn(async move {
        let _watcher = watcher;
        let mut last_event: Option<Instant> = None;
        let mut debounce_timer = tokio::time::interval(DEBOUNCE_DURATION);
        debounce_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        debounce_timer.tick().await;

        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    let relevant = event.paths.iter().any(|p| {
                        p.extension().is_some_and(|e| e == "ron")
                    });
                    let has_modify = event.kind.is_modify() || event.kind.is_create();
                    if relevant && has_modify {
                        last_event = Some(Instant::now());
                    }
                }
                _ = debounce_timer.tick() => {
                    let lang = &SETTINGS.load().server.lang;
                    if *lang != current_lang {
                        current_lang = lang.clone();
                        tracing::debug!("Lang switched to: {}", current_lang);
                    }

                    if let Some(last) = last_event
                        && last.elapsed() >= DEBOUNCE_DURATION {
                            last_event = None;
                            match crate::lang::Lang::reload() {
                                Ok(new_val) => {
                                    arc_swap.store(Arc::new(new_val));
                                    tracing::debug!("Reloaded config: Lang");
                                }
                                Err(e) => {
                                    tracing::error!("Failed to reload Lang: {}", e);
                                }
                            }
                        }
                }
            }
        }
    });
}

pub fn spawn_map_watcher(world: WorldHandle) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Event>();

    let dir = PathBuf::from("data/maps");
    let mut watcher: RecommendedWatcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        notify::Config::default(),
    )
    .expect("Failed to create map dir watcher");

    if dir.exists() {
        watcher
            .watch(&dir, RecursiveMode::NonRecursive)
            .unwrap_or_else(|_| panic!("Failed to watch maps dir: {:?}", dir));
    } else {
        tracing::warn!("Maps dir not found: {:?}", dir);
    }

    tokio::spawn(async move {
        let _watcher = watcher;
        let mut pending_maps: HashSet<i32> = HashSet::new();
        let mut last_event: Option<Instant> = None;
        let mut debounce_timer = tokio::time::interval(DEBOUNCE_DURATION);
        debounce_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        debounce_timer.tick().await;

        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    // Ignore reads (including reload_map's own open of the file),
                    // otherwise every reload would schedule another one forever.
                    if !is_map_content_change(&event.kind) {
                        continue;
                    }
                    for path in &event.paths {
                        if path.extension().is_some_and(|e| e == "emf")
                            && let Some(map_id) = extract_map_id(path) {
                                pending_maps.insert(map_id);
                                last_event = Some(Instant::now());
                            }
                    }
                }
                _ = debounce_timer.tick() => {
                    if let Some(last) = last_event
                        && last.elapsed() >= DEBOUNCE_DURATION {
                            last_event = None;
                            let maps_to_reload = std::mem::take(&mut pending_maps);
                            for map_id in maps_to_reload {
                                world.reload_map(map_id);
                                tracing::info!("Reloaded map: {}", map_id);
                            }
                        }
                }
            }
        }
    });
}

fn extract_map_id(path: &Path) -> Option<i32> {
    let stem = path.file_stem()?.to_str()?;
    let numeric: String = stem.chars().take_while(|c| c.is_ascii_digit()).collect();
    if numeric.is_empty() {
        return None;
    }
    numeric.parse::<i32>().ok()
}

/// True for events that can change a map file's contents: creation, data
/// writes, renames into place (editors that save via a temp file) and a
/// writer closing the file. Opens/reads and metadata-only changes are ignored.
fn is_map_content_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Create(_) => true,
        EventKind::Modify(ModifyKind::Metadata(_)) => false,
        EventKind::Modify(_) => true,
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use notify::event::{CreateKind, DataChange, MetadataKind, RemoveKind, RenameMode};

    use super::*;

    #[test]
    fn reads_do_not_trigger_map_reload() {
        assert!(!is_map_content_change(&EventKind::Access(AccessKind::Open(
            AccessMode::Any
        ))));
        assert!(!is_map_content_change(&EventKind::Access(
            AccessKind::Close(AccessMode::Read)
        )));
        assert!(!is_map_content_change(&EventKind::Access(AccessKind::Read)));
    }

    #[test]
    fn metadata_and_removal_do_not_trigger_map_reload() {
        assert!(!is_map_content_change(&EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::Any)
        )));
        assert!(!is_map_content_change(&EventKind::Remove(RemoveKind::File)));
    }

    #[test]
    fn writes_trigger_map_reload() {
        assert!(is_map_content_change(&EventKind::Create(CreateKind::File)));
        assert!(is_map_content_change(&EventKind::Modify(ModifyKind::Data(
            DataChange::Any
        ))));
        assert!(is_map_content_change(&EventKind::Modify(ModifyKind::Name(
            RenameMode::To
        ))));
        assert!(is_map_content_change(&EventKind::Access(
            AccessKind::Close(AccessMode::Write)
        )));
    }

    #[test]
    fn extracts_map_id_from_file_name() {
        assert_eq!(extract_map_id(Path::new("data/maps/00042.emf")), Some(42));
        assert_eq!(extract_map_id(Path::new("data/maps/readme.emf")), None);
    }
}
