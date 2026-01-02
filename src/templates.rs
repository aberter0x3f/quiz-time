use askama::Template;

#[derive(Template)]
#[template(path = "game.html")]
pub struct GameTemplate {
  pub username: String,
  pub is_spectate: bool,
  pub is_super: bool,
}

pub fn render_game(username: &str, is_spectate: bool, is_super: bool) -> GameTemplate {
  GameTemplate {
    username: username.to_string(),
    is_spectate,
    is_super,
  }
}
