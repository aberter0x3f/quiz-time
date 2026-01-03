use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use rand::RngCore;
use serde::{Deserialize, Serialize};

const TOKEN_VALIDITY_SECONDS: i64 = 60 * 60 * 24 * 7 - 1; // 7 days

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
  pub sub: i64,
  pub name: String,
  pub role: String,
  pub iat: i64,
  pub exp: usize,
}

pub struct TokenManager {
  encoding_key: EncodingKey,
  decoding_key: DecodingKey,
}

impl TokenManager {
  pub fn new() -> Self {
    let mut key_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key_bytes);

    let encoding_key = EncodingKey::from_secret(&key_bytes);
    let decoding_key = DecodingKey::from_secret(&key_bytes);

    Self {
      encoding_key,
      decoding_key,
    }
  }

  pub fn generate_token(&self, user: &super::User) -> String {
    let now = Utc::now();
    let claims = Claims {
      sub: user.id,
      name: user.name.clone(),
      role: user.role.to_string(),
      iat: now.timestamp(),
      exp: (now + Duration::seconds(TOKEN_VALIDITY_SECONDS))
        .timestamp()
        .try_into()
        .unwrap(),
    };
    encode(&Header::default(), &claims, &self.encoding_key).unwrap()
  }

  pub fn parse_token(&self, token: &str) -> Option<Claims> {
    let validation = Validation::new(Algorithm::HS256);
    decode::<Claims>(token, &self.decoding_key, &validation)
      .ok()
      .map(|data| data.claims)
  }
}
