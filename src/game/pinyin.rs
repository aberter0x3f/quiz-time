use super::pinyin_utils::{PinyinTable, get_text_components, validate_char};
use crate::models::*;
use chrono::Local;
use rand::seq::SliceRandom;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

pub struct PinyinGame {
  pub answer: String,
  pub hint: String,
  pub table: Arc<PinyinTable>,
  pub phase: GamePhase,

  pub players: Vec<i64>,
  pub player_data: HashMap<i64, PinyinPlayerState>,

  pub current_idx: usize,
  pub turn_deadline: Option<Instant>,

  pub history: Vec<PinyinHistoryItem>,
  pub banned_i: HashSet<String>,
  pub banned_f: HashSet<String>,

  // Logic flags
  pub is_first_describer: bool,
  pub current_prompt: String,
  pub answer_i: HashSet<String>,
  pub answer_f: HashSet<String>,
  pub all_i: Vec<String>,
  pub all_f: Vec<String>,

  pub winner: bool,
}

pub struct PinyinPlayerState {
  pub status: PlayerStatus,
}

impl PinyinGame {
  pub fn new(ans: String, hint: String, table: Arc<PinyinTable>) -> Self {
    let (ai, af) = get_text_components(&ans, &table);

    // 预计算所有声韵母供前端显示
    let mut distinct_i = HashSet::new();
    let mut distinct_f = HashSet::new();
    for (i, f) in table.values() {
      distinct_i.insert(i.clone());
      distinct_f.insert(f.clone());
    }
    let mut v_i: Vec<_> = distinct_i.into_iter().collect();
    v_i.sort();
    let mut v_f: Vec<_> = distinct_f.into_iter().collect();
    v_f.sort();

    Self {
      answer: ans.clone(),
      hint,
      table,
      phase: GamePhase::Waiting,
      players: vec![],
      player_data: HashMap::new(),
      current_idx: 0,
      turn_deadline: None,
      history: vec![],
      banned_i: HashSet::new(),
      banned_f: HashSet::new(),
      is_first_describer: true,
      current_prompt: ans,
      answer_i: ai,
      answer_f: af,
      all_i: v_i,
      all_f: v_f,
      winner: false,
    }
  }

  pub fn setup_players(&mut self, users: Vec<(i64, u16)>) {
    for (pid, _) in users {
      self.players.push(pid.clone());
      self.player_data.insert(
        pid,
        PinyinPlayerState {
          status: PlayerStatus::Waiting,
        },
      );
    }
  }

  pub fn start(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    if self.players.is_empty() {
      return;
    }
    self.players.shuffle(&mut rand::thread_rng());
    self.phase = GamePhase::Gaming;
    self.current_idx = 0;
    self.current_prompt = self.answer.clone();
    self.is_first_describer = true;
    self.banned_i.clear();
    self.banned_f.clear();
    self.history.clear();

    if let Some(first) = self.players.first() {
      if let Some(p) = self.player_data.get_mut(first) {
        p.status = PlayerStatus::Picking; // Active
      }
    }
    self.turn_deadline = Some(Instant::now() + Duration::from_secs(180));
    let _ = tx.send(InternalMsg::Log {
      who: "System".into(),
      text: "Pinyin Game Started".into(),
      time: Local::now().format("%H:%M:%S").to_string(),
    });
  }

  pub fn handle_join(&mut self, _: i64, _: &broadcast::Sender<InternalMsg>) {}
  pub fn handle_leave(&mut self, _: i64, _: &broadcast::Sender<InternalMsg>) {}
  pub fn handle_action(&mut self, _: i64, _: String, _: &broadcast::Sender<InternalMsg>) {}

  pub fn handle_answer(&mut self, pid: i64, content: String, tx: &broadcast::Sender<InternalMsg>) {
    if self.phase != GamePhase::Gaming {
      return;
    }
    if self.players.get(self.current_idx) != Some(&pid) {
      return;
    }
    if content.trim().is_empty() {
      return;
    }

    let is_guesser = self.current_idx == self.players.len() - 1;

    if is_guesser {
      let win = content == self.answer;
      self.history.push(PinyinHistoryItem {
        player: pid,
        content: content.clone(),
        is_guess: true,
      });
      self.finish(tx, win);
    } else {
      // Validate Pinyin
      for c in content.chars() {
        if let Err(e) = validate_char(c, &self.table, &self.banned_i, &self.banned_f) {
          let _ = tx.send(InternalMsg::Toast {
            to_user: pid,
            msg: e,
            kind: "error".into(),
          });
          return;
        }
        if self.is_first_describer {
          let (i, f) = &self.table[&c];
          if self.answer_i.contains(i) || self.answer_f.contains(f) {
            let _ = tx.send(InternalMsg::Toast {
              to_user: pid,
              msg: format!("Char '{}' invalid (in answer)", c),
              kind: "error".into(),
            });
            return;
          }
        }
      }

      // Update State
      let (ni, nf) = get_text_components(&content, &self.table);
      self.banned_i.extend(ni);
      self.banned_f.extend(nf);

      self.history.push(PinyinHistoryItem {
        player: pid,
        content: content.clone(),
        is_guess: false,
      });
      self.current_prompt = content;
      self.is_first_describer = false;

      if let Some(p) = self.player_data.get_mut(&pid) {
        p.status = PlayerStatus::Submitted;
      }
      self.advance_turn(tx);
    }
  }

