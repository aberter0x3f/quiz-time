use askama::Template;

#[derive(Template)]
#[template(path = "game.html")]
pub struct GameTemplate {
  pub username: String,
  pub is_spectate: bool,
}

pub fn render_game(username: &str, is_spectate: bool) -> GameTemplate {
  GameTemplate {
    username: username.to_string(),
    is_spectate,
  }
}
