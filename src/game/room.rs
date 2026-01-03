use super::{chain::ChainGame, pinyin::PinyinGame};
use crate::game::pinyin_utils::PinyinTable;
use crate::models::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::broadcast;
use uuid::Uuid;

pub enum GameSession {
  None,
  Chain(ChainGame),
  Pinyin(PinyinGame),
}

pub struct Room {
  pub id: Uuid,
  pub name: String,
  pub room_type: RoomType,
  pub max_players: usize,
  pub admin_ids: HashSet<i64>,
  pub tx: broadcast::Sender<InternalMsg>,
  pub players: HashMap<i64, RoomPlayer>,
  pub session: GameSession,
}

#[derive(Clone)]
pub struct RoomPlayer {
  pub id: i64,
  pub name: String,
  pub is_online: bool,
  pub is_spectator: bool,
  pub is_admin: bool,
  pub last_seen: Instant,
  pub color_hue: u16,
}

impl Room {
  pub fn new(id: Uuid, name: String, rtype: RoomType, max_players: usize, creator_id: i64) -> Self {
    let (tx, _) = broadcast::channel(100);
    let mut admins = HashSet::new();
    admins.insert(creator_id);

    Self {
      id,
      name,
      room_type: rtype,
      max_players,
      admin_ids: admins,
      tx,
      players: HashMap::new(),
      session: GameSession::None,
    }
  }

  pub fn join(
    &mut self,
    user_id: i64,
    username: String,
    is_spectator: bool,
    is_site_admin: bool,
  ) -> broadcast::Receiver<InternalMsg> {
    let rx = self.tx.subscribe();
    let now = Instant::now();

    // 计算该用户在房间内的有效管理员权限
    let is_room_admin = self.admin_ids.contains(&user_id) || is_site_admin;

    if let Some(p) = self.players.get_mut(&user_id) {
      // Reconnect
      p.is_online = true;
      p.last_seen = now;
      // Update spectator/admin status on rejoin
      p.is_spectator = is_spectator;
      p.is_admin = is_room_admin;
      let _ = self.tx.send(InternalMsg::Log {
        who: "System".into(),
        text: format!("{} reconnected", username),
        time: chrono::Local::now().format("%H:%M:%S").to_string(),
      });
    } else {
      // New Join
      if is_spectator
        || self.players.iter().filter(|p| !p.1.is_spectator).count() < self.max_players
        || is_room_admin
      {
        let hue = (self.players.len() * 360 / self.max_players.max(1)) as u16;
        self.players.insert(
          user_id.clone(),
          RoomPlayer {
            id: user_id.clone(),
            name: username.clone(),
            is_online: true,
            is_spectator,
            is_admin: is_room_admin,
            last_seen: now,
            color_hue: hue,
          },
        );
        if !is_spectator {
          let _ = self.tx.send(InternalMsg::Log {
            who: "System".into(),
            text: format!("{} joined", username),
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
          });
        }
      }
    }

    match &mut self.session {
      GameSession::Chain(g) => g.handle_join(user_id, &self.tx),
      GameSession::Pinyin(g) => g.handle_join(user_id, &self.tx),
      GameSession::None => {}
    }

    let _ = self.tx.send(InternalMsg::StateUpdated);
    rx
  }

  pub fn leave(&mut self, user_id: i64) {
    let is_waiting = matches!(self.session, GameSession::None);

    if is_waiting {
      // 如果还在等待阶段，直接移除玩家，避免幽灵
      if self.players.remove(&user_id).is_some() {
        // Send generic update
      }
    } else {
      // 游戏进行中，标记为离线
      if let Some(p) = self.players.get_mut(&user_id) {
        p.is_online = false;
        p.last_seen = Instant::now();
      }
    }

    match &mut self.session {
      GameSession::Chain(g) => g.handle_leave(user_id, &self.tx),
      GameSession::Pinyin(g) => g.handle_leave(user_id, &self.tx),
      _ => {}
    }
    let _ = self.tx.send(InternalMsg::StateUpdated);
  }

  pub fn handle_action(&mut self, user_id: i64, action: String) {
    // Spectators cannot act
    if let Some(p) = self.players.get(&user_id) {
      if p.is_spectator {
        return;
      }
    }
    match &mut self.session {
      GameSession::Chain(g) => g.handle_action(user_id, action, &self.tx),
      GameSession::Pinyin(g) => g.handle_action(user_id, action, &self.tx),
      _ => {}
    }
  }

  pub fn handle_answer(&mut self, user_id: i64, content: String) {
    if let Some(p) = self.players.get(&user_id) {
      if p.is_spectator {
        return;
      }
    }
    match &mut self.session {
      GameSession::Chain(g) => g.handle_answer(user_id, content, &self.tx),
      GameSession::Pinyin(g) => g.handle_answer(user_id, content, &self.tx),
      _ => {}
    }
  }

  pub fn tick(&mut self, _global_tx: &broadcast::Sender<InternalMsg>) {
    match &mut self.session {
      GameSession::Chain(g) => g.tick(&self.tx, &self.players),
      GameSession::Pinyin(g) => g.tick(&self.tx, &self.players),
      _ => {}
    }
  }

