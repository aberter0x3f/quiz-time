use rand::seq::SliceRandom;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, broadcast};

use crate::models::{
  ClientView, GamePhase, GameState, GridCell, InternalMsg, PlayerStatus, PlayerView,
};

pub fn generate_random_password() -> String {
  use rand::Rng;
  const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let mut rng = rand::thread_rng();
  (0..12)
    .map(|_| {
      let idx = rng.gen_range(0..CHARSET.len());
      CHARSET[idx] as char
    })
    .collect()
}

// 监听控制台输入 /start
pub fn handle_stdin(
  game: Arc<RwLock<GameState>>,
  tx: broadcast::Sender<InternalMsg>,
  handle: tokio::runtime::Handle,
) {
  let stdin = io::stdin();
  let mut line = String::new();
  while stdin.read_line(&mut line).is_ok() {
    if line.trim() == "/start" {
      let g = game.clone();
      let t = tx.clone();
      handle.spawn(async move {
        start_game(g, t).await;
      });
    }
    line.clear();
  }
}

pub async fn start_game(game: Arc<RwLock<GameState>>, tx: broadcast::Sender<InternalMsg>) {
  let mut g = game.write().await;
  if g.phase != GamePhase::Waiting {
    println!("Game already started.");
    return;
  }

  // 清理离线玩家
  let online_players: Vec<String> = g
    .players
    .iter()
    .filter(|id| g.player_map.get(*id).map_or(false, |p| p.is_online))
    .cloned()
    .collect();

  if online_players.is_empty() {
    println!("No players online.");
    return;
  }

  g.players = online_players;

  // 打乱顺序
  let mut rng = rand::thread_rng();
  g.players.shuffle(&mut rng);

  // 分配色相
  let player_ids = g.players.clone();
  let count = player_ids.len();
  for (i, id) in player_ids.iter().enumerate() {
    if let Some(p) = g.player_map.get_mut(id) {
      p.color_hue = ((i * 360) / count) as u16;
      p.status = PlayerStatus::Waiting;
    }
  }

  g.phase = GamePhase::Picking;
  g.cursor = 0;
  g.current_turn_idx = 0;

  // 首位玩家开始
  g.turn_deadline = Some(Instant::now() + Duration::from_secs(3));

  if let Some(first_id) = g.players.first().cloned() {
    if let Some(p) = g.player_map.get_mut(&first_id) {
      p.status = PlayerStatus::Picking;
    }
  }

  let _ = tx.send(InternalMsg::Log("比赛开始！顺序已打乱．".to_string()));
  let _ = tx.send(InternalMsg::StateUpdated);
}

// 游戏主循环：处理超时和自动流转
pub async fn game_loop(game: Arc<RwLock<GameState>>, tx: broadcast::Sender<InternalMsg>) {
  let mut interval = tokio::time::interval(Duration::from_millis(100));
  loop {
    interval.tick().await;
    let mut g = game.write().await;
    let now = Instant::now();

    // 检查断线超时
    if g.phase != GamePhase::Waiting && g.phase != GamePhase::Settlement {
      let mut timed_out_ids = Vec::new();
      for (id, p) in &g.player_map {
        if !p.is_online && now.duration_since(p.last_seen) > Duration::from_secs(30) {
          timed_out_ids.push(id.clone());
        }
      }

      for id in timed_out_ids {
        let _ = tx.send(InternalMsg::Log(format!(
          "玩家 {} 断线超时，强制移出．",
          id
        )));
        // 标记为已提交空答案
        if let Some(p) = g.player_map.get_mut(&id) {
          p.answer = Some("".to_string());
          p.status = PlayerStatus::Submitted;
        }
        // 若当前轮到该玩家，强制跳过
        if g.phase == GamePhase::Picking && g.players.get(g.current_turn_idx) == Some(&id) {
          force_next_turn(&mut g, &tx);
        }
      }
    }

    // 取字阶段超时
    if g.phase == GamePhase::Picking {
      if let Some(deadline) = g.turn_deadline {
        if now > deadline {
          perform_take_action(&mut g, &tx);
        }
      }
    }

    // 答题阶段超时
    if g.phase == GamePhase::Answering {
      if let Some(deadline) = g.answer_deadline {
        if now > deadline {
          finish_game(&mut g, &tx);
        }
      }
    }
  }
}

pub fn force_next_turn(g: &mut GameState, tx: &broadcast::Sender<InternalMsg>) {
  if let Some(current_id) = g.players.get(g.current_turn_idx).cloned() {
    if let Some(p) = g.player_map.get_mut(&current_id) {
      if p.status == PlayerStatus::Picking {
        p.status = PlayerStatus::Stopped;
      }
    }
  }
  advance_turn(g, tx);
}

pub fn perform_take_action(g: &mut GameState, tx: &broadcast::Sender<InternalMsg>) {
  let current_id = g.players[g.current_turn_idx].clone();

  if g.cursor >= g.problem_text.len() {
    let _ = tx.send(InternalMsg::Log("题面已取完．".to_string()));
    if let Some(p) = g.player_map.get_mut(&current_id) {
      p.status = PlayerStatus::Stopped;
    }
    advance_turn(g, tx);
    return;
  }

  if let Some(p) = g.player_map.get_mut(&current_id) {
    p.obtained_indices.push(g.cursor);
    let _ = tx.send(InternalMsg::Log(format!(
      "[操作] {} 拿取了一个字",
      current_id
    )));
  }
  g.cursor += 1;

  g.turn_deadline = Some(Instant::now() + Duration::from_secs(3));
  let _ = tx.send(InternalMsg::StateUpdated);
}

