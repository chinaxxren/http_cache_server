use tracing::{info, warn, error};
use tracing_subscriber::{fmt, EnvFilter};

pub fn init_logger() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env()
            .add_directive("http_cache=info".parse().unwrap()))
        .with_thread_ids(true)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .init();
}

pub fn log_request(path: &str, duration: std::time::Duration) {
    info!(
        target: "request",
        path = %path,
        duration_ms = %duration.as_millis(),
        "Request completed"
    );
} 