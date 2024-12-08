use std::time::Duration;
use tokio::time::sleep;
use crate::error::PluginError;
use tracing::warn;

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
        match f().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                attempts += 1;
                if attempts >= retries {
                    return Err(e);
                }
                warn!("Retry attempt {} after error: {}", attempts, e);
                sleep(delay).await;
            }
        }
    }
} 