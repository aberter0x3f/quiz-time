use crate::auth::Role;
use crate::state::AppState;
use axum::{
  extract::{Request, State},
  middleware::Next,
  response::{IntoResponse, Redirect, Response},
};
use std::sync::Arc;
use tower_cookies::Cookies;

pub async fn auth_middleware(
  State(state): State<Arc<AppState>>,
  cookies: Cookies,
  mut req: Request,
  next: Next,
) -> Response {
  let token = cookies.get("token").map(|c| c.value().to_string());
  let path = req.uri().path().to_string();

  // Whitelist
  if path.starts_with("/login") || path.starts_with("/oauth-callback") || path == "/logout" {
    return next.run(req).await;
  }

  let mut user_val = None;
  if let Some(token_str) = token {
    if let Some(claims) = state.token_manager.parse_token(&token_str) {
      if let Some(u) = state.users.get(&claims.sub) {
        if claims.iat >= u.valid_after && u.role != Role::Banned {
          user_val = Some(u.clone());
        }
      }
    }
  }

  if let Some(u) = user_val {
    req.extensions_mut().insert(u);
    next.run(req).await
  } else {
    if cookies.get("token").is_some() {
      cookies.remove(tower_cookies::Cookie::new("token", ""));
    }
    Redirect::to("/login").into_response()
  }
}
