use super::GameLogic;
use crate::models::*;
use chrono::Local;
use rand::seq::SliceRandom;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

pub struct ChainGame {
  pub game_id: String,
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
  pub player_password: String,
  pub super_password: String,
}

impl ChainGame {
  fn send_log(&self, tx: &broadcast::Sender<InternalMsg>, who: &str, text: String) {
    let time_str = Local::now().format("%H:%M:%S").to_string();
    println!("[{}] {}: {}", time_str, who, text);
    let _ = tx.send(InternalMsg::Log(LogEntry {
      who: who.to_string(),
      text,
      time: time_str,
    }));
  }

  fn advance_turn(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    let mut next_idx = (self.current_turn_idx + 1) % self.players.len();
    let mut found_valid = false;
    for _ in 0..self.players.len() {
      let pid = &self.players[next_idx];
      if let Some(p) = self.player_map.get(pid) {
        if p.status == PlayerStatus::Waiting {
          found_valid = true;
          break;
        }
      }
      next_idx = (next_idx + 1) % self.players.len();
    }
    let waiting_count = self
      .player_map
      .values()
      .filter(|p| p.status == PlayerStatus::Waiting)
      .count();

    if !found_valid {
      self.enter_answering_phase(tx);
    } else if waiting_count == 1 {
      let last_pid = self.players[next_idx].clone();
      let remaining_len = self.problem_text.len() - self.cursor;
      if remaining_len > 0 {
        if let Some(p) = self.player_map.get_mut(&last_pid) {
          for i in 0..remaining_len {
            p.obtained_indices.push(self.cursor + i);
          }
        }
        self.cursor += remaining_len;
        self.send_log(
          tx,
          "System",
          format!("Player {} auto-received remaining chars.", last_pid),
        );
      }
      if let Some(p) = self.player_map.get_mut(&last_pid) {
        p.status = PlayerStatus::Stopped;
      }
      self.enter_answering_phase(tx);
    } else {
      self.current_turn_idx = next_idx;
      let next_pid = self.players[next_idx].clone();
      if let Some(p) = self.player_map.get_mut(&next_pid) {
        p.status = PlayerStatus::Picking;
      }
      self.turn_deadline = Some(Instant::now() + Duration::from_secs(3));
      let _ = tx.send(InternalMsg::StateUpdated);
    }
  }

  fn enter_answering_phase(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    self.phase = GamePhase::Answering;
    self.turn_deadline = None;
    self.answer_deadline = Some(Instant::now() + Duration::from_secs(60));
    for p in self.player_map.values_mut() {
      if p.status != PlayerStatus::Submitted {
        p.status = PlayerStatus::Answering;
      }
    }
    self.send_log(tx, "System", "Picking ended, 60s to answer.".to_string());
    let _ = tx.send(InternalMsg::StateUpdated);
    self.check_all_submitted(tx);
  }

  fn check_all_submitted(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    if self.phase != GamePhase::Answering {
      return;
    }
    let all_done = self.players.iter().all(|id| match self.player_map.get(id) {
      Some(p) => p.status == PlayerStatus::Submitted || !p.is_online,
      None => true,
    });
    if all_done {
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
    self.send_log(tx, "System", "Game finished.".to_string());
    let _ = tx.send(InternalMsg::StateUpdated);
    tokio::spawn(async {
      tokio::time::sleep(Duration::from_secs(5)).await;
      std::process::exit(0);
    });
  }

  fn perform_take(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    let current_id = self.players[self.current_turn_idx].clone();
    if self.cursor >= self.problem_text.len() {
      if let Some(p) = self.player_map.get_mut(&current_id) {
        p.status = PlayerStatus::Stopped;
      }
      self.advance_turn(tx);
      return;
    }
    if let Some(p) = self.player_map.get_mut(&current_id) {
      p.obtained_indices.push(self.cursor);
    }
    self.cursor += 1;
    self.turn_deadline = Some(Instant::now() + Duration::from_secs(3));
    let _ = tx.send(InternalMsg::StateUpdated);
  }
}

impl GameLogic for ChainGame {
  fn handle_join(&mut self, pid: String, tx: &broadcast::Sender<InternalMsg>) {
    if self.phase != GamePhase::Waiting && !self.player_map.contains_key(&pid) {
      return;
    }
    if !self.player_map.contains_key(&pid) {
      self.players.push(pid.clone());
      self.player_map.insert(
        pid.clone(),
        Player {
          id: pid.clone(),
          color_hue: 0,
          status: PlayerStatus::Waiting,
          obtained_indices: vec![],
          answer: None,
          is_online: true,
          last_seen: Instant::now(),
        },
      );
      self.send_log(tx, "System", format!("{} joined.", pid));
      let _ = tx.send(InternalMsg::StateUpdated);
    } else {
      if let Some(p) = self.player_map.get_mut(&pid) {
        p.last_seen = Instant::now();
        if !p.is_online {
          p.is_online = true;
          self.send_log(tx, "System", format!("{} reconnected.", pid));
          let _ = tx.send(InternalMsg::StateUpdated);
        }
      }
    }
  }

  fn handle_leave(&mut self, pid: &str, tx: &broadcast::Sender<InternalMsg>) {
    if let Some(p) = self.player_map.get_mut(pid) {
      p.is_online = false;
      p.last_seen = Instant::now();
    }
    if self.phase == GamePhase::Waiting {
      self.players.retain(|x| x != pid);
      self.player_map.remove(pid);
      self.send_log(tx, "System", format!("{} left.", pid));
    } else {
      self.send_log(tx, "System", format!("{} disconnected.", pid));
    }
    let _ = tx.send(InternalMsg::StateUpdated);
  }

