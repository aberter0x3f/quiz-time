use axum::{
  Router,
  body::Body,
  extract::{
    Query, State,
    ws::{Message, WebSocket, WebSocketUpgrade},
  },
  http::{HeaderMap, StatusCode, header},
  response::{Html, IntoResponse, Response},
  routing::get,
};
use chrono::Local;
use futures::{sink::SinkExt, stream::StreamExt};
use rand::seq::SliceRandom;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
  collections::HashMap,
  env, fs, io,
  net::SocketAddr,
  sync::Arc,
  time::{Duration, Instant},
};
use tokio::sync::{RwLock, broadcast};

// 游戏阶段枚举
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum GamePhase {
  Waiting,
  Picking,
  Answering,
  Settlement,
}

// 玩家状态枚举
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum PlayerStatus {
  Waiting,
  Picking,
  Stopped,
  Answering,
  Submitted,
}

// 玩家数据结构
#[derive(Debug, Clone, Serialize)]
struct Player {
  id: String,
  color_hue: u16,
  status: PlayerStatus,
  obtained_indices: Vec<usize>,
  answer: Option<String>,
  is_online: bool,
  #[serde(skip)]
  last_seen: Instant,
}

// 游戏核心状态
struct GameState {
  phase: GamePhase,
  players: Vec<String>,
  player_map: HashMap<String, Player>,
  problem_text: Vec<char>,
  answer_text: String,
  hint_text: String,
  cursor: usize,
  current_turn_idx: usize,
  turn_deadline: Option<Instant>,
  answer_deadline: Option<Instant>,
  server_password: String,
}

// 内部消息广播
#[derive(Clone, Debug)]
enum InternalMsg {
  StateUpdated,
  Log(String),
}

// 应用共享状态
struct AppState {
  game: Arc<RwLock<GameState>>,
  tx: broadcast::Sender<InternalMsg>,
}

// 客户端上行消息
#[derive(Deserialize)]
#[serde(tag = "type", content = "data")]
enum ClientMsg {
  Heartbeat,
  Action { action: String },
  Answer { content: String },
}

// 下发给客户端的视图数据
#[derive(Serialize)]
struct ClientView {
  phase: GamePhase,
  hint: String,
  players: Vec<PlayerView>,
  grid: Vec<GridCell>,
  my_username: Option<String>,
  turn_deadline_ms: Option<u64>,
  answer_deadline_ms: Option<u64>,
  full_problem: Option<String>,
  correct_answer: Option<String>,
}

#[derive(Serialize)]
struct PlayerView {
  id: String,
  color_hue: u16,
  status: PlayerStatus,
  is_me: bool,
  is_online: bool,
  obtained_count: usize,
  answer: Option<String>,
}

#[derive(Serialize)]
struct GridCell {
  owner_color_hue: Option<u16>,
  char_content: Option<char>,
}

#[tokio::main]
async fn main() {
  let args: Vec<String> = env::args().collect();
  if args.len() < 4 {
    eprintln!("Usage: ./server <problem_path> <answer_path> <hint_path>");
    return;
  }

  let problem = fs::read_to_string(&args[1])
    .expect("Read problem failed")
    .trim()
    .chars()
    .collect();
  let answer = fs::read_to_string(&args[2])
    .expect("Read answer failed")
    .trim()
    .to_string();
  let hint = fs::read_to_string(&args[3])
    .expect("Read hint failed")
    .trim()
    .to_string();

  let password = generate_random_password();
  fs::write("passwords.txt", &password).expect("Write password failed");
  println!("Password generated: {}", password);

  // 初始化广播通道
  let (tx, _) = broadcast::channel::<InternalMsg>(100);

  let game_state = Arc::new(RwLock::new(GameState {
    phase: GamePhase::Waiting,
    players: Vec::new(),
    player_map: HashMap::new(),
    problem_text: problem,
    answer_text: answer,
    hint_text: hint,
    cursor: 0,
    current_turn_idx: 0,
    turn_deadline: None,
    answer_deadline: None,
    server_password: password,
  }));

  let app_state = Arc::new(AppState {
    game: game_state.clone(),
    tx: tx.clone(),
  });

  // 启动后台游戏循环
  let game_bg = game_state.clone();
  let tx_bg = tx.clone();
  tokio::spawn(async move {
    game_loop(game_bg, tx_bg).await;
  });

  // 启动标准输入监听
  let game_stdin = game_state.clone();
  let tx_stdin = tx.clone();
  let handle = tokio::runtime::Handle::current();
  std::thread::spawn(move || {
    handle_stdin(game_stdin, tx_stdin, handle);
  });

  let app = Router::new()
    .route("/", get(index_handler))
    .route("/watch", get(watch_handler))
    .route("/ws", get(ws_handler))
    .with_state(app_state);

  let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
  println!("Server listening on {}", addr);
  let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
  axum::serve(listener, app).await.unwrap();
}

