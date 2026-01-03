use axum::{
  http::StatusCode,
  response::{IntoResponse, Response},
};
use std::fmt::Debug;

// Make our own error that wraps `anyhow::Error`.
pub struct AppError(anyhow::Error);

// 允许直接使用 ? 转换各种错误
impl<E> From<E> for AppError
where
  E: Into<anyhow::Error>,
{
  fn from(err: E) -> Self {
    Self(err.into())
  }
}

impl IntoResponse for AppError {
  fn into_response(self) -> Response {
    // 1. 在服务端记录详细的错误堆栈和信息
    // 使用 tracing 记录 error 级别日志
    tracing::error!("Application error: {:#}", self.0);

    #[cfg(debug_assertions)]
    let message = format!(
      "Something went wrong:\n{}\n\nBacktrace:\n{}",
      self.0,
      self.0.backtrace()
    );

    #[cfg(not(debug_assertions))]
    let message = format!("Something went wrong: {}", self.0);

    (StatusCode::INTERNAL_SERVER_ERROR, message).into_response()
  }
}

// 为 AppError 实现 Debug，方便调试
impl Debug for AppError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{:?}", self.0)
  }
}
