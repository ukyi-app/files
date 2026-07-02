use crate::auth::KeyRegistry;
use crate::capacity::Capacity;
use crate::config::Config;
use crate::store::Store;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub keys: Arc<KeyRegistry>,
    pub cap: Capacity,
    pub cfg: Arc<Config>,
}

/// Config로부터 AppState 구성 — data_dir/.objects 생성, keys 로드, Store/Capacity 결선.
pub fn build_state(cfg: Config) -> std::io::Result<AppState> {
    std::fs::create_dir_all(cfg.data_dir.join(".objects"))?;
    let keys = KeyRegistry::load(&cfg.keys_path)?;
    let store = Store::new(cfg.data_dir.clone());
    let cap = Capacity::new(cfg.data_dir.clone(), cfg.min_free_bytes);
    Ok(AppState {
        store,
        keys: Arc::new(keys),
        cap,
        cfg: Arc::new(cfg),
    })
}
