//! Binary entrypoint for the async request queue demo.

use std::time::Duration;

use anyhow::Context;
use async_request_queue_challenge::{QueueConfig, RequestQueue};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let config = QueueConfig {
        capacity: 8,
        enqueue_timeout: Duration::from_secs(1),
        execution_timeout: Duration::from_secs(1),
        reply_timeout: Duration::from_secs(2),
    };

    let queue_result = RequestQueue::<&'static str>::new(config);
    let (queue, shutdown) = queue_result.context("failed to create request queue")?;

    let response = queue
        .enqueue(|| async { Ok("queue online") })
        .await
        .context("failed to execute queued request")?;

    println!("{response}");

    drop(queue);
    shutdown.wait().await.context("worker shutdown failed")?;

    Ok(())
}