// 轮转到下一位
pub fn advance_turn(g: &mut GameState, tx: &broadcast::Sender<InternalMsg>) {
  let mut next_idx = (g.current_turn_idx + 1) % g.players.len();

  // 寻找下一个状态为 Waiting 的玩家
  let mut found_valid = false;

  for _ in 0..g.players.len() {
    let pid = &g.players[next_idx];
    if let Some(p) = g.player_map.get(pid) {
      if p.status == PlayerStatus::Waiting {
        found_valid = true;
        break;
      }
    }
    next_idx = (next_idx + 1) % g.players.len();
  }

  let waiting_count = g
    .player_map
    .values()
    .filter(|p| p.status == PlayerStatus::Waiting)
    .count();

  if !found_valid {
    enter_answering_phase(g, tx);
  } else if waiting_count == 1 {
    let last_pid = g.players[next_idx].clone();
    let remaining_len = g.problem_text.len() - g.cursor;
    if remaining_len > 0 {
      if let Some(p) = g.player_map.get_mut(&last_pid) {
        for i in 0..remaining_len {
          p.obtained_indices.push(g.cursor + i);
        }
      }
      g.cursor += remaining_len;
      let _ = tx.send(InternalMsg::Log(format!(
        "最后一名选手 {} 自动获得剩余所有字符．",
        last_pid
      )));
    }

    if let Some(p) = g.player_map.get_mut(&last_pid) {
      p.status = PlayerStatus::Stopped;
    }

    enter_answering_phase(g, tx);
  } else {
    // 正常轮转
    g.current_turn_idx = next_idx;
    let next_pid = g.players[next_idx].clone();
    if let Some(p) = g.player_map.get_mut(&next_pid) {
      p.status = PlayerStatus::Picking;
    }
    g.turn_deadline = Some(Instant::now() + Duration::from_secs(3));
    let _ = tx.send(InternalMsg::StateUpdated);
  }
}

pub fn enter_answering_phase(g: &mut GameState, tx: &broadcast::Sender<InternalMsg>) {
  g.phase = GamePhase::Answering;
  g.answer_deadline = Some(Instant::now() + Duration::from_secs(60));

  for p in g.player_map.values_mut() {
    // 将所有非离线/非已提交状态转为答题中
    if p.status != PlayerStatus::Submitted {
      p.status = PlayerStatus::Answering;
    }
  }

  let _ = tx.send(InternalMsg::Log(
    "取字阶段结束，进入 60s 答题时间！".to_string(),
  ));
  let _ = tx.send(InternalMsg::StateUpdated);
}

pub fn finish_game(g: &mut GameState, tx: &broadcast::Sender<InternalMsg>) {
  if g.phase == GamePhase::Settlement {
    return;
  }
  g.phase = GamePhase::Settlement;
  let _ = tx.send(InternalMsg::Log("游戏结束，公布结果！".to_string()));
  let _ = tx.send(InternalMsg::StateUpdated);

  tokio::spawn(async {
    tokio::time::sleep(Duration::from_secs(60)).await;
    std::process::exit(0);
  });
}

pub fn check_all_submitted(g: &mut GameState, tx: &broadcast::Sender<InternalMsg>) {
  let all_done = g.players.iter().all(|id| match g.player_map.get(id) {
    Some(p) => p.status == PlayerStatus::Submitted || !p.is_online,
    None => true,
  });

  if all_done {
    finish_game(g, tx);
  }
}

// 构建视图，对不可见数据进行脱敏
pub fn build_client_view(g: &GameState, user: &Option<String>) -> ClientView {
  let now = Instant::now();
  let turn_deadline_ms = g
    .turn_deadline
    .map(|t| t.saturating_duration_since(now).as_millis() as u64);
  let answer_deadline_ms = g
    .answer_deadline
    .map(|t| t.saturating_duration_since(now).as_millis() as u64);

  let players_view: Vec<PlayerView> = g
    .players
    .iter()
    .map(|id| {
      let p = &g.player_map[id];
      let is_me = user.as_ref() == Some(id);
      let answer_visible = g.phase == GamePhase::Settlement || is_me;

      PlayerView {
        id: p.id.clone(),
        color_hue: p.color_hue,
        status: p.status.clone(),
        is_me,
        is_online: p.is_online,
        obtained_count: p.obtained_indices.len(),
        answer: if answer_visible {
          p.answer.clone()
        } else {
          None
        },
      }
    })
    .collect();

  let mut grid = Vec::new();
  let problem_len = g.problem_text.len();

  let mut index_owners = HashMap::new();
  for (pid, p) in &g.player_map {
    for &idx in &p.obtained_indices {
      index_owners.insert(idx, pid.clone());
    }
  }

  for i in 0..problem_len {
    let owner_id = index_owners.get(&i);
    let mut color = None;
    let mut char_content = None;

    if let Some(oid) = owner_id {
      if let Some(p) = g.player_map.get(oid) {
        color = Some(p.color_hue);
        let is_mine = user.as_ref() == Some(oid);
        if g.phase == GamePhase::Settlement || is_mine {
          char_content = Some(g.problem_text[i]);
        }
      }
    }

    grid.push(GridCell {
      owner_color_hue: color,
      char_content,
    });
  }

  ClientView {
    phase: g.phase.clone(),
    hint: g.hint_text.clone(),
    players: players_view,
    grid,
    my_username: user.clone(),
    turn_deadline_ms,
    answer_deadline_ms,
    full_problem: if g.phase == GamePhase::Settlement {
      Some(g.problem_text.iter().collect())
    } else {
      None
    },
    correct_answer: if g.phase == GamePhase::Settlement {
      Some(g.answer_text.clone())
    } else {
      None
    },
  }
}
