use crate::models::{ClientView, InternalMsg};
use tokio::sync::broadcast;

pub mod chain;
pub mod pinyin;
pub mod pinyin_utils;

#[async_trait::async_trait]
pub trait GameLogic: Send + Sync {
  fn handle_join(&mut self, player_id: String, tx: &broadcast::Sender<InternalMsg>);
  fn handle_leave(&mut self, player_id: &str, tx: &broadcast::Sender<InternalMsg>);
  fn handle_action(&mut self, player_id: &str, action: String, tx: &broadcast::Sender<InternalMsg>);
  fn handle_answer(
    &mut self,
    player_id: &str,
    content: String,
    tx: &broadcast::Sender<InternalMsg>,
  );
  fn tick(&mut self, tx: &broadcast::Sender<InternalMsg>);
  fn get_view(&self, player_id: Option<&str>, is_super: bool) -> ClientView;
  fn start_game(&mut self, tx: &broadcast::Sender<InternalMsg>);
  fn get_passwords(&self) -> (String, String);
}

pub enum GameMode {
  Chain(chain::ChainGame),
  Pinyin(pinyin::PinyinGame),
}

#[async_trait::async_trait]
impl GameLogic for GameMode {
  fn handle_join(&mut self, pid: String, tx: &broadcast::Sender<InternalMsg>) {
    match self {
      Self::Chain(g) => g.handle_join(pid, tx),
      Self::Pinyin(g) => g.handle_join(pid, tx),
    }
  }
  fn handle_leave(&mut self, pid: &str, tx: &broadcast::Sender<InternalMsg>) {
    match self {
      Self::Chain(g) => g.handle_leave(pid, tx),
      Self::Pinyin(g) => g.handle_leave(pid, tx),
    }
  }
  fn handle_action(&mut self, pid: &str, act: String, tx: &broadcast::Sender<InternalMsg>) {
    match self {
      Self::Chain(g) => g.handle_action(pid, act, tx),
      Self::Pinyin(g) => g.handle_action(pid, act, tx),
    }
  }
  fn handle_answer(&mut self, pid: &str, cnt: String, tx: &broadcast::Sender<InternalMsg>) {
    match self {
      Self::Chain(g) => g.handle_answer(pid, cnt, tx),
      Self::Pinyin(g) => g.handle_answer(pid, cnt, tx),
    }
  }
  fn tick(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    match self {
      Self::Chain(g) => g.tick(tx),
      Self::Pinyin(g) => g.tick(tx),
    }
  }
  fn get_view(&self, pid: Option<&str>, is_sup: bool) -> ClientView {
    match self {
      Self::Chain(g) => g.get_view(pid, is_sup),
      Self::Pinyin(g) => g.get_view(pid, is_sup),
    }
  }
  fn start_game(&mut self, tx: &broadcast::Sender<InternalMsg>) {
    match self {
      Self::Chain(g) => g.start_game(tx),
      Self::Pinyin(g) => g.start_game(tx),
    }
  }
  fn get_passwords(&self) -> (String, String) {
    match self {
      Self::Chain(g) => (g.player_password.clone(), g.super_password.clone()),
      Self::Pinyin(g) => (g.player_password.clone(), g.super_password.clone()),
    }
  }
}

pub fn generate_random_id() -> String {
  use rand::Rng;
  (0..16)
    .map(|_| rand::thread_rng().gen_range(0..10).to_string())
    .collect::<String>()
}
