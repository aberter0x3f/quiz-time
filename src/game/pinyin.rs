use super::{GameLogic, generate_random_id, pinyin_utils::*};
use crate::models::*;
use chrono::Local;
use rand::seq::SliceRandom;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

#[derive(Clone, Serialize, Debug)]
pub struct PinyinHistoryItem {
  pub player: String,
  pub content: String,
  pub is_guess: bool,
}

pub struct PinyinGame {
  pub game_id: String,
  pub phase: GamePhase,
  pub players: Vec<String>,
  pub player_map: HashMap<String, Player>,
  pub pinyin_table: PinyinTable,
  pub answer_text: String,
  pub hint_text: String,
  pub current_player_idx: usize,
  pub turn_deadline: Option<Instant>,
  pub history: Vec<PinyinHistoryItem>,
  pub is_first_describer: bool,
  pub current_input_prompt: String,
  pub banned_initials: HashSet<String>,
  pub banned_finals: HashSet<String>,
  pub answer_initials: HashSet<String>,
  pub answer_finals: HashSet<String>,
  pub all_initials: Vec<String>,
  pub all_finals: Vec<String>,
  pub player_password: String,
  pub super_password: String,
  pub winner: bool,
}

impl PinyinGame {
  pub fn new(ans: String, hint: String, table: PinyinTable, pp: String, sp: String) -> Self {
    let (ans_i, ans_f) = get_text_components(&ans, &table);

    // 收集所有可能的声韵母用于前端显示
    let mut all_i = HashSet::new();
    let mut all_f = HashSet::new();
    for (i, f) in table.values() {
      all_i.insert(i.clone());
      all_f.insert(f.clone());
    }
    let mut all_i_vec: Vec<_> = all_i.into_iter().collect();
    let mut all_f_vec: Vec<_> = all_f.into_iter().collect();
    all_i_vec.sort();
    all_f_vec.sort();

    Self {
      game_id: generate_random_id(),
      phase: GamePhase::Waiting,
      players: vec![],
      player_map: HashMap::new(),
      pinyin_table: table,
      answer_text: ans.clone(),
      hint_text: hint,
      current_player_idx: 0,
      turn_deadline: None,
      history: vec![],
      is_first_describer: true,
      current_input_prompt: ans, // 初始提示就是答案
      banned_initials: HashSet::new(),
      banned_finals: HashSet::new(),
      answer_initials: ans_i,
      answer_finals: ans_f,
      all_initials: all_i_vec,
      all_finals: all_f_vec,
      player_password: pp,
      super_password: sp,
      winner: false,
    }
  }

  fn send_log(&self, tx: &broadcast::Sender<InternalMsg>, who: &str, text: String) {
    let time_str = Local::now().format("%H:%M:%S").to_string();
    println!("[{}] {}: {}", time_str, who, text);
    let _ = tx.send(InternalMsg::Log(LogEntry {
      who: who.to_string(),
      text,
      time: time_str,
    }));
  }

  fn send_toast(&self, tx: &broadcast::Sender<InternalMsg>, to: &str, msg: String, err: bool) {
    let _ = tx.send(InternalMsg::Toast(ToastMsg {
      to_user: to.to_string(),
      msg,
      kind: if err { "error".into() } else { "info".into() },
    }));
  }

  fn finish_game(&mut self, tx: &broadcast::Sender<InternalMsg>, win: bool) {
    self.phase = GamePhase::Settlement;
    self.winner = win;
    self.turn_deadline = None;
    self.send_log(
      tx,
      "System",
      format!("Game Over. Result: {}", if win { "Win" } else { "Loss" }),
    );
    let _ = tx.send(InternalMsg::StateUpdated);
    tokio::spawn(async {
      tokio::time::sleep(Duration::from_secs(5)).await;
      std::process::exit(0);
    });
  }

  fn advance_turn(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    // 更新当前玩家状态
    if let Some(curr_pid) = self.players.get(self.current_player_idx) {
      if let Some(p) = self.player_map.get_mut(curr_pid) {
        p.status = PlayerStatus::Submitted;
      }
    }

    self.current_player_idx += 1;

    // 检查是否所有人都结束了
    if self.current_player_idx >= self.players.len() {
      self.finish_game(tx, false); // 没人猜对或最后一人超时
      return;
    }

    let next_pid = self.players[self.current_player_idx].clone();
    if let Some(p) = self.player_map.get_mut(&next_pid) {
      p.status = PlayerStatus::Picking; // 复用 Picking 为 Active
    }

    if self.current_input_prompt == self.answer_text {
      self.is_first_describer = true;
    }

    self.turn_deadline = Some(Instant::now() + Duration::from_secs(180));
    let _ = tx.send(InternalMsg::StateUpdated);
  }

