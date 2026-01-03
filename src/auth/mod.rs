pub mod oauth;
pub mod token;

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum Role {
  Admin,
  Normal,
  Banned,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
  pub id: i64,
  pub name: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub password: Option<String>,
  pub role: Role,
  #[serde(skip, default)]
  pub valid_after: i64,
}

impl User {
  pub fn is_admin(&self) -> bool {
    self.role == Role::Admin
  }
}
