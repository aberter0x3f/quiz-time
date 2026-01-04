use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum RoomType {
  Chain,
  Pinyin,
}

#[derive(
  Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString, Default,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum GamePhase {
  #[default]
  Waiting,
  Picking,   // Chain specific
  Answering, // Chain specific
  Gaming,    // Pinyin specific
  Settlement,
}

#[derive(
  Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString, Default,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum PlayerStatus {
  #[default]
  Waiting,
  Picking,   // Chain: active picker, Pinyin: active describer
  Answering, // Chain specific
  Submitted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InternalMsg {
  StateUpdated,
  Log {
    who: String,
    text: String,
    time: String,
  },
  Toast {
    to_user: i64,
    msg: String,
    kind: String,
  },
  Kick {
    target: i64,
  },
}

#[derive(Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ClientAction {
  Action { action: String },
  Answer { content: String },
}

#[derive(Serialize)]
pub struct RoomSummary {
  pub id: String,
  pub name: String,
  pub room_type: RoomType,
  pub phase: GamePhase,
  pub player_count: usize,
  pub max_players: usize,
}

#[derive(Serialize)]
pub struct ClientView {
  pub room_id: String,
  pub room_name: String,
  pub room_type: RoomType,
  pub phase: GamePhase,
  pub hint: String,
  pub deadline_ms: Option<u64>,
  pub is_admin: bool,
  // Only sent if is_admin is true
  #[serde(skip_serializing_if = "Option::is_none")]
  pub admin_ids: Option<Vec<i64>>,
  pub players: Vec<PlayerView>,
  pub max_players: usize,

  // Optional Game-Specific Data
  #[serde(skip_serializing_if = "Option::is_none")]
  pub grid: Option<Vec<GridCell>>, // Chain

  #[serde(skip_serializing_if = "Option::is_none")]
  pub pinyin_state: Option<PinyinSpecificView>, // Pinyin

  #[serde(skip_serializing_if = "Option::is_none")]
  pub winner: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub correct_answer: Option<String>,
}

#[derive(Serialize)]
pub struct PlayerView {
  pub id: i64,
  pub name: String,
  pub color_hue: u16,
  pub status: PlayerStatus,
  pub is_me: bool,
  pub is_online: bool,
  pub is_active_turn: bool,
  pub score_display: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub answer: Option<String>,
  pub is_spectator: bool,
  pub is_admin: bool,
}

// Chain Specific
#[derive(Serialize)]
pub struct GridCell {
  pub owner_color_hue: Option<u16>,
  pub char_content: Option<char>, // Strictly None if not allowed to see
}

// Pinyin Specific
#[derive(Serialize)]
pub struct PinyinSpecificView {
  pub all_initials: Vec<String>,
  pub all_finals: Vec<String>,
  pub banned_initials: Vec<String>,
  pub banned_finals: Vec<String>,
  pub history: Vec<PinyinHistoryItem>,
  pub my_prompt: Option<String>,
  pub is_first_turn: bool,
  pub is_guessing_turn: bool,
  pub end_message: Option<String>,
}

#[derive(Clone, Serialize, Debug)]
pub struct PinyinHistoryItem {
  pub player: i64,
  pub content: String,
  pub is_guess: bool,
}
