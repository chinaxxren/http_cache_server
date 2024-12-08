use std::time::Duration;
use tokio::time::sleep;
use crate::error::PluginError;
use tracing::{info, warn, error, debug};

pub(crate) async fn with_retry<F, Fut, T>(
    f: F,
    retries: u32,
    delay: Duration,
) -> Result<T, PluginError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, PluginError>>,
{
    let mut attempts = 0;
    loop {
        debug!("Attempt {} of {}", attempts + 1, retries);
        match f().await {
            Ok(result) => {
                if attempts > 0 {
                    info!("Operation succeeded after {} retries", attempts);
                }
                return Ok(result);
            }
            Err(e) => {
                attempts += 1;
                if attempts >= retries {
                    error!("Operation failed after {} attempts: {}", attempts, e);
                    return Err(e);
                }
                warn!(
                    "Attempt {} failed: {}. Retrying in {:?}...",
                    attempts, e, delay
                );
                sleep(delay).await;
            }
        }
    }
} 