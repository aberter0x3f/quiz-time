use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum GamePhase {
  #[default]
  Waiting,
  Picking,   // Chain specific
  Answering, // Chain specific
  Settlement,
  Gaming, // Pinyin specific
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlayerStatus {
  Waiting,
  Picking, // Used as "Active Turn" in Pinyin
  Stopped,
  Answering,
  Submitted,
}

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

#[derive(Clone, Debug, Serialize)]
pub struct LogEntry {
  pub who: String,
  pub text: String,
  pub time: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToastMsg {
  pub to_user: String,
  pub msg: String,
  pub kind: String, // "info" or "error"
}

#[derive(Clone, Debug)]
pub enum InternalMsg {
  StateUpdated,
  Log(LogEntry),
  Toast(ToastMsg),
}

#[derive(Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ClientMsg {
  Heartbeat,
  Action { action: String },
  Answer { content: String },
}

#[derive(Serialize, Default)]
pub struct ClientView {
  pub game_id: String,
  pub phase: GamePhase,
  pub hint: String,
  pub players: Vec<PlayerView>,
  pub deadline_ms: Option<u64>,
  pub is_super: bool,
  pub correct_answer: Option<String>,

  // Chain Specific
  #[serde(skip_serializing_if = "Option::is_none")]
  pub grid: Option<Vec<GridCell>>,

  // Pinyin Specific
  #[serde(skip_serializing_if = "Option::is_none")]
  pub all_initials: Option<Vec<String>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub all_finals: Option<Vec<String>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub banned_initials: Option<Vec<String>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub banned_finals: Option<Vec<String>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub history: Option<Vec<crate::game::pinyin::PinyinHistoryItem>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub my_prompt: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub is_first_turn: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub is_guessing_turn: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub full_history: Option<Vec<crate::game::pinyin::PinyinHistoryItem>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub winner: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub end_message: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub current_player_id: Option<String>,
}

#[derive(Serialize)]
pub struct PlayerView {
  pub id: String,
  pub color_hue: u16,
  pub status: PlayerStatus,
  pub is_me: bool,
  pub is_online: bool,
  pub score_display: Option<String>,
  pub extra_info: Option<String>,
  pub is_active_turn: bool,
  pub answer: Option<String>,
}

#[derive(Serialize)]
pub struct GridCell {
  pub owner_color_hue: Option<u16>,
  pub char_content: Option<char>,
}