  fn handle_action(&mut self, pid: &str, act: String, tx: &broadcast::Sender<InternalMsg>) {
    if self.phase == GamePhase::Picking
      && self.players.get(self.current_turn_idx).map(|s| s.as_str()) == Some(pid)
    {
      if act == "take" {
        self.perform_take(tx);
      } else if act == "stop" {
        if let Some(p) = self.player_map.get_mut(pid) {
          p.status = PlayerStatus::Stopped;
        }
        self.send_log(tx, "Action", format!("{} stopped.", pid));
        self.advance_turn(tx);
      }
    }
  }

  fn handle_answer(&mut self, pid: &str, cnt: String, tx: &broadcast::Sender<InternalMsg>) {
    let can = if let Some(p) = self.player_map.get(pid) {
      self.phase == GamePhase::Answering
        || (self.phase == GamePhase::Picking && p.status == PlayerStatus::Stopped)
    } else {
      false
    };
    if can {
      if let Some(p) = self.player_map.get_mut(pid) {
        if p.status != PlayerStatus::Submitted {
          p.answer = Some(cnt);
          p.status = PlayerStatus::Submitted;
          self.send_log(tx, "System", format!("{} submitted.", pid));
          self.check_all_submitted(tx);
          let _ = tx.send(InternalMsg::StateUpdated);
        }
      }
    }
  }

  fn tick(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    let now = Instant::now();
    if self.phase != GamePhase::Waiting && self.phase != GamePhase::Settlement {
      let mut timed_out = vec![];
      for (id, p) in &self.player_map {
        if !p.is_online && now.duration_since(p.last_seen) > Duration::from_secs(30) {
          timed_out.push(id.clone());
        }
      }
      for id in timed_out {
        if let Some(p) = self.player_map.get_mut(&id) {
          if p.status != PlayerStatus::Submitted {
            p.status = PlayerStatus::Submitted;
          }
        }
        if self.phase == GamePhase::Picking && self.players.get(self.current_turn_idx) == Some(&id)
        {
          if let Some(p) = self.player_map.get_mut(&id) {
            p.status = PlayerStatus::Stopped;
          }
          self.advance_turn(tx);
        }
      }
      if self.phase == GamePhase::Answering {
        self.check_all_submitted(tx);
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
    }
  }

  fn start_game(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    if self.phase != GamePhase::Waiting {
      return;
    }
    let onlines: Vec<String> = self
      .players
      .iter()
      .filter(|id| self.player_map.get(*id).map_or(false, |p| p.is_online))
      .cloned()
      .collect();
    if onlines.is_empty() {
      return;
    }
    self.players = onlines;
    self.players.shuffle(&mut rand::thread_rng());
    for (i, id) in self.players.iter().enumerate() {
      if let Some(p) = self.player_map.get_mut(id) {
        p.color_hue = ((i * 360) / self.players.len()) as u16;
        p.status = PlayerStatus::Waiting;
      }
    }
    self.phase = GamePhase::Picking;
    self.cursor = 0;
    self.current_turn_idx = 0;
    self.turn_deadline = Some(Instant::now() + Duration::from_secs(3));
    if let Some(first) = self.players.first() {
      if let Some(p) = self.player_map.get_mut(first) {
        p.status = PlayerStatus::Picking;
      }
    }
    self.send_log(tx, "System", "Game Started.".to_string());
    let _ = tx.send(InternalMsg::StateUpdated);
  }

  fn get_view(&self, user: Option<&str>, is_super: bool) -> ClientView {
    let now = Instant::now();
    let p_views = self
      .players
      .iter()
      .map(|id| {
        let p = &self.player_map[id];
        let is_me = user == Some(id);
        let show_ans = is_super || self.phase == GamePhase::Settlement || is_me;
        PlayerView {
          id: id.clone(),
          color_hue: p.color_hue,
          status: p.status.clone(),
          is_me,
          is_online: p.is_online,
          extra_info: None,
          score_display: Some(format!("{}", p.obtained_indices.len())),
          is_active_turn: self.phase == GamePhase::Picking
            && self.players.get(self.current_turn_idx) == Some(id),
          answer: if show_ans { p.answer.clone() } else { None },
        }
      })
      .collect();

    let mut grid = Vec::new();
    let mut idx_owners = HashMap::new();
    for (pid, p) in &self.player_map {
      for &idx in &p.obtained_indices {
        idx_owners.insert(idx, pid.clone());
      }
    }
    for i in 0..self.problem_text.len() {
      let owner = idx_owners.get(&i);
      let mut color = None;
      let mut char_c = None;
      if let Some(oid) = owner {
        if let Some(p) = self.player_map.get(oid) {
          color = Some(p.color_hue);
          if is_super || self.phase == GamePhase::Settlement || user == Some(oid) {
            char_c = Some(self.problem_text[i]);
          }
        }
      }
      grid.push(GridCell {
        owner_color_hue: color,
        char_content: char_c,
      });
    }

    ClientView {
      game_id: self.game_id.clone(),
      phase: self.phase.clone(),
      hint: self.hint_text.clone(),
      players: p_views,
      grid: Some(grid),
      deadline_ms: self
        .turn_deadline
        .or(self.answer_deadline)
        .map(|t| t.saturating_duration_since(now).as_millis() as u64),
      is_super,
      correct_answer: if is_super || self.phase == GamePhase::Settlement {
        Some(self.answer_text.clone())
      } else {
        None
      },
      ..Default::default()
    }
  }

  fn get_passwords(&self) -> (String, String) {
    (self.player_password.clone(), self.super_password.clone())
  }
}
