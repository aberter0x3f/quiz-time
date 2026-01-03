use crate::models::*;
use chrono::Local;
use rand::seq::SliceRandom;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

pub struct ChainGame {
  pub problem_text: Vec<char>,
  pub answer_text: String,
  pub hint_text: String,
  pub phase: GamePhase,
  pub players: Vec<i64>,
  pub player_data: HashMap<i64, ChainPlayerState>,
  pub cursor: usize,
  pub current_turn_idx: usize,
  pub turn_deadline: Option<Instant>,
  pub answer_deadline: Option<Instant>,
}

pub struct ChainPlayerState {
  pub status: PlayerStatus,
  pub obtained_indices: Vec<usize>,
  pub answer: Option<String>,
}

impl ChainGame {
  pub fn new(prob: String, ans: String, hint: String) -> Self {
    Self {
      problem_text: prob.chars().collect(),
      answer_text: ans,
      hint_text: hint,
      phase: GamePhase::Waiting,
      players: vec![],
      player_data: HashMap::new(),
      cursor: 0,
      current_turn_idx: 0,
      turn_deadline: None,
      answer_deadline: None,
    }
  }

  pub fn setup_players(&mut self, users: Vec<(i64, u16)>) {
    for (pid, _) in users {
      self.players.push(pid);
      self.player_data.insert(
        pid,
        ChainPlayerState {
          status: PlayerStatus::Waiting,
          obtained_indices: vec![],
          answer: None,
        },
      );
    }
  }

  pub fn start(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    if self.players.is_empty() {
      return;
    }
    self.players.shuffle(&mut rand::thread_rng());
    self.phase = GamePhase::Picking;
    self.cursor = 0;
    self.current_turn_idx = 0;
    if let Some(first) = self.players.first() {
      if let Some(p) = self.player_data.get_mut(first) {
        p.status = PlayerStatus::Picking;
      }
    }
    self.turn_deadline = Some(Instant::now() + Duration::from_secs(3));
    let _ = tx.send(InternalMsg::Log {
      who: "System".into(),
      text: "Chain Game Started".into(),
      time: Local::now().format("%H:%M:%S").to_string(),
    });
  }

  pub fn handle_join(&mut self, _: i64, _: &broadcast::Sender<InternalMsg>) {}
  pub fn handle_leave(&mut self, _: i64, _: &broadcast::Sender<InternalMsg>) {}

  pub fn handle_action(&mut self, pid: i64, action: String, tx: &broadcast::Sender<InternalMsg>) {
    if self.phase == GamePhase::Picking && self.players.get(self.current_turn_idx) == Some(&pid) {
      if action == "take" {
        self.perform_take(tx);
      } else if action == "stop" {
        if let Some(p) = self.player_data.get_mut(&pid) {
          p.status = PlayerStatus::Stopped;
        }
        self.send_log(tx, "Action", format!("{} stopped", pid));
        self.advance_turn(tx);
      }
    }
  }

  pub fn handle_answer(&mut self, pid: i64, content: String, tx: &broadcast::Sender<InternalMsg>) {
    let can_answer = if let Some(p) = self.player_data.get(&pid) {
      self.phase == GamePhase::Answering
        || (self.phase == GamePhase::Picking && p.status == PlayerStatus::Stopped)
    } else {
      false
    };

    if can_answer {
      if let Some(p) = self.player_data.get_mut(&pid) {
        if p.status != PlayerStatus::Submitted {
          p.answer = Some(content);
          p.status = PlayerStatus::Submitted;
          self.send_log(tx, "System", format!("{} submitted answer", pid));
          self.check_all_submitted(tx);
          let _ = tx.send(InternalMsg::StateUpdated);
        }
      }
    }
  }

  pub fn tick(
    &mut self,
    tx: &broadcast::Sender<InternalMsg>,
    room_players: &HashMap<i64, super::room::RoomPlayer>,
  ) {
    let now = Instant::now();

    let player_ids = self.players.clone();

    for pid in player_ids {
      let is_online = room_players
        .get(&pid)
        .map(|rp| rp.is_online)
        .unwrap_or(false);

      if !is_online {
        // Drop handler logic for picking phase
        if self.phase == GamePhase::Picking && self.players.get(self.current_turn_idx) == Some(&pid)
        {
          if let Some(p) = self.player_data.get_mut(&pid) {
            p.status = PlayerStatus::Stopped;
          }
          self.advance_turn(tx);
        }
      }
    }

    if self.phase == GamePhase::Picking {
      if let Some(d) = self.turn_deadline {
        if now > d {
          self.perform_take(tx);
        }
      }
    }
    if self.phase == GamePhase::Answering {
      if let Some(d) = self.answer_deadline {
        if now > d {
          self.finish_game(tx);
        }
      }
      self.check_all_submitted(tx);
    }
  }

