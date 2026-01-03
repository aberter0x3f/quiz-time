use crate::models::{GamePhase, RoomType};
use crate::{
  auth::{Role, User},
  error::AppError,
  middleware::auth_middleware,
  state::AppState,
  ws,
};
use askama::Template;
use axum::{
  Json, Router,
  extract::{Form, Path, State},
  http::StatusCode,
  middleware,
  response::{Html, IntoResponse, Redirect, Response},
  routing::{get, post},
};
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_cookies::CookieManagerLayer;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
  error: Option<String>,
  user: Option<User>,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
  user: Option<User>,
  rooms: Vec<RoomSummaryView>,
}

#[derive(Template)]
#[template(path = "room.html")]
struct RoomTemplate {
  user: Option<User>,
  room_id: String,
  is_spectate: bool,
  is_admin: bool,
}

struct RoomSummaryView {
  id: String,
  name: String,
  mode: String,
  phase: String,
  count: usize,
  max: usize,
}

pub fn app(state: Arc<AppState>) -> Router {
  let auth_routes = Router::new()
    .route("/", get(index))
    .route("/room", post(create_room))
    .route(
      "/room/{id}",
      get(enter_room).put(update_room).delete(delete_room),
    )
    .route("/room/{id}/spectate", get(spectate_room))
    .route("/room/{id}/start", post(start_game))
    .route("/room/{id}/stop", post(stop_game))
    .route("/ws", get(ws::ws_handler))
    .layer(middleware::from_fn_with_state(
      state.clone(),
      auth_middleware,
    ));

  let public_routes = Router::new()
    .route("/login", get(login_page).post(login_submit))
    .route("/login/codeberg", get(crate::auth::oauth::login_codeberg))
    .route(
      "/oauth-callback/codeberg",
      get(crate::auth::oauth::callback_codeberg),
    )
    .route("/logout", get(logout));

  Router::new()
    .merge(public_routes)
    .merge(auth_routes)
    .layer(
      ServiceBuilder::new()
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new().deflate(true).gzip(true))
        .layer(CookieManagerLayer::new())
        .layer(axum::middleware::from_fn_with_state(
          state.clone(),
          crate::middleware::auth_middleware,
        )),
    )
    .with_state(state)
}

// Helper to manually render and return Html response to avoid IntoResponse trait issues with Html<Template>
fn render<T: Template>(t: T) -> Result<impl IntoResponse, AppError> {
  let s = t
    .render()
    .map_err(|e| anyhow::anyhow!("Template error: {}", e))?;
  Ok(Html(s))
}

async fn index(
  State(state): State<Arc<AppState>>,
  axum::Extension(user): axum::Extension<User>,
) -> impl IntoResponse {
  let mut rooms = vec![];
  for r_lock in state.rooms.iter() {
    let r = r_lock.read().await;
    let phase = match &r.session {
      crate::game::room::GameSession::None => GamePhase::Waiting,
      crate::game::room::GameSession::Chain(g) => g.phase,
      crate::game::room::GameSession::Pinyin(g) => g.phase,
    };

    rooms.push(RoomSummaryView {
      id: r.id.to_string(),
      name: r.name.clone(),
      mode: r.room_type.to_string(),
      phase: phase.to_string(),
      count: r.players.iter().filter(|p| !p.1.is_spectator).count(),
      max: r.max_players,
    });
  }
  render(IndexTemplate {
    user: Some(user),
    rooms,
  })
}

async fn login_page() -> impl IntoResponse {
  render(LoginTemplate {
    error: None,
    user: None,
  })
}

#[derive(serde::Deserialize)]
struct LoginParams {
  username: String,
  password: String,
}

async fn login_submit(
  State(state): State<Arc<AppState>>,
  cookies: tower_cookies::Cookies,
  Form(form): Form<LoginParams>,
) -> Response {
  let valid = state
    .users
    .iter()
    .find(|u| u.name == form.username && u.password.as_ref() == Some(&form.password));

  if let Some(entry) = valid {
    let user = entry.value();
    if user.role == Role::Banned {
      return render(LoginTemplate {
        error: Some("Banned".into()),
        user: None,
      })
      .into_response();
    }
    let token = state.token_manager.generate_token(user);
    cookies.add(
      tower_cookies::Cookie::build(("token", token))
        .path("/")
        .http_only(true)
        .build(),
    );
    Redirect::to("/").into_response()
  } else {
    render(LoginTemplate {
      error: Some("Invalid credentials".into()),
      user: None,
    })
    .into_response()
  }
}

