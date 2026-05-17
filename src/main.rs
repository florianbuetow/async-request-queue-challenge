//! Binary entrypoint for the async request queue demo.

use std::time::Duration;

use anyhow::Context;
use async_request_queue_challenge::{QueueConfig, RequestQueue};

const DEMO_CAPACITY: usize = 8;
const DEMO_ENQUEUE_TIMEOUT_S: u64 = 1;
const DEMO_EXECUTION_TIMEOUT_S: u64 = 1;
const DEMO_REPLY_TIMEOUT_S: u64 = 2;

const fn demo_enqueue_timeout() -> Duration {
    Duration::from_secs(DEMO_ENQUEUE_TIMEOUT_S)
}

const fn demo_execution_timeout() -> Duration {
    Duration::from_secs(DEMO_EXECUTION_TIMEOUT_S)
}

const fn demo_reply_timeout() -> Duration {
    Duration::from_secs(DEMO_REPLY_TIMEOUT_S)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let config = QueueConfig {
        capacity: DEMO_CAPACITY,
        enqueue_timeout: demo_enqueue_timeout(),
        execution_timeout: demo_execution_timeout(),
        reply_timeout: demo_reply_timeout(),
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
