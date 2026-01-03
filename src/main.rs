pub mod auth;
pub mod conf;
pub mod error;
pub mod game;
pub mod middleware;
pub mod models;
pub mod routes;
pub mod state;
pub mod ws;

use anyhow::Result;
use state::AppState;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
  tracing_subscriber::registry()
    .with(
      tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,quiz_time=debug".into()),
    )
    .with(tracing_subscriber::fmt::layer())
    .init();

  let app_state = Arc::new(AppState::new()?);

  // Background tick loop
  let bg_state = app_state.clone();
  tokio::spawn(async move {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
    loop {
      interval.tick().await;
      // Iterate over all rooms safely
      // Note: dashmap iter doesn't lock everything, but we need to write lock specific rooms
      // to update game state. This is safe.
      for r in bg_state.rooms.iter() {
        let mut room = r.value().write().await;
        room.tick(&bg_state.global_tx);
      }
    }
  });

  let app = routes::app(app_state);
  let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
  tracing::info!("Listening on 0.0.0.0:8080");
  axum::serve(listener, app).await?;
  Ok(())
}
