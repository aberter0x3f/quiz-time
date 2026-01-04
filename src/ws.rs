use crate::auth::User;
use crate::game::{ClientAction, InternalMsg};
use crate::state::AppState;
use axum::{
  extract::{
    Query, State,
    ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
  },
  response::IntoResponse,
};
use futures::{sink::SinkExt, stream::StreamExt};
use std::sync::Arc;
use uuid::Uuid;

#[derive(serde::Deserialize)]
pub struct WsParams {
  room: Uuid,
  #[serde(default)]
  spectate: bool,
}

pub async fn ws_handler(
  State(state): State<Arc<AppState>>,
  ws: WebSocketUpgrade,
  Query(params): Query<WsParams>,
  user_ext: Option<axum::Extension<User>>,
) -> impl IntoResponse {
  if let Some(axum::Extension(u)) = user_ext {
    ws.on_upgrade(move |socket| handle_socket(socket, state, params.room, u, params.spectate))
  } else {
    (axum::http::StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
  }
}

async fn handle_socket(
  socket: WebSocket,
  state: Arc<AppState>,
  room_id: Uuid,
  user: User,
  req_spectate: bool,
) {
  let (mut sender, mut receiver) = socket.split();

  let (rx, _tx) = {
    let r_lock = match state.rooms.get(&room_id) {
      Some(r) => r,
      None => return,
    };
    let mut room = r_lock.write().await;
    match room.join(
      user.id.clone(),
      user.name.clone(),
      req_spectate,
      user.is_admin(),
    ) {
      Ok(rx) => (rx, room.tx.clone()),
      Err(e) => {
        let _ = sender
          .send(Message::Close(Some(CloseFrame {
            code: 4000,
            reason: e.into(),
          })))
          .await;
        return;
      }
    }
  };

  let mut broadcast_rx = rx;

  // Initial State
  {
    if let Some(r_lock) = state.rooms.get(&room_id) {
      let room = r_lock.read().await;
      let view = room.get_view(Some(user.id), user.is_admin());
      if let Ok(json) =
        serde_json::to_string(&serde_json::json!({ "type": "update", "data": view }))
      {
        let _ = sender.send(Message::text(json)).await;
      }
    }
  }

  loop {
    tokio::select! {
      Some(Ok(msg)) = receiver.next() => {
        match msg {
           Message::Text(text) => {
             // Spectators shouldn't really send actions, but we filter in room logic anyway
             if let Ok(action) = serde_json::from_str::<ClientAction>(&text) {
                if let Some(r_lock) = state.rooms.get(&room_id) {
                  let mut room = r_lock.write().await;
                  match action {
                    ClientAction::Action { action } => room.handle_action(user.id, action),
                    ClientAction::Answer { content } => room.handle_answer(user.id, content),
                  }
                }
             }
           },
           Message::Close(_) => break,
           _ => {}
        }
      }
      Ok(msg) = broadcast_rx.recv() => {
        match msg {
          InternalMsg::StateUpdated => {
            if let Some(r_lock) = state.rooms.get(&room_id) {
              let room = r_lock.read().await;
              let view = room.get_view(Some(user.id), user.is_admin());
              if let Ok(json) = serde_json::to_string(&serde_json::json!({ "type": "update", "data": view })) {
                if sender.send(Message::text(json)).await.is_err() { break; }
              }
            }
          },
          InternalMsg::Log { who, text, time } => {
            let json = serde_json::json!({"type": "log", "data": {"who": who, "text": text, "time": time}});
            if sender.send(Message::text(json.to_string())).await.is_err() { break; }
          },
          InternalMsg::Toast { to_user, msg, kind } => {
            // Toast logic: 0 means broadcast to all, otherwise specific user
            if to_user == 0 || to_user == user.id {
              let json = serde_json::json!({"type": "toast", "data": {"msg": msg, "kind": kind}});
              if sender.send(Message::text(json.to_string())).await.is_err() { break; }
            }
          },
          InternalMsg::Kick { target } => {
            if target == user.id {
              let _ = sender.send(Message::Close(Some(CloseFrame {
                code: 4001,
                reason: "You have been kicked".into(),
              }))).await;
              break; // Break the loop to close connection
            }
          }
        }
      }
    }
  }

  // Cleanup on disconnect
  if let Some(r_lock) = state.rooms.get(&room_id) {
    let mut room = r_lock.write().await;
    room.leave(user.id);
  }
}
