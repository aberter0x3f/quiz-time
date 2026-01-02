use crate::{
  logic::{advance_turn, build_client_view, check_all_submitted, perform_take_action},
  models::{AppState, ClientMsg, GamePhase, InternalMsg, Player, PlayerStatus},
  templates::render_html,
};
use axum::{
  body::Body,
  extract::{
    Query, State,
    ws::{Message, WebSocket, WebSocketUpgrade},
  },
  http::{HeaderMap, StatusCode, header},
  response::{Html, IntoResponse, Response},
};
use chrono::Local;
use futures::{sink::SinkExt, stream::StreamExt};
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

// 鉴权辅助
pub fn check_auth(auth_header: Option<&str>, password: &str) -> Option<String> {
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

pub async fn index_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
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

pub async fn watch_handler() -> Html<String> {
  Html(render_html("", true))
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
  let mut user = check_auth(auth_header, &game_r.server_password);

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
