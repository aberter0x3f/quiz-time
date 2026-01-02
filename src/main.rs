mod game;
mod handlers;
mod models;
mod state;

use crate::game::GameLogic;
use crate::game::{
  GameMode, chain::ChainGame, generate_random_id, pinyin::PinyinGame,
  pinyin_utils::load_pinyin_table,
};
use crate::handlers::{index_handler, spectate_handler, super_spectate_handler, ws_handler};
use crate::state::AppState;
use axum::{Router, routing::get};
use clap::{Parser, Subcommand};
use std::{fs, net::SocketAddr, sync::Arc};
use tokio::sync::{RwLock, broadcast};

#[derive(Parser)]
struct Cli {
  #[command(subcommand)]
  command: Commands,
}

#[derive(Subcommand)]
enum Commands {
  Chain {
    problem_path: String,
    answer_path: String,
    hint_path: String,
  },
  Pinyin {
    answer_path: String,
    hint_path: String,
    pinyin_table_path: String,
  },
}

fn generate_passwords() -> (String, String) {
  use rand::Rng;
  const C: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let get = || {
    (0..16)
      .map(|_| C[rand::thread_rng().gen_range(0..C.len())] as char)
      .collect()
  };
  (get(), get())
}

#[tokio::main]
async fn main() {
  let cli = Cli::parse();
  let (pp, sp) = generate_passwords();
  fs::write("passwords.txt", format!("{}\n{}", pp, sp)).expect("Write passwords failed");
  println!("Passwords generated in passwords.txt");
  println!("Player Password: {}", pp);
  println!("Super Spectator Password: {}", sp);

  let (tx, _) = broadcast::channel(100);

  let (game_mode, is_pinyin) = match cli.command {
    Commands::Chain {
      problem_path,
      answer_path,
      hint_path,
    } => {
      let p = fs::read_to_string(problem_path)
        .expect("Read problem")
        .trim()
        .chars()
        .collect();
      let a = fs::read_to_string(answer_path)
        .expect("Read answer")
        .trim()
        .to_string();
      let h = fs::read_to_string(hint_path)
        .expect("Read hint")
        .trim()
        .to_string();
      let g = ChainGame {
        game_id: generate_random_id(),
        phase: crate::models::GamePhase::Waiting,
        players: vec![],
        player_map: std::collections::HashMap::new(),
        problem_text: p,
        answer_text: a,
        hint_text: h,
        cursor: 0,
        current_turn_idx: 0,
        turn_deadline: None,
        answer_deadline: None,
        player_password: pp,
        super_password: sp,
      };
      (GameMode::Chain(g), false)
    }
    Commands::Pinyin {
      answer_path,
      hint_path,
      pinyin_table_path,
    } => {
      let a = fs::read_to_string(answer_path)
        .expect("Read answer")
        .trim()
        .to_string();
      let h = fs::read_to_string(hint_path)
        .expect("Read hint")
        .trim()
        .to_string();
      let table = load_pinyin_table(&pinyin_table_path);
      let g = PinyinGame::new(a, h, table, pp, sp);
      (GameMode::Pinyin(g), true)
    }
  };

  let app_state = Arc::new(AppState {
    game: Arc::new(RwLock::new(game_mode)),
    tx: tx.clone(),
    is_pinyin,
  });

  // Background Loop
  let bg_state = app_state.clone();
  tokio::spawn(async move {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
    loop {
      interval.tick().await;
      bg_state.game.write().await.tick(&bg_state.tx);
    }
  });

  // Stdin Listener
  let stdin_state = app_state.clone();
  let rt = tokio::runtime::Handle::current();
  std::thread::spawn(move || {
    let stdin = std::io::stdin();
    let mut line = String::new();
    while stdin.read_line(&mut line).is_ok() {
      if line.trim() == "/start" {
        let s = stdin_state.clone();
        rt.spawn(async move {
          s.game.write().await.start_game(&s.tx);
        });
      }
      line.clear();
    }
  });

  let app = Router::new()
    .route("/", get(index_handler))
    .route("/spectate", get(spectate_handler))
    .route("/super-spectate", get(super_spectate_handler))
    .route("/ws", get(ws_handler))
    .with_state(app_state);

  let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
  println!("Server listening on {}", addr);
  let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
  axum::serve(listener, app).await.unwrap();
}
