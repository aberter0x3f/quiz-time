use crate::{
  game::GameLogic,
  models::{ClientMsg, InternalMsg},
  state::AppState,
};
use askama::Template;
use axum::{
  extract::{
    Query, State,
    ws::{Message, WebSocket, WebSocketUpgrade},
  },
  http::{HeaderMap, StatusCode, header},
  response::{Html, IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use futures::{sink::SinkExt, stream::StreamExt};
use regex::Regex;
use std::{
  collections::HashMap,
  sync::{Arc, OnceLock},
};

#[derive(Template)]
#[template(path = "chain.html")]
struct ChainTemplate {
  username: String,
  is_spectate: bool,
  is_super: bool,
}

#[derive(Template)]
#[template(path = "pinyin.html")]
struct PinyinTemplate {
  username: String,
  is_spectate: bool,
  is_super: bool,
}

pub enum AuthResult {
  Player(String),
  SuperSpectator,
  Invalid,
}

pub fn check_auth(auth_header: Option<&str>, player_pass: &str, super_pass: &str) -> AuthResult {
  static AUTH_REGEX: OnceLock<Regex> = OnceLock::new();
  let auth_str = match auth_header.and_then(|h| h.strip_prefix("Basic ")) {
    Some(s) => s,
    None => return AuthResult::Invalid,
  };
  let decoded = match BASE64.decode(auth_str) {
    Ok(d) => d,
    Err(_) => return AuthResult::Invalid,
  };
  let s = match String::from_utf8(decoded) {
    Ok(s) => s,
    Err(_) => return AuthResult::Invalid,
  };
  let (u, p) = match s.split_once(':') {
    Some(res) => res,
    None => return AuthResult::Invalid,
  };
  if p == super_pass {
    return AuthResult::SuperSpectator;
  }
  if p == player_pass {
    let re = AUTH_REGEX.get_or_init(|| Regex::new(r"^[0-9A-Za-z_\-]{1,24}$").unwrap());
    if re.is_match(u) {
      return AuthResult::Player(u.to_string());
    }
  }
  AuthResult::Invalid
}

fn unauthorized_resp(realm: &str) -> Response {
  let mut resp = Response::new(axum::body::Body::new("Unauthorized".to_string()).into());
  *resp.status_mut() = StatusCode::UNAUTHORIZED;
  resp.headers_mut().insert(
    header::WWW_AUTHENTICATE,
    format!("Basic realm=\"{}\"", realm).parse().unwrap(),
  );
  resp
}

pub async fn index_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
  let game = state.game.read().await;
  let (pp, sp) = game.get_passwords();
  let auth = headers
    .get(header::AUTHORIZATION)
    .and_then(|h| h.to_str().ok());
  match check_auth(auth, &pp, &sp) {
    AuthResult::Player(u) => {
      if state.is_pinyin {
        Html(
          PinyinTemplate {
            username: u,
            is_spectate: false,
            is_super: false,
          }
          .render()
          .unwrap(),
        )
        .into_response()
      } else {
        Html(
          ChainTemplate {
            username: u,
            is_spectate: false,
            is_super: false,
          }
          .render()
          .unwrap(),
        )
        .into_response()
      }
    }
    _ => unauthorized_resp("Quiz Game"),
  }
}

pub async fn spectate_handler(State(state): State<Arc<AppState>>) -> Response {
  if state.is_pinyin {
    Html(
      PinyinTemplate {
        username: "".into(),
        is_spectate: true,
        is_super: false,
      }
      .render()
      .unwrap(),
    )
    .into_response()
  } else {
    Html(
      ChainTemplate {
        username: "".into(),
        is_spectate: true,
        is_super: false,
      }
      .render()
      .unwrap(),
    )
    .into_response()
  }
}

pub async fn super_spectate_handler(
  State(state): State<Arc<AppState>>,
  headers: HeaderMap,
) -> Response {
  let game = state.game.read().await;
  let (pp, sp) = game.get_passwords();
  match check_auth(
    headers
      .get(header::AUTHORIZATION)
      .and_then(|h| h.to_str().ok()),
    &pp,
    &sp,
  ) {
    AuthResult::SuperSpectator => {
      if state.is_pinyin {
        Html(
          PinyinTemplate {
            username: "".into(),
            is_spectate: true,
            is_super: true,
          }
          .render()
          .unwrap(),
        )
        .into_response()
      } else {
        Html(
          ChainTemplate {
            username: "".into(),
            is_spectate: true,
            is_super: true,
          }
          .render()
          .unwrap(),
        )
        .into_response()
      }
    }
    _ => unauthorized_resp("Quiz Game"),
  }
}

pub async fn ws_handler(
  State(state): State<Arc<AppState>>,
  headers: HeaderMap,
  Query(params): Query<HashMap<String, String>>,
  ws: WebSocketUpgrade,
) -> Response {
  let game = state.game.read().await;
  let (pp, sp) = game.get_passwords();
  let auth = headers
    .get(header::AUTHORIZATION)
    .and_then(|h| h.to_str().ok());
  let is_spec = params.contains_key("spectate");
  let is_super = params.contains_key("super");

  let (user, is_super_mode) = if is_spec {
    if is_super {
      match check_auth(auth, &pp, &sp) {
        AuthResult::SuperSpectator => (None, true),
        _ => return unauthorized_resp("Quiz Game"),
      }
    } else {
      (None, false)
    }
  } else {
    match check_auth(auth, &pp, &sp) {
      AuthResult::Player(u) => (Some(u), false),
      _ => return unauthorized_resp("Quiz Game"),
    }
  };
  drop(game);
  ws.on_upgrade(move |socket| handle_socket(socket, state, user, is_super_mode))
}

async fn handle_socket(
  socket: WebSocket,
  state: Arc<AppState>,
  user: Option<String>,
  is_super: bool,
) {
  let (mut sender, mut receiver) = socket.split();
  let is_watcher = user.is_none();
  let username = user
    .clone()
    .unwrap_or_else(|| format!("Guest_{}", rand::random::<u16>()));

  if !is_watcher {
    state
      .game
      .write()
      .await
      .handle_join(username.clone(), &state.tx);
  }

  // 发送初始状态
  let initial_view = state.game.read().await.get_view(user.as_deref(), is_super);
  if let Ok(json) =
    serde_json::to_string(&serde_json::json!({"type": "update", "data": initial_view}))
  {
    let _ = sender.send(Message::text(json)).await;
  }

  let mut rx = state.tx.subscribe();

  loop {
    tokio::select! {
      res = receiver.next() => {
        match res {
          Some(Ok(Message::Text(text))) => {
            if let Ok(msg) = serde_json::from_str::<ClientMsg>(&text) {
              if is_watcher { continue; }
              let mut g = state.game.write().await;
              match msg {
                ClientMsg::Heartbeat => g.handle_join(username.clone(), &state.tx),
                ClientMsg::Action { action } => g.handle_action(&username, action, &state.tx),
                ClientMsg::Answer { content } => g.handle_answer(&username, content, &state.tx),
              }
            }
          },
          Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
          _ => {}
        }
      }
      Ok(msg) = rx.recv() => {
        match msg {
          InternalMsg::StateUpdated => {
            let view = state.game.read().await.get_view(user.as_deref(), is_super);
            if let Ok(json) = serde_json::to_string(&serde_json::json!({"type": "update", "data": view})) {
                if sender.send(Message::text(json)).await.is_err() { break; }
            }
          },
          InternalMsg::Log(entry) => {
            if let Ok(json) = serde_json::to_string(&serde_json::json!({"type": "log", "data": entry})) {
                if sender.send(Message::text(json)).await.is_err() { break; }
            }
          },
          InternalMsg::Toast(toast) => {
            if toast.to_user == username {
              if let Ok(json) = serde_json::to_string(&serde_json::json!({"type": "toast", "data": toast})) {
                  if sender.send(Message::text(json)).await.is_err() { break; }
              }
            }
          }
        }
      }
    }
  }
  if !is_watcher {
    state.game.write().await.handle_leave(&username, &state.tx);
  }
}