  fn handle_timeout(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    let pid = self.players[self.current_player_idx].clone();

    // 如果是最后一人（Guesser）超时 -> 输
    if self.current_player_idx == self.players.len() - 1 {
      self.history.push(PinyinHistoryItem {
        player: pid,
        content: "(Timeout)".into(),
        is_guess: true,
      });
      self.finish_game(tx, false);
    } else {
      // 中间的人超时 -> 跳过，下一个人接手当前 prompt
      self.history.push(PinyinHistoryItem {
        player: pid.clone(),
        content: "(Timeout/Skipped)".into(),
        is_guess: false,
      });
      self.send_log(tx, "System", format!("Player {} timed out. Skipping.", pid));
      // prompt 不变
      self.advance_turn(tx);
    }
  }
}

impl GameLogic for PinyinGame {
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

  fn handle_action(&mut self, _: &str, _: String, _: &broadcast::Sender<InternalMsg>) {}

  fn handle_answer(&mut self, pid: &str, content: String, tx: &broadcast::Sender<InternalMsg>) {
    if self.phase != GamePhase::Gaming {
      return;
    }
    let curr_pid = &self.players[self.current_player_idx];
    if pid != curr_pid {
      return;
    }

    // 内容空检查
    if content.trim().is_empty() {
      self.send_toast(tx, pid, "Content cannot be empty.".into(), true);
      return;
    }

    let is_last_player = self.current_player_idx == self.players.len() - 1;

    if is_last_player {
      // Guesser: 无拼音限制，猜对即赢
      if content == self.answer_text {
        self.history.push(PinyinHistoryItem {
          player: pid.to_string(),
          content,
          is_guess: true,
        });
        self.finish_game(tx, true);
      } else {
        self.history.push(PinyinHistoryItem {
          player: pid.to_string(),
          content,
          is_guess: true,
        });
        self.finish_game(tx, false);
      }
    } else {
      // Describer: 验证拼音
      for c in content.chars() {
        if let Err(e) = validate_char(
          c,
          &self.pinyin_table,
          &self.banned_initials,
          &self.banned_finals,
        ) {
          self.send_toast(tx, pid, e, true);
          return;
        }
        // 第一棒（或因超时继承第一棒规则的人）不能使用答案的拼音
        if self.is_first_describer {
          let (i, f) = &self.pinyin_table[&c];
          if self.answer_initials.contains(i) || self.answer_finals.contains(f) {
            self.send_toast(
              tx,
              pid,
              format!("Forbidden char '{}' (part of answer components).", c),
              true,
            );
            return;
          }
        }
      }

      // 更新 Ban List
      let (new_i, new_f) = get_text_components(&content, &self.pinyin_table);
      self.banned_initials.extend(new_i);
      self.banned_finals.extend(new_f);

      self.history.push(PinyinHistoryItem {
        player: pid.to_string(),
        content: content.clone(),
        is_guess: false,
      });
      self.current_input_prompt = content;
      self.is_first_describer = false;
      self.send_log(tx, "Game", format!("{} finished turn.", pid));
      self.advance_turn(tx);
    }
  }

