use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{RwLock, broadcast};

// 游戏阶段枚举
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GamePhase {
  Waiting,
  Picking,
  Answering,
  Settlement,
}

// 玩家状态枚举
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlayerStatus {
  Waiting,
  Picking,
  Stopped,
  Answering,
  Submitted,
}

// 玩家数据结构
#[derive(Debug, Clone, Serialize)]
pub struct Player {
  pub id: String,
  pub color_hue: u16,
  pub status: PlayerStatus,
  pub obtained_indices: Vec<usize>,
  pub answer: Option<String>,
  pub is_online: bool,
  #[serde(skip)]
  pub last_seen: Instant,
}

// 游戏核心状态
pub struct GameState {
  pub phase: GamePhase,
  pub players: Vec<String>,
  pub player_map: HashMap<String, Player>,
  pub problem_text: Vec<char>,
  pub answer_text: String,
  pub hint_text: String,
  pub cursor: usize,
  pub current_turn_idx: usize,
  pub turn_deadline: Option<Instant>,
  pub answer_deadline: Option<Instant>,
  pub server_password: String,
}

// 内部消息广播
#[derive(Clone, Debug)]
pub enum InternalMsg {
  StateUpdated,
  Log(String),
}

// 应用共享状态
pub struct AppState {
  pub game: Arc<RwLock<GameState>>,
  pub tx: broadcast::Sender<InternalMsg>,
}

// 客户端上行消息
#[derive(Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ClientMsg {
  Heartbeat,
  Action { action: String },
  Answer { content: String },
}

// 下发给客户端的视图数据
#[derive(Serialize)]
pub struct ClientView {
  pub phase: GamePhase,
  pub hint: String,
  pub players: Vec<PlayerView>,
  pub grid: Vec<GridCell>,
  pub my_username: Option<String>,
  pub turn_deadline_ms: Option<u64>,
  pub answer_deadline_ms: Option<u64>,
  pub full_problem: Option<String>,
  pub correct_answer: Option<String>,
}

#[derive(Serialize)]
pub struct PlayerView {
  pub id: String,
  pub color_hue: u16,
  pub status: PlayerStatus,
  pub is_me: bool,
  pub is_online: bool,
  pub obtained_count: usize,
  pub answer: Option<String>,
}

#[derive(Serialize)]
pub struct GridCell {
  pub owner_color_hue: Option<u16>,
  pub char_content: Option<char>,
}