fn generate_random_password() -> String {
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
fn handle_stdin(
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

async fn start_game(game: Arc<RwLock<GameState>>, tx: broadcast::Sender<InternalMsg>) {
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
async fn game_loop(game: Arc<RwLock<GameState>>, tx: broadcast::Sender<InternalMsg>) {
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

fn force_next_turn(g: &mut GameState, tx: &broadcast::Sender<InternalMsg>) {
  if let Some(current_id) = g.players.get(g.current_turn_idx).cloned() {
    if let Some(p) = g.player_map.get_mut(&current_id) {
      if p.status == PlayerStatus::Picking {
        p.status = PlayerStatus::Stopped;
      }
    }
  }
  advance_turn(g, tx);
}

fn perform_take_action(g: &mut GameState, tx: &broadcast::Sender<InternalMsg>) {
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
fn advance_turn(g: &mut GameState, tx: &broadcast::Sender<InternalMsg>) {
  let start_idx = g.current_turn_idx;
  let mut next_idx = (start_idx + 1) % g.players.len();

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

fn enter_answering_phase(g: &mut GameState, tx: &broadcast::Sender<InternalMsg>) {
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

fn finish_game(g: &mut GameState, tx: &broadcast::Sender<InternalMsg>) {
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

fn check_all_submitted(g: &mut GameState, tx: &broadcast::Sender<InternalMsg>) {
  let all_done = g.players.iter().all(|id| match g.player_map.get(id) {
    Some(p) => p.status == PlayerStatus::Submitted || !p.is_online,
    None => true,
  });

  if all_done {
    finish_game(g, tx);
  }
}

// 鉴权辅助
fn check_auth(auth_header: Option<&str>, password: &str) -> Option<String> {
  if let Some(header) = auth_header {
    if header.starts_with("Basic ") {
      if let Ok(decoded) = base64::decode(&header[6..]) {
        if let Ok(s) = String::from_utf8(decoded) {
          if let Some((u, p)) = s.split_once(':') {
            if p == password {
              let re = Regex::new(r"^[0-9A-Za-z_\-]{1,16}$").unwrap();
              if re.is_match(u) {
                return Some(u.to_string());
              }
            }
          }
        }
      }
    }
  }
  None
}

async fn index_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
  let game = state.game.read().await;
  let auth_header = headers
    .get(header::AUTHORIZATION)
    .and_then(|h| h.to_str().ok());

  match check_auth(auth_header, &game.server_password) {
    Some(username) => Html(render_html(&username, false)).into_response(),
    None => {
      let mut resp = Response::new(Body::new("Unauthorized".to_string()).into());
      *resp.status_mut() = StatusCode::UNAUTHORIZED;
      resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        "Basic realm=\"Quiz Game\"".parse().unwrap(),
      );
      resp
    }
  }
}

async fn watch_handler() -> Html<String> {
  Html(render_html("", true))
}

async fn ws_handler(
  ws: WebSocketUpgrade,
  State(state): State<Arc<AppState>>,
  headers: HeaderMap,
  Query(params): Query<HashMap<String, String>>,
) -> Response {
  let game_r = state.game.read().await;
  let auth_header = headers
    .get(header::AUTHORIZATION)
    .and_then(|h| h.to_str().ok());
  let mut user = check_auth(auth_header, &game_r.server_password);

  // 如果 URL 包含 spectate 参数，强制将用户视为匿名（观战者），忽略 Auth Header
  if params.contains_key("spectate") {
    user = None;
  }

  drop(game_r);
  ws.on_upgrade(move |socket| handle_socket(socket, state, user))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>, user: Option<String>) {
  let (mut sender, mut receiver) = socket.split();
  let is_watcher = user.is_none();
  let username = user
    .clone()
    .unwrap_or_else(|| format!("Guest_{}", rand::random::<u16>()));

  if !is_watcher {
    let mut g = state.game.write().await;
    if g.phase != GamePhase::Waiting && !g.player_map.contains_key(&username) {
      let _ = sender
        .send(Message::text(
          serde_json::to_string(
            &serde_json::json!({"type": "error", "data": "游戏已开始，禁止加入．"}),
          )
          .unwrap(),
        ))
        .await;
      return;
    }
    if !g.player_map.contains_key(&username) {
      g.players.push(username.clone());
      g.player_map.insert(
        username.clone(),
        Player {
          id: username.clone(),
          color_hue: 0,
          status: PlayerStatus::Waiting,
          obtained_indices: Vec::new(),
          answer: None,
          is_online: true,
          last_seen: Instant::now(),
        },
      );
      let _ = state
        .tx
        .send(InternalMsg::Log(format!("{} 加入了游戏", username)));
      let _ = state.tx.send(InternalMsg::StateUpdated);
    } else {
      if let Some(p) = g.player_map.get_mut(&username) {
        p.is_online = true;
      }
      let _ = state
        .tx
        .send(InternalMsg::Log(format!("{} 重连成功", username)));
      let _ = state.tx.send(InternalMsg::StateUpdated);
    }
  }

  let initial_view = {
    let g = state.game.read().await;
    build_client_view(&g, &user)
  };
  let _ = sender
    .send(Message::text(
      serde_json::to_string(&serde_json::json!({"type": "update", "data": initial_view})).unwrap(),
    ))
    .await;

  let mut rx = state.tx.subscribe();

  loop {
    tokio::select! {
      // 处理 WebSocket 消息
      res = receiver.next() => {
        match res {
          Some(Ok(Message::Text(text))) => {
            if let Ok(msg) = serde_json::from_str::<ClientMsg>(&text) {
              if is_watcher { continue; }
              let uname = username.as_str();
              let mut g = state.game.write().await;

              match msg {
                ClientMsg::Heartbeat => { if let Some(p) = g.player_map.get_mut(uname) { p.last_seen = Instant::now(); } },
                ClientMsg::Action { action } => {
                  if g.phase == GamePhase::Picking && g.players.get(g.current_turn_idx).map(|s| s.as_str()) == Some(uname) {
                    if action == "take" { perform_take_action(&mut g, &state.tx); }
                    else if action == "stop" {
                      if let Some(p) = g.player_map.get_mut(uname) { p.status = PlayerStatus::Stopped; let _ = state.tx.send(InternalMsg::Log(format!("[操作] {} 停止取字", uname))); }
                      advance_turn(&mut g, &state.tx);
                    }
                  }
                },
                ClientMsg::Answer { content } => {
                  if g.phase == GamePhase::Answering {
                    if let Some(p) = g.player_map.get_mut(uname) {
                      if p.status != PlayerStatus::Submitted {
                        p.answer = Some(content);
                        p.status = PlayerStatus::Submitted;
                        check_all_submitted(&mut g, &state.tx);
                        let _ = state.tx.send(InternalMsg::StateUpdated);
                      }
                    }
                  }
                }
              }
            }
          },
          Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
          _ => {}
        }
      }
      // 处理内部广播
      Ok(msg) = rx.recv() => {
        match msg {
          InternalMsg::StateUpdated => {
            let g = state.game.read().await;
            let view = build_client_view(&g, &user);
            let json = serde_json::to_string(&serde_json::json!({"type": "update", "data": view})).unwrap();
            if sender.send(Message::text(json)).await.is_err() { break; }
          },
          InternalMsg::Log(l) => {
            let json = serde_json::to_string(&serde_json::json!({"type": "log", "data": format!("{} [系统] {}", Local::now().format("%H:%M:%S"), l)})).unwrap();
            if sender.send(Message::text(json)).await.is_err() { break; }
          }
        }
      }
    }
  }

  // 断开处理
  if !is_watcher {
    let mut g = state.game.write().await;
    if let Some(p) = g.player_map.get_mut(&username) {
      p.is_online = false;
      p.last_seen = Instant::now();
    }

    if g.phase == GamePhase::Waiting {
      g.players.retain(|x| x != &username);
      g.player_map.remove(&username);
      let _ = state
        .tx
        .send(InternalMsg::Log(format!("{} 离开了游戏", username)));
      let _ = state.tx.send(InternalMsg::StateUpdated);
    } else {
      let _ = state.tx.send(InternalMsg::Log(format!(
        "{} 掉线了（保留席位 30s）",
        username
      )));
      let _ = state.tx.send(InternalMsg::StateUpdated);
    }
  }
}

// 构建视图，对不可见数据进行脱敏
fn build_client_view(g: &GameState, user: &Option<String>) -> ClientView {
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

  // 此处遍历题目全长，确保初始显示正确数量的黑框
  for i in 0..problem_len {
    let owner_id = index_owners.get(&i);
    let mut color = None;
    let mut char_content = None;

    if let Some(oid) = owner_id {
      if let Some(p) = g.player_map.get(oid) {
        color = Some(p.color_hue);
        // 如果是观战者(user=None)，is_mine 恒为 false
        // 只有在结算阶段，或者该字属于当前用户时，才填充字符内容
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

fn render_html(username: &str, is_watch: bool) -> String {
  format!(
    r#"
    <!DOCTYPE html>
    <html lang="zh-CN">
      <head>
        <meta charset="UTF-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1.0, maximum-scale=1.0, user-scalable=no" />
        <title>Quiz 接龙</title>
        <link rel="stylesheet" href="https://csstools.github.io/sanitize.css/13.0.0/sanitize.css" />
        <style>
          :root {{ --bg: #f4f4f4; --text: #333; --cell-size: 40px; }}
          body {{ font-family: monospace; background: var(--bg); color: var(--text); margin: 0; padding: 0; height: 100vh; display: flex; overflow: hidden; }}
          * {{ border-radius: 0 !important; box-sizing: border-box; }}

          /* PC Layout */
          .sidebar {{ width: 250px; background: #e0e0e0; border-right: 2px solid #000; display: flex; flex-direction: column; overflow-y: auto; flex-shrink: 0; }}
          .main {{ flex: 1; display: flex; flex-direction: column; overflow: hidden; }}
          .log-panel {{ width: 300px; background: #fff; border-left: 2px solid #000; overflow-y: auto; font-size: 12px; padding: 10px; flex-shrink: 0; }}

          /* Mobile Layout */
          @media (max-width: 800px) {{
            body {{ flex-direction: column; height: auto; overflow-y: auto; }}
            .sidebar {{ width: 100%; height: 180px; border-right: none; border-top: 2px solid #000; order: 2; }}
            .main {{ width: 100%; min-height: 60vh; order: 1; }}
            .log-panel {{ width: 100%; height: 200px; border-left: none; border-top: 2px solid #000; order: 3; }}
          }}

          .header {{ height: 60px; border-bottom: 2px solid #000; display: flex; align-items: center; justify-content: center; font-size: 1.1em; font-weight: bold; background: #fff; padding: 0 10px; }}
          .content {{ flex: 1; padding: 20px; overflow-y: auto; display: flex; justify-content: center; align-items: flex-start; }}
          .controls {{ min-height: 80px; height: auto; border-top: 2px solid #000; background: #ddd; display: flex; align-items: center; justify-content: center; gap: 10px; padding: 10px; flex-wrap: wrap; }}

          .grid {{ display: grid; grid-template-columns: repeat(auto-fill, var(--cell-size)); gap: 4px; width: 100%; max-width: 800px; padding: 10px; background: transparent; }}
          .cell {{
            width: var(--cell-size); height: var(--cell-size);
            background: #222; /* 默认未翻开颜色 */
            border: 1px solid #555; /* 增加边框以便看清空格子 */
            color: #000; display: flex; align-items: center; justify-content: center;
            font-size: 20px; font-weight: bold;
          }}

          .player-item {{ padding: 10px; border-bottom: 1px solid #999; display: flex; justify-content: space-between; align-items: center; font-size: 14px; }}
          .player-status {{ font-size: 10px; padding: 2px 4px; background: #333; color: #fff; }}

          button {{ padding: 12px 20px; border: 2px solid #000; background: #fff; cursor: pointer; font-weight: bold; font-size: 16px; min-width: 80px; }}
          button:hover {{ background: #eee; }}
          button:active {{ background: #ccc; }}
          input[type="text"] {{ padding: 10px; border: 2px solid #000; width: 100%; max-width: 300px; font-size: 16px; }}

          .active-turn {{ border-left: 6px solid red; background: #fff0f0; }}
          #result-area {{ display: none; padding: 20px; background: #fff; border: 2px solid #000; margin-top: 20px; width: 100%; max-width: 800px; }}
        </style>
      </head>
      <body>
        <div class="sidebar" id="player-list"></div>
        <div class="main">
          <div class="header" id="hint-box">...</div>
          <div class="content">
            <div style="width: 100%; display: flex; flex-direction: column; align-items: center;">
              <div class="grid" id="grid-box"></div>
              <div id="result-area"></div>
            </div>
          </div>
          <div class="controls" id="control-box">
            <div id="status-text">连接中...</div>
          </div>
        </div>
        <div class="log-panel" id="log-box"></div>
        <script>
          const isWatch = {};
          const username = "{}";
          let socket;
          let gameState = null;
          let timerInterval = null;

          function connect() {{
              const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
              const qs = isWatch ? '?spectate=true' : '';
              socket = new WebSocket(`${{protocol}}//${{window.location.host}}/ws${{qs}}`);
              socket.onopen = () => {{
                  log("系统", "已连接服务器");
                  document.getElementById('status-text').innerText = "已连接";
                  setInterval(() => socket.send(JSON.stringify({{ type: "Heartbeat", data: null }})), 5000);
                  if (!timerInterval) timerInterval = setInterval(updateUiTimer, 100);
              }};
              socket.onmessage = (event) => {{
                  const msg = JSON.parse(event.data);
                  if (msg.type === 'update') {{ gameState = msg.data; render(); }}
                  else if (msg.type === 'log') {{ const parts = msg.data.split(' [系统] '); log("系统", parts[1], parts[0]); }}
                  else if (msg.type === 'error') {{ alert(msg.data); }}
              }};
              socket.onclose = () => {{
                log("系统", "连接断开，尝试重连...");
                setTimeout(connect, 3000);
              }};
          }}

          function log(who, text, time) {{
              const box = document.getElementById('log-box');
              const div = document.createElement('div');
              div.style.marginBottom = "4px";
              div.style.wordBreak = "break-all"; // 防止长单词撑开
              div.innerText = `${{time || new Date().toLocaleTimeString('en-GB')}} [${{who}}] ${{text}}`;
              box.appendChild(div);
              box.scrollTop = box.scrollHeight;
          }}

          function sendAction(act) {{ socket.send(JSON.stringify({{ type: "Action", data: {{ action: act }} }})); }}
          function sendAnswer() {{ socket.send(JSON.stringify({{ type: "Answer", data: {{ content: document.getElementById('ans-input').value }} }})); }}

          function updateUiTimer() {{
             if (!gameState) return;
             // 更新取字按钮倒计时
             const takeBtn = document.getElementById('timer-val-take');
             if (takeBtn && gameState.turn_deadline_ms) {{
                 const deadline = Date.now() + (gameState.turn_deadline_ms || 0);
             }}

             if (gameState._localTargetTime) {{
                 const rem = Math.max(0, (gameState._localTargetTime - Date.now()) / 1000);
                 const els = document.querySelectorAll('.timer-text');
                 els.forEach(el => el.innerText = rem.toFixed(1) + 's');
             }}
          }}

          function render() {{
              // 记录本地目标时间用于倒计时动画
              if (gameState.turn_deadline_ms) {{
                 gameState._localTargetTime = Date.now() + gameState.turn_deadline_ms;
              }} else if (gameState.answer_deadline_ms) {{
                 gameState._localTargetTime = Date.now() + gameState.answer_deadline_ms;
              }} else {{
                 gameState._localTargetTime = null;
              }}

              // 头部提示 + 总字数
              const totalChars = gameState.grid.length;
              const hintText = gameState.phase === 'Settlement' ?
                  "比赛结束" : (gameState.hint || "等待开始...");
              document.getElementById('hint-box').innerText = `${{hintText}} (共 ${{totalChars}} 字)`;

              // 玩家列表
              const pList = document.getElementById('player-list');
              pList.innerHTML = '';
              gameState.players.forEach(p => {{
                  const div = document.createElement('div');
                  div.className = 'player-item';
                  div.style.backgroundColor = `hsl(${{p.color_hue}}, 70%, 90%)`;
                  if (p.status === 'Picking') div.classList.add('active-turn');
                  div.innerHTML = `<div><strong>${{p.id}}</strong> ${{p.is_me ? '(我)' : ''}}<br><small>字数: ${{p.obtained_count}}</small></div><div class="player-status">${{p.is_online?p.status:'OFFLINE'}}</div>`;
                  pList.appendChild(div);
              }});

              // 棋盘格
              const grid = document.getElementById('grid-box');
              grid.innerHTML = '';
              gameState.grid.forEach(cell => {{
                  const div = document.createElement('div');
                  div.className = 'cell';
                  if (cell.owner_color_hue !== null) div.style.backgroundColor = `hsl(${{cell.owner_color_hue}}, 70%, 80%)`;
                  if (cell.char_content) div.innerText = cell.char_content;
                  grid.appendChild(div);
              }});

              // 结果显示 ( Settlement 阶段对所有人可见 )
              const resArea = document.getElementById('result-area');
              if (gameState.phase === 'Settlement') {{
                  resArea.style.display = 'block';
                  let html = `<h3>正确答案：${{gameState.correct_answer}}</h3>
                              <h4>完整题面：</h4><p style="word-break:break-all">${{gameState.full_problem}}</p>
                              <h4>玩家回答：</h4><ul>`;
                  gameState.players.forEach(p => {{
                      let color = p.answer === gameState.correct_answer ? 'green' : 'red';
                      let showAns = p.answer === null ? '(未提交)' : (p.answer === '' ? '(空)' : p.answer);
                      html += `<li style="color:${{color}}">${{p.id}}: ${{showAns}}</li>`;
                  }});
                  resArea.innerHTML = html + '</ul>';
              }} else {{
                  resArea.style.display = 'none';
              }}

              // 控制栏
              const ctrl = document.getElementById('control-box');

              // 观战模式显示
              if (isWatch) {{
                  ctrl.innerHTML = '<div>正在观战中...</div>';
                  return;
              }}

              const me = gameState.players.find(p => p.is_me);
              if (!me) {{ ctrl.innerHTML = ''; return; }}

              if (gameState.phase === 'Picking') {{
                  if (me.status === 'Picking') {{
                      // 这里的 timer-text 会被 updateUiTimer 更新
                      ctrl.innerHTML = `<button onclick="sendAction('take')">要一个字 (<span class="timer-text">--</span>)</button>
                                        <button onclick="sendAction('stop')" style="background:#fdd">停止</button>`;
                  }} else {{
                      ctrl.innerHTML = `<div>等待他人操作...</div>`;
                  }}
              }} else if (gameState.phase === 'Answering') {{
                  if (me.status === 'Submitted') {{
                      ctrl.innerHTML = `<div>答案已提交，等待其他人...</div>`;
                  }} else {{
                      // 在重绘前尝试获取已存在的输入框的值，如果存在则保留
                      const oldInput = document.getElementById('ans-input');
                      const draft = oldInput ? oldInput.value : (me.answer || '');

                      ctrl.innerHTML = `<input type="text" id="ans-input" placeholder="输入答案...">
                                        <button onclick="sendAnswer()">提交 (<span class="timer-text">--</span>)</button>`;

                      // 恢复输入值
                      const newInput = document.getElementById('ans-input');
                      if (newInput) newInput.value = draft;
                  }}
              }} else if (gameState.phase === 'Settlement') {{
                  ctrl.innerHTML = `<div>游戏结束</div>`;
              }} else {{
                  ctrl.innerHTML = `<div>等待管理员 /start</div>`;
              }}

              // 立即触发一次 Timer UI 更新，避免闪烁
              updateUiTimer();
          }}
          connect();
        </script>
      </body>
    </html>
    "#,
    is_watch, username
  )
}