  fn tick(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    let now = Instant::now();
    if self.phase == GamePhase::Gaming {
      if self.current_player_idx >= self.players.len() {
        return;
      }
      let curr_pid = self.players[self.current_player_idx].clone();
      let is_offline = self
        .player_map
        .get(&curr_pid)
        .map_or(true, |p| !p.is_online);

      if is_offline {
        self.send_log(
          tx,
          "System",
          format!("Player {} offline. Skipping.", curr_pid),
        );
        self.handle_timeout(tx);
      } else if let Some(d) = self.turn_deadline {
        if now > d {
          self.send_log(tx, "System", format!("Player {} timed out.", curr_pid));
          self.handle_timeout(tx);
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
      println!("Cannot start: no players online");
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
    self.phase = GamePhase::Gaming;
    self.current_player_idx = 0;
    self.current_input_prompt = self.answer_text.clone();
    self.is_first_describer = true;
    self.banned_initials.clear();
    self.banned_finals.clear();
    self.history.clear();

    if let Some(first) = self.players.first() {
      if let Some(p) = self.player_map.get_mut(first) {
        p.status = PlayerStatus::Picking; // Active
      }
    }
    self.turn_deadline = Some(Instant::now() + Duration::from_secs(180));
    self.send_log(tx, "System", "Pinyin Game Started.".to_string());
    let _ = tx.send(InternalMsg::StateUpdated);
  }

  fn get_view(&self, user: Option<&str>, is_super: bool) -> ClientView {
    let now = Instant::now();
    let p_views = self
      .players
      .iter()
      .enumerate()
      .map(|(idx, id)| {
        let p = &self.player_map[id];
        let is_me = user == Some(id);
        let is_active = self.phase == GamePhase::Gaming && idx == self.current_player_idx;
        PlayerView {
          id: id.clone(),
          color_hue: p.color_hue,
          status: p.status.clone(),
          is_me,
          is_online: p.is_online,
          extra_info: if idx == self.players.len() - 1 {
            Some("Guesser".to_string())
          } else {
            Some(format!("Round {}", idx + 1))
          },
          score_display: None,
          is_active_turn: is_active,
          answer: None,
        }
      })
      .collect();

    let mut visible_history = vec![];
    let mut my_prompt = None;
    let mut is_first_turn = false;
    let mut is_guessing_turn = false;

    let me_idx = if let Some(u) = user {
      self.players.iter().position(|r| r == u)
    } else {
      None
    };

    let is_settled = self.phase == GamePhase::Settlement;

    if is_settled || is_super {
      visible_history = self.history.clone();
    } else {
      // 普通玩家视角
      if let Some(midx) = me_idx {
        // 如果我已经行动过（在当前玩家之前），我可以看到历史
        if midx < self.current_player_idx {
          visible_history = self.history.clone();
        }
        // 轮到我了
        if midx == self.current_player_idx && self.phase == GamePhase::Gaming {
          my_prompt = Some(self.current_input_prompt.clone());
          is_first_turn = self.is_first_describer;
          is_guessing_turn = midx == self.players.len() - 1;
        }
      }
    }

    // 处理 Banned Initials/Finals 的显示逻辑
    // 规则：
    // 1. 如果是结算阶段或 Super，显示所有。
    // 2. 如果是玩家：
    //    - 如果是过去行动过的玩家 (midx < current) -> 可以看到 Ban 表 (了解情况)
    //    - 如果是当前行动的玩家 (midx == current) -> 可以看到 Ban 表 (必须知道规则)
    //    - 如果是未来玩家 (midx > current) -> 只能看到空表 (不透露信息)
    // 3. 旁观者可以看到所有 (默认)。

    let mut display_banned_i = HashSet::new();
    let mut display_banned_f = HashSet::new();

    let show_bans = if is_super || is_settled {
      true
    } else if let Some(u) = user {
      // 玩家视角
      if let Some(midx) = self.players.iter().position(|p| p == u) {
        // 只有 过去 或 当前 玩家可见
        midx <= self.current_player_idx
      } else {
        // 非参赛玩家（普通旁观者）可见
        true
      }
    } else {
      // 匿名旁观者可见
      true
    };

    if show_bans {
      display_banned_i = self.banned_initials.clone();
      display_banned_f = self.banned_finals.clone();
    }

    // 第一棒特殊逻辑：
    // 如果是第一棒，且请求者正是当前玩家，需要将答案的声韵母混入 ban 列表显示
    if self.phase == GamePhase::Gaming && self.is_first_describer {
      if let Some(u) = user {
        if let Some(curr_id) = self.players.get(self.current_player_idx) {
          if curr_id == u {
            display_banned_i.extend(self.answer_initials.clone());
            display_banned_f.extend(self.answer_finals.clone());
          }
        }
      }
    }

    ClientView {
      game_id: self.game_id.clone(),
      phase: self.phase.clone(),
      hint: self.hint_text.clone(),
      players: p_views,
      deadline_ms: self
        .turn_deadline
        .map(|t| t.saturating_duration_since(now).as_millis() as u64),
      is_super,
      correct_answer: if is_super || is_settled {
        Some(self.answer_text.clone())
      } else {
        None
      },
      // Pinyin fields
      all_initials: Some(self.all_initials.clone()),
      all_finals: Some(self.all_finals.clone()),
      banned_initials: Some(display_banned_i.into_iter().collect()),
      banned_finals: Some(display_banned_f.into_iter().collect()),
      history: Some(visible_history),
      my_prompt,
      is_first_turn: Some(is_first_turn),
      is_guessing_turn: Some(is_guessing_turn),
      full_history: if is_settled {
        Some(self.history.clone())
      } else {
        None
      },
      winner: if is_settled { Some(self.winner) } else { None },
      end_message: if is_settled {
        Some(if self.winner { "Success!" } else { "Failed." }.to_string())
      } else {
        None
      },
      current_player_id: if self.phase == GamePhase::Gaming
        && self.current_player_idx < self.players.len()
      {
        Some(self.players[self.current_player_idx].clone())
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
