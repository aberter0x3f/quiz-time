use serde::{Deserialize, Serialize};
use std::env;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
  pub domain: String,
  pub oauth: OAuthConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OAuthConfig {
  pub client_id: String,
  pub client_secret: String,
}

impl Config {
  pub fn load() -> Self {
    Self {
      domain: env::var("DOMAIN").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string()),
      oauth: OAuthConfig {
        client_id: env::var("OAUTH_CLIENT_ID").unwrap(),
        client_secret: env::var("OAUTH_CLIENT_SECRET").unwrap(),
      },
    }
  }
}