async fn logout(
  State(state): State<Arc<AppState>>,
  cookies: tower_cookies::Cookies,
) -> impl IntoResponse {
  if let Some(token) = cookies.get("token") {
    if let Some(claims) = state.token_manager.parse_token(token.value()) {
      if let Some(mut user) = state.users.get_mut(&claims.sub) {
        user.valid_after = chrono::Utc::now().timestamp();
      }
    }
  }
  cookies.remove(tower_cookies::Cookie::new("token", ""));
  Redirect::to("/login").into_response()
}

#[derive(serde::Deserialize)]
struct CreateRoomForm {
  name: String,
  rtype: RoomType,
  max: usize,
}

async fn create_room(
  State(state): State<Arc<AppState>>,
  axum::Extension(user): axum::Extension<User>,
  Form(form): Form<CreateRoomForm>,
) -> impl IntoResponse {
  if user.role != Role::Admin {
    return Redirect::to("/").into_response();
  }
  let id = Uuid::now_v7();
  let room = crate::game::room::Room::new(id, form.name, form.rtype, form.max, user.id.clone());
  state
    .rooms
    .insert(id, Arc::new(tokio::sync::RwLock::new(room)));
  Redirect::to("/").into_response()
}

async fn enter_room(
  State(state): State<Arc<AppState>>,
  Path(id): Path<Uuid>,
  axum::Extension(user): axum::Extension<User>,
) -> Response {
  let r_lock = match state.rooms.get(&id) {
    Some(r) => r,
    None => return Redirect::to("/").into_response(),
  };
  let room = r_lock.read().await;
  let is_admin = room.admin_ids.contains(&user.id) || user.role == Role::Admin;
  render(RoomTemplate {
    user: Some(user),
    room_id: id.to_string(),
    is_spectate: false,
    is_admin,
  })
  .into_response()
}

async fn spectate_room(
  State(_state): State<Arc<AppState>>,
  Path(id): Path<Uuid>,
  axum::Extension(user): axum::Extension<User>,
) -> impl IntoResponse {
  render(RoomTemplate {
    user: Some(user),
    room_id: id.to_string(),
    is_spectate: true,
    is_admin: false,
  })
}

#[derive(serde::Deserialize)]
struct UpdateRoomJson {
  name: String,
  max: usize,
  admins: Vec<i64>,
}

async fn update_room(
  State(state): State<Arc<AppState>>,
  Path(id): Path<Uuid>,
  axum::Extension(user): axum::Extension<User>,
  Json(payload): Json<UpdateRoomJson>,
) -> impl IntoResponse {
  if let Some(r_lock) = state.rooms.get(&id) {
    let mut room = r_lock.write().await;
    if !room.admin_ids.contains(&user.id) && user.role != Role::Admin {
      return StatusCode::FORBIDDEN;
    }
    room.name = payload.name;
    room.max_players = payload.max;
    room.admin_ids = payload.admins.into_iter().collect();
    if user.role != Role::Admin {
      room.admin_ids.insert(user.id.clone());
    }
  }
  StatusCode::OK
}

async fn delete_room(
  State(state): State<Arc<AppState>>,
  Path(id): Path<Uuid>,
  axum::Extension(user): axum::Extension<User>,
) -> impl IntoResponse {
  // Simple auth check
  let can_delete = if let Some(r_lock) = state.rooms.get(&id) {
    let room = r_lock.read().await;
    room.admin_ids.contains(&user.id) || user.role == Role::Admin
  } else {
    false
  };

  if !can_delete {
    return StatusCode::FORBIDDEN;
  }

  state.rooms.remove(&id);
  StatusCode::OK
}

#[derive(serde::Deserialize)]
struct StartGameJson {
  problem: String,
  answer: String,
  hint: String,
}

async fn start_game(
  State(state): State<Arc<AppState>>,
  Path(id): Path<Uuid>,
  axum::Extension(user): axum::Extension<User>,
  Json(payload): Json<StartGameJson>,
) -> impl IntoResponse {
  if let Some(r_lock) = state.rooms.get(&id) {
    let mut room = r_lock.write().await;
    if !room.admin_ids.contains(&user.id) && user.role != Role::Admin {
      return StatusCode::FORBIDDEN.into_response();
    }
    room.start_game(
      payload.problem,
      payload.answer,
      payload.hint,
      state.pinyin_table.clone(),
    );
  }
  StatusCode::OK.into_response()
}

async fn stop_game(
  State(state): State<Arc<AppState>>,
  Path(id): Path<Uuid>,
  axum::Extension(user): axum::Extension<User>,
) -> impl IntoResponse {
  if let Some(r_lock) = state.rooms.get(&id) {
    let mut room = r_lock.write().await;
    if !room.admin_ids.contains(&user.id) && user.role != Role::Admin {
      return StatusCode::FORBIDDEN.into_response();
    }
    room.stop_game();
  }
  StatusCode::OK.into_response()
}