  fn advance_turn(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    self.current_idx += 1;
    if self.current_idx >= self.players.len() {
      self.finish(tx, false);
      return;
    }
    let next = &self.players[self.current_idx];
    if let Some(p) = self.player_data.get_mut(next) {
      p.status = PlayerStatus::Picking;
    }

    // Check "Prompt loop" logic: if prompt == answer (reset), first describer logic applies?
    // Original logic: if current_input_prompt == answer_text { is_first_describer = true }
    if self.current_prompt == self.answer {
      self.is_first_describer = true;
    }

    self.turn_deadline = Some(Instant::now() + Duration::from_secs(180));
    let _ = tx.send(InternalMsg::StateUpdated);
  }

  fn finish(&mut self, tx: &broadcast::Sender<InternalMsg>, win: bool) {
    self.phase = GamePhase::Settlement;
    self.winner = win;
    self.turn_deadline = None;
    let _ = tx.send(InternalMsg::StateUpdated);
  }

  pub fn tick(
    &mut self,
    tx: &broadcast::Sender<InternalMsg>,
    room_players: &HashMap<i64, super::room::RoomPlayer>,
  ) {
    if self.phase == GamePhase::Gaming {
      if self.current_idx >= self.players.len() {
        return;
      }
      let curr = &self.players[self.current_idx];
      let is_online = room_players.get(curr).map(|p| p.is_online).unwrap_or(false);

      let mut timeout = false;
      if !is_online {
        timeout = true;
      }
      if let Some(d) = self.turn_deadline {
        if Instant::now() > d {
          timeout = true;
        }
      }

      if timeout {
        // Timeout logic
        let is_guesser = self.current_idx == self.players.len() - 1;
        self.history.push(PinyinHistoryItem {
          player: curr.clone(),
          content: "(Timeout)".into(),
          is_guess: is_guesser,
        });
        if is_guesser {
          self.finish(tx, false);
        } else {
          self.advance_turn(tx);
        }
      }
    }
  }

  pub fn get_view_data(
    &self,
    user_id: Option<i64>,
    show_all: bool,
    _hue_map: &HashMap<i64, u16>,
  ) -> (
    GamePhase,
    String,
    Option<Instant>,
    Option<Vec<GridCell>>,
    Option<PinyinSpecificView>,
    Option<bool>,
    Option<String>,
  ) {
    let is_settled = self.phase == GamePhase::Settlement;
    let can_see_all = show_all || is_settled;

    // Visibility Logic for Bans:
    // 1. Super/Settled -> All
    // 2. Player: if my_idx <= current_idx (Past or Current) -> See Bans. Future -> Don't see.
    // 3. Spectator -> All
    let mut show_bans = can_see_all;
    if !show_bans && user_id.is_some() {
      if let Some(u) = user_id {
        if let Some(my_idx) = self.players.iter().position(|p| *p == u) {
          if my_idx <= self.current_idx {
            show_bans = true;
          }
        } else {
          // Authenticated user but not playing (spectator)
          show_bans = true;
        }
      }
    }

    let mut b_i = if show_bans {
      self.banned_i.iter().cloned().collect()
    } else {
      vec![]
    };
    let mut b_f = if show_bans {
      self.banned_f.iter().cloned().collect()
    } else {
      vec![]
    };

    // First describer strict logic: if I am the first describer, show answer bans in the ban list
    if self.phase == GamePhase::Gaming && self.is_first_describer && user_id.is_some() {
      if self.players.get(self.current_idx) == user_id.as_ref() {
        b_i.extend(self.answer_i.clone());
        b_f.extend(self.answer_f.clone());
      }
    }
    b_i.sort();
    b_f.sort();

    // History Visibility:
    // Same logic: Past/Current players see history. Future don't.
    let mut visible_history = vec![];
    if can_see_all {
      visible_history = self.history.clone();
    } else if let Some(u) = user_id {
      if let Some(my_idx) = self.players.iter().position(|p| *p == u) {
        if my_idx <= self.current_idx {
          visible_history = self.history.clone();
        }
      } else {
        visible_history = self.history.clone();
      }
    }

    let my_prompt = if user_id.is_some()
      && self.phase == GamePhase::Gaming
      && self.players.get(self.current_idx) == user_id.as_ref()
    {
      Some(self.current_prompt.clone())
    } else {
      None
    };

    let pinyin_state = PinyinSpecificView {
      all_initials: self.all_i.clone(),
      all_finals: self.all_f.clone(),
      banned_initials: b_i,
      banned_finals: b_f,
      history: visible_history,
      my_prompt,
      is_first_turn: self.is_first_describer,
      is_guessing_turn: self.players.len() > 0 && self.current_idx == self.players.len() - 1,
      end_message: if is_settled {
        Some(if self.winner {
          "Success".into()
        } else {
          "Failed".into()
        })
      } else {
        None
      },
    };

    (
      self.phase,
      self.hint.clone(),
      self.turn_deadline,
      None,
      Some(pinyin_state),
      Some(self.winner),
      if can_see_all {
        Some(self.answer.clone())
      } else {
        None
      },
    )
  }

  pub fn get_player_state(
    &self,
    pid: i64,
    _user_id: Option<i64>,
    _show_all: bool,
  ) -> (PlayerStatus, Option<String>, bool, Option<String>) {
    let p_status = self
      .player_data
      .get(&pid)
      .map(|p| p.status)
      .unwrap_or(PlayerStatus::Waiting);
    let is_active =
      self.phase == GamePhase::Gaming && self.players.get(self.current_idx) == Some(&pid);

    // In Pinyin, rounds are the score equivalent
    let round_idx = self.players.iter().position(|p| *p == pid).unwrap_or(0);
    let role = if round_idx == self.players.len().saturating_sub(1) {
      "Guesser"
    } else {
      "Describer"
    };

    (p_status, Some(role.to_string()), is_active, None)
  }
}