  pub fn start_game(
    &mut self,
    problem: String,
    answer: String,
    hint: String,
    pinyin_table: Arc<PinyinTable>,
  ) {
    // Filter active players (online AND not spectator)
    let mut active_players = Vec::new();
    for (pid, p) in &self.players {
      if p.is_online && !p.is_spectator {
        active_players.push((pid.clone(), p.color_hue));
      }
    }

    if active_players.is_empty() {
      let _ = self.tx.send(InternalMsg::Toast {
        to_user: 0, // Broadcast
        msg: "Cannot start: No active players.".into(),
        kind: "error".into(),
      });
      return;
    }

    match self.room_type {
      RoomType::Chain => {
        let mut game = ChainGame::new(problem, answer, hint);
        game.setup_players(active_players);
        game.start(&self.tx);
        self.session = GameSession::Chain(game);
      }
      RoomType::Pinyin => {
        let mut game = PinyinGame::new(answer, hint, pinyin_table);
        game.setup_players(active_players);
        game.start(&self.tx);
        self.session = GameSession::Pinyin(game);
      }
    }
    let _ = self.tx.send(InternalMsg::StateUpdated);
  }

  pub fn stop_game(&mut self) {
    self.session = GameSession::None;
    let _ = self.tx.send(InternalMsg::StateUpdated);
  }

  pub fn get_view(&self, user_id: Option<i64>, is_super: bool) -> ClientView {
    let is_admin = user_id
      .map(|id| self.admin_ids.contains(&id))
      .unwrap_or(false)
      || is_super;

    let is_spectator = user_id.map_or(false, |id| {
      self.players.get(&id).map_or(false, |p| p.is_spectator)
    });

    let hue_map: HashMap<i64, u16> = self
      .players
      .iter()
      .map(|(k, v)| (*k, v.color_hue))
      .collect();

    let (phase, hint, deadline, grid, pinyin_state, winner, correct_ans) = match &self.session {
      GameSession::None => (
        GamePhase::Waiting,
        String::new(),
        None,
        None,
        None,
        None,
        None,
      ),
      GameSession::Chain(g) => {
        g.get_view_data(user_id, is_spectator && (is_super || is_admin), &hue_map)
      }
      GameSession::Pinyin(g) => {
        g.get_view_data(user_id, is_spectator && (is_super || is_admin), &hue_map)
      }
    };

    let mut player_views = Vec::new();

    // Sort logic: Active Game Players first, then Spectators/Others
    let mut all_pids: Vec<i64> = self.players.keys().cloned().collect();

    // If gaming, put game order first
    let game_order = match &self.session {
      GameSession::Chain(g) => Some(g.players.clone()),
      GameSession::Pinyin(g) => Some(g.players.clone()),
      GameSession::None => None,
    };

    if let Some(order) = game_order {
      // Remove game players from all_pids to find remaining (spectators)
      all_pids.retain(|id| !order.contains(id));
      // Add game players in order
      for pid in order {
        self.push_player_view(pid, user_id, is_admin, &mut player_views);
      }
    }

    // Add remaining (Spectators/Waiting)
    all_pids.sort(); // Sort by ID or name if needed
    for pid in all_pids {
      self.push_player_view(pid, user_id, is_admin, &mut player_views);
    }

    ClientView {
      room_id: self.id.to_string(),
      room_name: self.name.clone(),
      room_type: self.room_type,
      phase,
      hint,
      deadline_ms: deadline.map(|t| t.saturating_duration_since(Instant::now()).as_millis() as u64),
      is_admin,
      admin_ids: if is_admin {
        Some(self.admin_ids.iter().cloned().collect())
      } else {
        None
      },
      players: player_views,
      max_players: self.max_players,
      grid,
      pinyin_state,
      winner,
      correct_answer: correct_ans,
    }
  }

  fn push_player_view(
    &self,
    pid: i64,
    user_id: Option<i64>,
    is_admin: bool,
    views: &mut Vec<PlayerView>,
  ) {
    if let Some(rp) = self.players.get(&pid) {
      let (status, score, active, ans) = match &self.session {
        GameSession::Chain(g) => g.get_player_state(pid, user_id, rp.is_spectator && is_admin),
        GameSession::Pinyin(g) => g.get_player_state(pid, user_id, rp.is_spectator && is_admin),
        GameSession::None => (PlayerStatus::Waiting, None, false, None),
      };

      // 非管理员不能看到观战人员
      if rp.is_spectator && !is_admin {
        return;
      }

      views.push(PlayerView {
        id: rp.id,
        name: rp.name.clone(),
        color_hue: rp.color_hue,
        status: if rp.is_spectator {
          PlayerStatus::Waiting
        } else {
          status
        },
        is_me: user_id == Some(pid),
        is_online: rp.is_online,
        is_active_turn: active,
        score_display: score,
        answer: ans,
        is_spectator: rp.is_spectator,
        is_admin: rp.is_admin,
      });
    }
  }
}
