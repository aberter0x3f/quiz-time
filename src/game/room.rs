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
  ) -> Result<broadcast::Receiver<InternalMsg>, String> {
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
      if !is_spectator {
        let _ = self.tx.send(InternalMsg::Log {
          who: "System".into(),
          text: format!("{} reconnected", username),
          time: chrono::Local::now().format("%H:%M:%S").to_string(),
        });
      }
    } else {
      // New Join
      let game_in_progress = match &self.session {
        GameSession::None => false,
        GameSession::Chain(g) => g.phase != GamePhase::Settlement,
        GameSession::Pinyin(g) => g.phase != GamePhase::Settlement,
      };

      if !is_spectator && game_in_progress {
        return Err("Game is in progress".to_string());
      }

      let current_count = self.players.iter().filter(|p| !p.1.is_spectator).count();

      // Check Capacity
      if !is_spectator && current_count >= self.max_players {
        return Err("Room is full".to_string());
      }

      self.players.insert(
        user_id.clone(),
        RoomPlayer {
          id: user_id.clone(),
          name: username.clone(),
          is_online: true,
          is_spectator,
          is_admin: is_room_admin,
          last_seen: now,
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

    match &mut self.session {
      GameSession::Chain(g) => g.handle_join(user_id, &self.tx),
      GameSession::Pinyin(g) => g.handle_join(user_id, &self.tx),
      GameSession::None => {}
    }

    let _ = self.tx.send(InternalMsg::StateUpdated);
    Ok(rx)
  }

  pub fn leave(&mut self, user_id: i64) {
    let is_waiting = matches!(self.session, GameSession::None);

    if is_waiting {
      // 如果还在等待阶段，直接移除玩家，避免幽灵
      let _ = self.players.remove(&user_id);
    } else {
      let is_spectator = self.players.get(&user_id).map_or(false, |p| p.is_spectator);
      if is_spectator {
        let _ = self.players.remove(&user_id);
      } else if let Some(p) = self.players.get_mut(&user_id) {
        // 游戏进行中，标记为离线
        p.is_online = false;
        p.last_seen = Instant::now();
        let _ = self.tx.send(InternalMsg::Log {
          who: "System".into(),
          text: format!("{} left room", &p.name),
          time: chrono::Local::now().format("%H:%M:%S").to_string(),
        });
      }
    }

    match &mut self.session {
      GameSession::Chain(g) => g.handle_leave(user_id, &self.tx),
      GameSession::Pinyin(g) => g.handle_leave(user_id, &self.tx),
      _ => {}
    }
    let _ = self.tx.send(InternalMsg::StateUpdated);
  }

  pub fn kick(&mut self, user_id: i64) {
    // 1. 先执行离开逻辑，更新游戏内状态（如跳过回合）
    self.leave(user_id);

    // 2. 从房间玩家列表中彻底移除 (防止 leave 逻辑仅仅标记为离线)
    self.players.remove(&user_id);

    // 3. 广播踢人消息，触发 WS 断开
    let _ = self.tx.send(InternalMsg::Kick { target: user_id });

    let _ = self.tx.send(InternalMsg::StateUpdated);
  }

  /// Clean up players who are marked as offline
  fn kick_offline_players(&mut self) {
    let offline_ids: Vec<i64> = self
      .players
      .iter()
      .filter(|(_, p)| !p.is_online)
      .map(|(k, _)| *k)
      .collect();

    if !offline_ids.is_empty() {
      for pid in offline_ids {
        self.kick(pid);
      }
    }
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
    let mut should_clean = false;
    match &mut self.session {
      GameSession::Chain(g) => {
        g.tick(&self.tx, &self.players);
        if g.phase == GamePhase::Settlement {
          should_clean = true;
        }
      }
      GameSession::Pinyin(g) => {
        g.tick(&self.tx, &self.players);
        if g.phase == GamePhase::Settlement {
          should_clean = true;
        }
      }
      _ => {}
    }

    if should_clean {
      self.kick_offline_players();
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
        active_players.push(pid.clone());
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
    self.kick_offline_players();
    let _ = self.tx.send(InternalMsg::StateUpdated);
  }

  pub fn get_view(&self, user_id: Option<i64>, is_site_super: bool) -> ClientView {
    let is_admin = user_id
      .map(|id| self.admin_ids.contains(&id))
      .unwrap_or(false)
      || is_site_super;

    let is_spectator = user_id.map_or(false, |id| {
      self.players.get(&id).map_or(false, |p| p.is_spectator)
    });

    // 实时颜色计算逻辑
    // 1. 确定排序依据（游戏中用游戏列表，大厅中用 ID 排序）
    let active_order: Vec<i64> = match &self.session {
      GameSession::Chain(g) => g.players.clone(),
      GameSession::Pinyin(g) => g.players.clone(),
      GameSession::None => {
        let mut ids: Vec<i64> = self
          .players
          .iter()
          .filter(|(_, p)| !p.is_spectator)
          .map(|(k, _)| *k)
          .collect();
        ids.sort(); // 稳定排序
        ids
      }
    };

    let total = active_order.len().max(1);
    let mut hue_map = HashMap::new();
    for (i, pid) in active_order.iter().enumerate() {
      let hue = (i * 360 / total) as u16;
      hue_map.insert(*pid, hue);
    }

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
      GameSession::Chain(g) => g.get_view_data(user_id, is_spectator && is_admin, &hue_map),
      GameSession::Pinyin(g) => g.get_view_data(user_id, is_spectator && is_admin, &hue_map),
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
        self.push_player_view(
          pid,
          user_id,
          is_admin,
          is_spectator,
          &hue_map,
          &mut player_views,
        );
      }
    }

    // Add remaining (Spectators/Waiting)
    all_pids.sort(); // Sort by ID or name if needed
    for pid in all_pids {
      self.push_player_view(
        pid,
        user_id,
        is_admin,
        is_spectator,
        &hue_map,
        &mut player_views,
      );
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
    is_viewer_admin: bool,
    is_viewer_spectator: bool,
    hue_map: &HashMap<i64, u16>,
    views: &mut Vec<PlayerView>,
  ) {
    if let Some(rp) = self.players.get(&pid) {
      let (status, score, active, ans) = match &self.session {
        GameSession::Chain(g) => {
          g.get_player_state(pid, user_id, is_viewer_admin && is_viewer_spectator)
        }
        GameSession::Pinyin(g) => {
          g.get_player_state(pid, user_id, is_viewer_admin && is_viewer_spectator)
        }
        GameSession::None => (PlayerStatus::Waiting, None, false, None),
      };

      // 非管理员不能看到观战人员
      if rp.is_spectator && !is_viewer_admin {
        return;
      }

      views.push(PlayerView {
        id: rp.id,
        name: rp.name.clone(),
        // 如果在 hue_map 中则使用计算出的颜色，否则（观战）默认为 0
        color_hue: hue_map.get(&pid).cloned().unwrap_or(0),
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
