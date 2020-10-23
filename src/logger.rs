use kv_log_macro as log;
use tide::{Middleware, Next, Request, Result};

/// Log all incoming requests and responses.
///
/// This middleware is enabled by default in Tide. In the case of
/// nested applications, this middleware will only run once for each
/// request.
///
/// # Examples
///
/// ```
/// let mut app = tide::Server::new();
/// app.with(tide::log::LogMiddleware::new());
/// ```
#[derive(Debug, Default, Clone)]
pub struct LogMiddleware {
    _priv: (),
}

struct LogMiddlewareHasBeenRun;

impl LogMiddleware {
    /// Create a new instance of `LogMiddleware`.
    #[must_use]
    pub fn new() -> Self {
        Self { _priv: () }
    }

    /// Log a request and a response.
    async fn log<'a, State: Clone + Send + Sync + 'static>(
        &'a self,
        mut req: Request<State>,
        next: Next<'a, State>,
    ) -> Result {
        if req.ext::<LogMiddlewareHasBeenRun>().is_some() {
            return Ok(next.run(req).await);
        }
        req.set_ext(LogMiddlewareHasBeenRun);

        let start = std::time::Instant::now();
        let response = next.run(req).await;
        let status = response.status();

        if status.is_server_error() {
            if let Some(error) = response.error() {
                log::error!("Internal error --> Response sent", {
                    message: format!("{:?}", error),
                    error_type: error.type_name(),
                    status: format!("{} - {}", status as u16, status.canonical_reason()),
                    duration: format!("{:?}", start.elapsed()),
                });
            } else {
                log::error!("Internal error --> Response sent", {
                    status: format!("{} - {}", status as u16, status.canonical_reason()),
                    duration: format!("{:?}", start.elapsed()),
                });
            }
        } else if status.is_client_error() {
            if let Some(error) = response.error() {
                log::warn!("Client error --> Response sent", {
                    message: format!("{:?}", error),
                    error_type: error.type_name(),
                    status: format!("{} - {}", status as u16, status.canonical_reason()),
                    duration: format!("{:?}", start.elapsed()),
                });
            } else {
                log::warn!("Client error --> Response sent", {
                    status: format!("{} - {}", status as u16, status.canonical_reason()),
                    duration: format!("{:?}", start.elapsed()),
                });
            }
        } else {
        }
        Ok(response)
    }
}

#[tide::utils::async_trait]
impl<State: Clone + Send + Sync + 'static> Middleware<State> for LogMiddleware {
    async fn handle(&self, req: Request<State>, next: Next<'_, State>) -> Result {
        self.log(req, next).await
    }
}
