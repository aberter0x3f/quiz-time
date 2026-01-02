pub mod handlers;
pub mod logic;
pub mod models;
pub mod templates;

use axum::{Router, routing::get};
use std::{collections::HashMap, env, fs, net::SocketAddr, sync::Arc};
use tokio::sync::{RwLock, broadcast};

use crate::{
  handlers::{index_handler, spectate_handler, ws_handler},
  logic::{game_loop, generate_random_password, handle_stdin},
  models::{AppState, GamePhase, GameState, InternalMsg},
};

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
    game_id: generate_random_password(),
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
    .route("/spectate", get(spectate_handler))
    .route("/ws", get(ws_handler))
    .with_state(app_state);

  let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
  println!("Server listening on {}", addr);
  let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
  axum::serve(listener, app).await.unwrap();
}
