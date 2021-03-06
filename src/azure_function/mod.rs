use std::sync::Arc;

use async_std::sync::RwLock;

mod http_context_transform;

#[macro_use]
pub mod logger;

pub use http_context_transform::AzureFnMiddleware;
pub use logger::LogMiddleware as AzureFnLogMiddleware;

pub type AzureFnLogger = Arc<RwLock<AzureFnLoggerInner>>;

#[derive(Debug)]
pub struct AzureFnLoggerInner {
    logs: Vec<String>,
    invocation_id: String,
}

/// Makes logging to the AzureFnLogger less code-verbose.
#[tide::utils::async_trait]
pub trait AzureFnLoggerExt {
    async fn log(&mut self, log_line: String);
}

#[tide::utils::async_trait]
impl AzureFnLoggerExt for AzureFnLogger {
    #[must_use = "requires await"]
    async fn log(&mut self, log_line: String) {
        let mut inner = self.write().await;
        let line = format!("{} {}", inner.invocation_id, log_line);
        inner.logs.push(line);
    }
}

#[tide::utils::async_trait]
impl AzureFnLoggerExt for &'_ mut AzureFnLogger {
    #[must_use = "requires await"]
    async fn log(&mut self, log_line: String) {
        let mut inner = self.write().await;
        let line = format!("{} {}", inner.invocation_id, log_line);
        inner.logs.push(line);
    }
}
