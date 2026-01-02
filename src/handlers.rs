use crate::{
  logic::{
    advance_turn, build_client_view, check_all_submitted, perform_take_action, send_sys_log,
  },
  models::{AppState, ClientMsg, GamePhase, InternalMsg, Player, PlayerStatus},
  templates::render_game,
};
use askama::Template;
use axum::{
  body::Body,
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
  time::Instant,
};

// 鉴权结果
pub enum AuthResult {
  Player(String),
  SuperSpectator,
  Invalid,
}

// 鉴权辅助
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
    let re = AUTH_REGEX.get_or_init(|| Regex::new(r"^[0-9A-Za-z_\-]{1,16}$").unwrap());
    if re.is_match(u) {
      return AuthResult::Player(u.to_string());
    }
  }

  AuthResult::Invalid
}

pub async fn index_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
  let game = state.game.read().await;
  let auth_header = headers
    .get(header::AUTHORIZATION)
    .and_then(|h| h.to_str().ok());

  match check_auth(
    auth_header,
    &game.player_password,
    &game.super_spectate_password,
  ) {
    AuthResult::Player(username) => {
      Html(render_game(&username, false, false).render().unwrap()).into_response()
    }
    _ => {
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

pub async fn spectate_handler() -> impl IntoResponse {
  Html(render_game("", true, false).render().unwrap())
}

pub async fn super_spectate_handler(
  State(state): State<Arc<AppState>>,
  headers: HeaderMap,
) -> Response {
  let game = state.game.read().await;
  let auth_header = headers
    .get(header::AUTHORIZATION)
    .and_then(|h| h.to_str().ok());

  // 检查是否使用了超级观察者密码
  match check_auth(
    auth_header,
    &game.player_password,
    &game.super_spectate_password,
  ) {
    AuthResult::SuperSpectator => {
      Html(render_game("", true, true).render().unwrap()).into_response()
    }
    _ => {
      let mut resp = Response::new(Body::new("Unauthorized Super Spectator".to_string()).into());
      *resp.status_mut() = StatusCode::UNAUTHORIZED;
      resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        "Basic realm=\"Quiz Game Super\"".parse().unwrap(),
      );
      resp
    }
  }
}

pub async fn ws_handler(
  ws: WebSocketUpgrade,
  State(state): State<Arc<AppState>>,
  headers: HeaderMap,
  Query(params): Query<HashMap<String, String>>,
) -> Response {
  let game_r = state.game.read().await;
  let auth_header = headers
    .get(header::AUTHORIZATION)
    .and_then(|h| h.to_str().ok());

  let is_spectate_query = params.contains_key("spectate");
  let is_super_query = params.contains_key("super");

  let (user, is_super_mode) = if is_spectate_query {
    // 如果请求了超级模式，校验密码
    if is_super_query {
      match check_auth(
        auth_header,
        &game_r.player_password,
        &game_r.super_spectate_password,
      ) {
        AuthResult::SuperSpectator => (None, true),
        _ => return (StatusCode::UNAUTHORIZED, "Super Spectator Auth Failed").into_response(),
      }
    } else {
      // 普通观察者无需密码
      (None, false)
    }
  } else {
    // 玩家模式
    match check_auth(
      auth_header,
      &game_r.player_password,
      &game_r.super_spectate_password,
    ) {
      AuthResult::Player(u) => (Some(u), false),
      _ => {
        return (
          StatusCode::UNAUTHORIZED,
          [(header::WWW_AUTHENTICATE, "Basic realm=\"Quiz Game\"")],
        )
          .into_response();
      }
    }
  };

  drop(game_r);
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
    let mut g = state.game.write().await;
    // 玩家加入逻辑...
    if g.phase != GamePhase::Waiting && !g.player_map.contains_key(&username) {
      let _ = sender
        .send(Message::text(
          serde_json::to_string(
            &serde_json::json!({"type": "error", "data": "Game in progress, joining denied."}),
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
      send_sys_log(
        &state.tx,
        "System",
        format!("{} joined the game.", username),
      );
      let _ = state.tx.send(InternalMsg::StateUpdated);
    } else {
      if let Some(p) = g.player_map.get_mut(&username) {
        p.is_online = true;
      }
      send_sys_log(&state.tx, "System", format!("{} reconnected.", username));
      let _ = state.tx.send(InternalMsg::StateUpdated);
    }
  }

  // 构建初始视图
  let initial_view = {
    let g = state.game.read().await;
    build_client_view(&g, &user, is_super)
  };
  let _ = sender
    .send(Message::text(
      serde_json::to_string(&serde_json::json!({"type": "update", "data": initial_view})).unwrap(),
    ))
    .await;

  let mut rx = state.tx.subscribe();

  loop {
    tokio::select! {
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
                      if let Some(p) = g.player_map.get_mut(uname) { p.status = PlayerStatus::Stopped; }
                      send_sys_log(&state.tx, "Action", format!("{} stopped picking.", uname));
                      advance_turn(&mut g, &state.tx);
                    }
                  }
                },
                ClientMsg::Answer { content } => {
                  let can_answer = if let Some(p) = g.player_map.get(uname) {
                     g.phase == GamePhase::Answering || (g.phase == GamePhase::Picking && p.status == PlayerStatus::Stopped)
                  } else { false };

                  if can_answer {
                    if let Some(p) = g.player_map.get_mut(uname) {
                      if p.status != PlayerStatus::Submitted {
                        p.answer = Some(content);
                        p.status = PlayerStatus::Submitted;
                        send_sys_log(&state.tx, "System", format!("{} submitted an answer.", uname));
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
      Ok(msg) = rx.recv() => {
        match msg {
          InternalMsg::StateUpdated => {
            let g = state.game.read().await;
            // 每次更新时，根据是否是超级观察者构建不同视图
            let view = build_client_view(&g, &user, is_super);
            let json = serde_json::to_string(&serde_json::json!({"type": "update", "data": view})).unwrap();
            if sender.send(Message::text(json)).await.is_err() { break; }
          },
          InternalMsg::Log(entry) => {
            let json = serde_json::to_string(&serde_json::json!({"type": "log", "data": entry})).unwrap();
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
      send_sys_log(&state.tx, "System", format!("{} left the game.", username));
      let _ = state.tx.send(InternalMsg::StateUpdated);
    } else {
      send_sys_log(
        &state.tx,
        "System",
        format!("{} disconnected (reserved for 30s) .", username),
      );
      let _ = state.tx.send(InternalMsg::StateUpdated);
    }
  }
}