  fn perform_take(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    let curr_pid = self.players[self.current_turn_idx].clone();
    if self.cursor >= self.problem_text.len() {
      if let Some(p) = self.player_data.get_mut(&curr_pid) {
        p.status = PlayerStatus::Stopped;
      }
      self.advance_turn(tx);
      return;
    }
    if let Some(p) = self.player_data.get_mut(&curr_pid) {
      p.obtained_indices.push(self.cursor);
    }
    self.cursor += 1;
    self.turn_deadline = Some(Instant::now() + Duration::from_secs(3));
    let _ = tx.send(InternalMsg::StateUpdated);
  }

  fn advance_turn(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    let mut next_idx = (self.current_turn_idx + 1) % self.players.len();
    let mut found = false;
    for _ in 0..self.players.len() {
      let pid = &self.players[next_idx];
      if let Some(p) = self.player_data.get(pid) {
        if p.status == PlayerStatus::Waiting {
          found = true;
          break;
        }
      }
      next_idx = (next_idx + 1) % self.players.len();
    }

    let waiting_count = self
      .player_data
      .values()
      .filter(|p| p.status == PlayerStatus::Waiting)
      .count();

    if !found {
      self.enter_answering(tx);
    } else if waiting_count == 1 {
      let last_pid = self.players[next_idx].clone();
      let remaining = self.problem_text.len() - self.cursor;
      if remaining > 0 {
        if let Some(p) = self.player_data.get_mut(&last_pid) {
          for i in 0..remaining {
            p.obtained_indices.push(self.cursor + i);
          }
        }
        self.cursor += remaining;
      }
      if let Some(p) = self.player_data.get_mut(&last_pid) {
        p.status = PlayerStatus::Stopped;
      }
      self.enter_answering(tx);
    } else {
      self.current_turn_idx = next_idx;
      let next_pid = self.players[next_idx].clone();
      if let Some(p) = self.player_data.get_mut(&next_pid) {
        p.status = PlayerStatus::Picking;
      }
      self.turn_deadline = Some(Instant::now() + Duration::from_secs(3));
      let _ = tx.send(InternalMsg::StateUpdated);
    }
  }

  fn enter_answering(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    self.phase = GamePhase::Answering;
    self.turn_deadline = None;
    self.answer_deadline = Some(Instant::now() + Duration::from_secs(60));
    for p in self.player_data.values_mut() {
      if p.status != PlayerStatus::Submitted {
        p.status = PlayerStatus::Answering;
      }
    }
    self.send_log(tx, "System", "Picking ended. 60s to answer".into());
    let _ = tx.send(InternalMsg::StateUpdated);
  }

  fn check_all_submitted(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    if self
      .player_data
      .values()
      .all(|p| p.status == PlayerStatus::Submitted)
    {
      self.finish_game(tx);
    }
  }

  fn finish_game(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    if self.phase == GamePhase::Settlement {
      return;
    }
    self.phase = GamePhase::Settlement;
    self.turn_deadline = None;
    self.answer_deadline = None;
    self.send_log(tx, "System", "Game Finished".into());
    let _ = tx.send(InternalMsg::StateUpdated);
  }

  fn send_log(&self, tx: &broadcast::Sender<InternalMsg>, who: &str, text: String) {
    let _ = tx.send(InternalMsg::Log {
      who: who.to_string(),
      text,
      time: Local::now().format("%H:%M:%S").to_string(),
    });
  }

  pub fn get_view_data(
    &self,
    user_id: Option<i64>,
    show_all: bool,
    hue_map: &HashMap<i64, u16>,
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

    let mut grid = Vec::new();
    // Build index ownership map
    let mut idx_owner = HashMap::new();
    for (pid, p) in &self.player_data {
      for &idx in &p.obtained_indices {
        idx_owner.insert(idx, *pid);
      }
    }

    for i in 0..self.problem_text.len() {
      let owner_id = idx_owner.get(&i);
      let show_char = can_see_all || (user_id.is_some() && owner_id == user_id.as_ref());

      let hue = owner_id.and_then(|id| hue_map.get(id)).cloned();

      grid.push(GridCell {
        owner_color_hue: hue,
        char_content: if show_char {
          Some(self.problem_text[i])
        } else {
          None
        },
      });
    }

    let deadline = if self.phase == GamePhase::Picking {
      self.turn_deadline
    } else {
      self.answer_deadline
    };
    let correct_ans = if can_see_all {
      Some(self.answer_text.clone())
    } else {
      None
    };

    (
      self.phase,
      self.hint_text.clone(),
      deadline,
      Some(grid),
      None,
      None,
      correct_ans,
    )
  }

  pub fn get_player_state(
    &self,
    pid: i64,
    user_id: Option<i64>,
    show_all: bool,
  ) -> (PlayerStatus, Option<String>, bool, Option<String>) {
    if let Some(p) = self.player_data.get(&pid) {
      let is_active =
        self.phase == GamePhase::Picking && self.players.get(self.current_turn_idx) == Some(&pid);
      let score = format!("{}", p.obtained_indices.len());
      let show_ans = show_all || self.phase == GamePhase::Settlement || user_id == Some(pid);
      let ans = if show_ans { p.answer.clone() } else { None };

      (p.status, Some(score), is_active, ans)
    } else {
      (PlayerStatus::Waiting, None, false, None)
    }
  }
}
