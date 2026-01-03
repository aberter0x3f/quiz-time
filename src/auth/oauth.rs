use crate::auth::{Role, User};
use crate::error::AppError;
use crate::state::AppState;
use anyhow::{Result, anyhow};
use axum::{
  extract::{Query, State},
  response::{IntoResponse, Redirect},
};
use oauth2::reqwest;
use oauth2::{
  AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EmptyExtraTokenFields,
  EndpointNotSet, EndpointSet, RedirectUrl, RevocationErrorResponseType, StandardErrorResponse,
  StandardRevocableToken, StandardTokenIntrospectionResponse, StandardTokenResponse, TokenResponse,
  TokenUrl,
  basic::{BasicClient, BasicErrorResponseType, BasicTokenType},
};
use std::sync::Arc;
use tower_cookies::{Cookie, Cookies};

pub const AUTH_URL: &str = "https://codeberg.org/login/oauth/authorize";
pub const TOKEN_URL: &str = "https://codeberg.org/login/oauth/access_token";
pub const CODEBERG_API_BASE_URL: &str = "https://codeberg.org/api/v1";

pub type Client = oauth2::Client<
  StandardErrorResponse<BasicErrorResponseType>,
  StandardTokenResponse<EmptyExtraTokenFields, BasicTokenType>,
  StandardTokenIntrospectionResponse<EmptyExtraTokenFields, BasicTokenType>,
  StandardRevocableToken,
  StandardErrorResponse<RevocationErrorResponseType>,
  EndpointSet,
  EndpointNotSet,
  EndpointNotSet,
  EndpointNotSet,
  EndpointSet,
>;

pub fn init_oauth_client(config: &crate::conf::Config) -> Client {
  let client_id = ClientId::new(config.oauth.client_id.clone());
  let client_secret = ClientSecret::new(config.oauth.client_secret.clone());
  let auth_url = AuthUrl::new(AUTH_URL.to_string()).expect("Invalid authorization endpoint URL");
  let token_url = TokenUrl::new(TOKEN_URL.to_string()).expect("Invalid token endpoint URL");
  let redirect_url = RedirectUrl::new(config.domain.to_string() + "/oauth-callback/codeberg")
    .expect("Invalid redirect URL");

  BasicClient::new(client_id)
    .set_client_secret(client_secret)
    .set_auth_uri(auth_url)
    .set_token_uri(token_url)
    .set_redirect_uri(redirect_url)
}

// Handlers

pub async fn login_codeberg(State(state): State<Arc<AppState>>) -> impl IntoResponse {
  let (auth_url, _csrf_token) = state
    .oauth_client
    .authorize_url(CsrfToken::new_random)
    .url();
  Redirect::to(auth_url.as_str())
}

#[derive(serde::Deserialize)]
pub struct AuthRequest {
  code: String,
  state: String,
}

#[derive(serde::Deserialize)]
struct CodebergUser {
  id: i64,
  username: String,
}

pub async fn callback_codeberg(
  State(state): State<Arc<AppState>>,
  cookies: Cookies,
  Query(params): Query<AuthRequest>,
) -> Result<Redirect, AppError> {
  let code = AuthorizationCode::new(params.code);
  let _state = CsrfToken::new(params.state.clone());

  let http_client = reqwest::ClientBuilder::new()
    .redirect(reqwest::redirect::Policy::none())
    .build()
    .expect("Client should build");

  let token = state
    .oauth_client
    .exchange_code(code)
    .request_async(&http_client)
    .await;

  if token.is_err() {
    tracing::error!("exchange_code failed, error: {:?}", token.unwrap_err());
    return Err(anyhow!("exchange_code failed").into());
  }
  let token = token.unwrap();

  let user_info: CodebergUser = serde_json::from_str(
    &http_client
      .get(format!("{}/user", CODEBERG_API_BASE_URL))
      .header(
        "Authorization",
        format!("Bearer {}", token.access_token().secret()),
      )
      .send()
      .await?
      .text()
      .await?,
  )?;

  // Check if user exists to handle roles
  let user = if let Some(existing) = state.users.get_mut(&user_info.id) {
    existing.clone()
  } else {
    let new_user = User {
      id: user_info.id,
      name: user_info.username,
      password: None,
      role: Role::Normal,
      valid_after: chrono::Utc::now().timestamp(),
    };
    state.users.insert(user_info.id, new_user.clone());
    new_user
  };

  if user.role == Role::Banned {
    return Err(anyhow!("User is banned").into());
  }

  // Generate Token
  let jwt = state.token_manager.generate_token(&user);
  cookies.add(Cookie::build(("token", jwt)).path("/").build());

  Ok(Redirect::to("/"))
}
