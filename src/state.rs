use crate::auth::User;
use crate::conf::Config;
use crate::game::InternalMsg;
use crate::game::{pinyin_utils::PinyinTable, room::Room};
use anyhow::Result;
use dashmap::DashMap;
use std::{fs, sync::Arc};
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

pub struct AppState {
  pub config: Config,
  pub users: DashMap<i64, User>,
  // RwLock 允许对房间进行内部修改，DashMap 处理并发访问
  pub rooms: DashMap<Uuid, Arc<RwLock<Room>>>,
  pub pinyin_table: Arc<PinyinTable>,
  // 全局广播通道 (用于系统级通知，房间有自己的通道)
  pub global_tx: broadcast::Sender<InternalMsg>,
  pub oauth_client: crate::auth::oauth::Client,
  pub token_manager: crate::auth::token::TokenManager,
}

impl AppState {
  pub fn new() -> Result<Self> {
    let config = Config::load();

    let users_json = fs::read_to_string("users.json").unwrap();
    let users_list: Vec<User> = serde_json::from_str(&users_json)?;
    let users_map = DashMap::new();
    for u in users_list {
      users_map.insert(u.id, u);
    }

    let pinyin_table = Arc::new(crate::game::pinyin_utils::load_pinyin_table("dict.txt"));
    let oauth_client = crate::auth::oauth::init_oauth_client(&config);
    let token_manager = crate::auth::token::TokenManager::new();
    let (tx, _) = broadcast::channel(1);

    Ok(Self {
      config,
      users: users_map,
      rooms: DashMap::new(),
      pinyin_table,
      global_tx: tx,
      oauth_client,
      token_manager,
    })
  }
}
