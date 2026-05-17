use async_request_queue_challenge::{QueueConfig, RequestQueue};
use std::time::Duration;

#[tokio::test]
async fn queue_rejects_zero_capacity() {
    let config = QueueConfig {
        capacity: 0,
        enqueue_timeout: Duration::from_millis(10),
        execution_timeout: Duration::from_millis(10),
        reply_timeout: Duration::from_secs(1),
    };

    let result = RequestQueue::<u8>::new(config);

    match result {
        Ok((_queue, _shutdown)) => {
            panic!("expected queue construction to fail for zero capacity");
        }
        Err(error) => {
            assert_eq!(error.to_string(), "queue capacity must be at least 1");
        }
    }
}

#[tokio::test]
async fn queue_processes_requests_from_integration_test() {
    let config = QueueConfig {
        capacity: 2,
        enqueue_timeout: Duration::from_millis(20),
        execution_timeout: Duration::from_millis(20),
        reply_timeout: Duration::from_secs(1),
    };

    let new_result = RequestQueue::<u8>::new(config);
    let (queue, shutdown) = match new_result {
        Ok(ok) => ok,
        Err(error) => panic!("failed to construct queue unexpectedly: {error}"),
    };

    let response = queue.enqueue(|| async { Ok(5) }).await;

    match response {
        Ok(value) => {
            assert_eq!(value, 5);
        }
        Err(error) => panic!("expected successful queue response, got: {error}"),
    }

    drop(queue);

    let shutdown_result = tokio::time::timeout(Duration::from_millis(200), shutdown.wait()).await;
    match shutdown_result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("expected clean worker shutdown, got: {error}"),
        Err(elapsed) => panic!("expected worker shutdown to complete within timeout: {elapsed}"),
    }
}
