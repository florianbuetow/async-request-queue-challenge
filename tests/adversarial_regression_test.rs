use async_request_queue_challenge::{QueueConfig, RequestQueue, RequestQueueError};
use std::future::Future;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn panic_before_future() -> impl Future<Output = Result<u32, RequestQueueError>> + Send {
    let should_panic = true;
    assert!(!should_panic, "factory panic before creating future");

    async { Ok(1) }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos(),
        Err(error) => panic!("system clock is before unix epoch: {error}"),
    };

    let mut dir = std::env::temp_dir();
    dir.push(format!("{prefix}-{nanos}"));
    dir
}

#[tokio::test]
async fn factory_panic_isolated_and_queue_recovers() {
    let config = QueueConfig {
        capacity: 4,
        enqueue_timeout: Duration::from_millis(100),
        execution_timeout: Duration::from_millis(100),
        reply_timeout: Duration::from_secs(2),
    };

    let create_result = RequestQueue::<u32>::new(config);
    let (queue, shutdown) = match create_result {
        Ok(ok) => ok,
        Err(error) => panic!("failed to create queue: {error}"),
    };

    let panic_result = queue.enqueue(panic_before_future).await;

    match panic_result {
        Err(RequestQueueError::JobPanicked) => {}
        Ok(value) => panic!("expected JobPanicked, got success value: {value}"),
        Err(error) => panic!("expected JobPanicked, got different error: {error}"),
    }

    let follow_up = queue.enqueue(|| async { Ok(11) }).await;
    match follow_up {
        Ok(value) => assert_eq!(value, 11),
        Err(error) => panic!("queue did not recover after factory panic: {error}"),
    }

    drop(queue);
    let shutdown_result = tokio::time::timeout(Duration::from_secs(1), shutdown.wait()).await;
    match shutdown_result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("shutdown failed unexpectedly: {error}"),
        Err(elapsed) => panic!("shutdown timed out unexpectedly: {elapsed}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocking_job_timeout_does_not_stall_queue() {
    let config = QueueConfig {
        capacity: 4,
        enqueue_timeout: Duration::from_millis(100),
        execution_timeout: Duration::from_millis(10),
        reply_timeout: Duration::from_secs(2),
    };

    let create_result = RequestQueue::<u32>::new(config);
    let (queue, shutdown) = match create_result {
        Ok(ok) => ok,
        Err(error) => panic!("failed to create queue: {error}"),
    };

    let timeout_result = queue
        .enqueue(|| async {
            std::thread::sleep(Duration::from_millis(200));
            Ok(1)
        })
        .await;

    match timeout_result {
        Err(RequestQueueError::ExecutionTimedOut { timeout }) => {
            assert_eq!(timeout, Duration::from_millis(10));
        }
        Ok(value) => panic!("expected timeout, got success value: {value}"),
        Err(error) => panic!("expected timeout, got different error: {error}"),
    }

    let second_result = tokio::time::timeout(Duration::from_millis(80), queue.enqueue(|| async { Ok(2) })).await;

    match second_result {
        Ok(Ok(value)) => assert_eq!(value, 2),
        Ok(Err(error)) => panic!("second request failed unexpectedly: {error}"),
        Err(elapsed) => panic!("queue remained stalled after timeout: {elapsed}"),
    }

    drop(queue);
    let shutdown_result = tokio::time::timeout(Duration::from_secs(1), shutdown.wait()).await;
    match shutdown_result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("shutdown failed unexpectedly: {error}"),
        Err(elapsed) => panic!("shutdown timed out unexpectedly: {elapsed}"),
    }
}

#[test]
fn code_security_fails_when_geiger_fails_even_with_valid_looking_output() {
    let fake_bin_dir = unique_temp_dir("fake-cargo-bin");

    if let Err(error) = std::fs::create_dir_all(&fake_bin_dir) {
        panic!("failed to create fake cargo dir: {error}");
    }

    let fake_cargo_path = fake_bin_dir.join("cargo");
    let script = r#"#!/usr/bin/env bash
if [ "$1" = "geiger" ]; then
  printf "Functions  Expressions  Impls  Traits  Methods  Dependency\n"
  printf "0/0        0/0          0/0    0/0     0/0      ?  async-request-queue-challenge 0.1.0\n"
  exit 23
fi
printf "unexpected cargo invocation: %s\n" "$*" >&2
exit 99
"#;

    match std::fs::File::create(&fake_cargo_path) {
        Ok(mut file) => {
            if let Err(error) = file.write_all(script.as_bytes()) {
                panic!("failed writing fake cargo script: {error}");
            }
        }
        Err(error) => panic!("failed creating fake cargo script: {error}"),
    }

    let metadata = match std::fs::metadata(&fake_cargo_path) {
        Ok(meta) => meta,
        Err(error) => panic!("failed reading fake cargo metadata: {error}"),
    };
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o755);
    if let Err(error) = std::fs::set_permissions(&fake_cargo_path, permissions) {
        panic!("failed making fake cargo executable: {error}");
    }

    let original_path = match std::env::var("PATH") {
        Ok(path) => path,
        Err(error) => panic!("PATH is unavailable: {error}"),
    };
    let composite_path = format!("{}:{}", fake_bin_dir.display(), original_path);

    let output = match Command::new("just").arg("code-security").env("PATH", composite_path).output() {
        Ok(out) => out,
        Err(error) => panic!("failed to run just code-security: {error}"),
    };

    assert!(
        !output.status.success(),
        "expected code-security to fail when cargo geiger exits non-zero"
    );
}
