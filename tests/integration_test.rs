use async_request_queue_challenge::{QueueConfig, RequestQueue, RequestQueueBuildError};
use std::time::Duration;

const ZERO_CAPACITY: usize = 0;
const DEFAULT_CAPACITY: usize = 2;
const ENQUEUE_TIMEOUT_MS: u64 = 20;
const EXECUTION_TIMEOUT_MS: u64 = 20;
const REPLY_TIMEOUT_S: u64 = 1;
const SHUTDOWN_TIMEOUT_MS: u64 = 200;
const EXPECTED_VALUE: u8 = 5;

const fn enqueue_timeout() -> Duration {
    Duration::from_millis(ENQUEUE_TIMEOUT_MS)
}

const fn execution_timeout() -> Duration {
    Duration::from_millis(EXECUTION_TIMEOUT_MS)
}

const fn reply_timeout() -> Duration {
    Duration::from_secs(REPLY_TIMEOUT_S)
}

const fn shutdown_timeout() -> Duration {
    Duration::from_millis(SHUTDOWN_TIMEOUT_MS)
}

#[tokio::test]
async fn queue_rejects_zero_capacity() {
    let config = QueueConfig {
        capacity: ZERO_CAPACITY,
        enqueue_timeout: enqueue_timeout(),
        execution_timeout: execution_timeout(),
        reply_timeout: reply_timeout(),
    };

    let result = RequestQueue::<u8>::new(config);

    assert!(
        matches!(result, Err(RequestQueueBuildError::InvalidCapacity)),
        "expected InvalidCapacity, got: {result:?}"
    );
}

#[tokio::test]
async fn queue_processes_requests_from_integration_test() {
    let config = QueueConfig {
        capacity: DEFAULT_CAPACITY,
        enqueue_timeout: enqueue_timeout(),
        execution_timeout: execution_timeout(),
        reply_timeout: reply_timeout(),
    };

    let (queue, shutdown) = RequestQueue::<u8>::new(config).expect("queue construction should succeed for valid config");

    let response = queue.enqueue(|| async { Ok(EXPECTED_VALUE) }).await;
    assert_eq!(response, Ok(EXPECTED_VALUE), "queue returned unexpected response");

    drop(queue);

    let shutdown_result = tokio::time::timeout(shutdown_timeout(), shutdown.wait()).await;
    assert!(
        shutdown_result.is_ok(),
        "expected worker shutdown to complete within timeout, got: {shutdown_result:?}"
    );

    let wait_result = shutdown_result.expect("timeout branch already asserted as impossible");
    assert_eq!(wait_result, Ok(()), "expected clean worker shutdown");
}
