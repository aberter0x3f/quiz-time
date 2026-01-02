use crate::game::GameMode;
use crate::models::InternalMsg;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};

pub struct AppState {
  pub game: Arc<RwLock<GameMode>>,
  pub tx: broadcast::Sender<InternalMsg>,
  pub is_pinyin: bool,
}
