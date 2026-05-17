use async_request_queue_challenge::{QueueConfig, RequestQueue, RequestQueueError};
use std::future::Future;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CAPACITY_STANDARD: usize = 4;
const ENQUEUE_TIMEOUT_MS: u64 = 100;
const EXECUTION_TIMEOUT_MS: u64 = 100;
const EXECUTION_TIMEOUT_SHORT_MS: u64 = 10;
const JOB_SLEEP_MS: u64 = 200;
const SECOND_REQUEST_TIMEOUT_MS: u64 = 80;
const REPLY_TIMEOUT_S: u64 = 2;
const SHUTDOWN_TIMEOUT_S: u64 = 1;
const EXPECTED_FOLLOW_UP: u32 = 11;
const EXPECTED_SECOND_VALUE: u32 = 2;

const fn enqueue_timeout() -> Duration {
    Duration::from_millis(ENQUEUE_TIMEOUT_MS)
}

const fn execution_timeout() -> Duration {
    Duration::from_millis(EXECUTION_TIMEOUT_MS)
}

const fn execution_timeout_short() -> Duration {
    Duration::from_millis(EXECUTION_TIMEOUT_SHORT_MS)
}

const fn reply_timeout() -> Duration {
    Duration::from_secs(REPLY_TIMEOUT_S)
}

const fn shutdown_timeout() -> Duration {
    Duration::from_secs(SHUTDOWN_TIMEOUT_S)
}

const fn blocking_sleep_duration() -> Duration {
    Duration::from_millis(JOB_SLEEP_MS)
}

const fn second_request_timeout() -> Duration {
    Duration::from_millis(SECOND_REQUEST_TIMEOUT_MS)
}

fn panic_before_future() -> impl Future<Output = Result<u32, RequestQueueError>> + Send {
    let should_panic = true;
    assert!(!should_panic, "factory panic before creating future");

    async { Ok(1) }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();

    let mut dir = std::env::temp_dir();
    dir.push(format!("{prefix}-{nanos}"));
    dir
}

#[tokio::test]
async fn factory_panic_isolated_and_queue_recovers() {
    let config = QueueConfig {
        capacity: CAPACITY_STANDARD,
        enqueue_timeout: enqueue_timeout(),
        execution_timeout: execution_timeout(),
        reply_timeout: reply_timeout(),
    };

    let (queue, shutdown) = RequestQueue::<u32>::new(config).expect("queue construction should succeed");

    let panic_result = queue.enqueue(panic_before_future).await;
    assert!(
        matches!(panic_result, Err(RequestQueueError::JobPanicked)),
        "expected JobPanicked, got: {panic_result:?}"
    );

    let follow_up = queue.enqueue(|| async { Ok(EXPECTED_FOLLOW_UP) }).await;
    assert_eq!(follow_up, Ok(EXPECTED_FOLLOW_UP), "queue should recover after panic");

    drop(queue);

    let shutdown_result = tokio::time::timeout(shutdown_timeout(), shutdown.wait()).await;
    assert!(
        shutdown_result.is_ok(),
        "shutdown should complete within timeout, got: {shutdown_result:?}"
    );

    let wait_result = shutdown_result.expect("timeout branch already asserted as impossible");
    assert_eq!(wait_result, Ok(()), "shutdown should complete cleanly");
}

#[tokio::test(flavor = "multi_thread")]
async fn blocking_job_timeout_does_not_stall_queue() {
    let config = QueueConfig {
        capacity: CAPACITY_STANDARD,
        enqueue_timeout: enqueue_timeout(),
        execution_timeout: execution_timeout_short(),
        reply_timeout: reply_timeout(),
    };

    let (queue, shutdown) = RequestQueue::<u32>::new(config).expect("queue construction should succeed");

    let timeout_result = queue
        .enqueue(|| async {
            std::thread::sleep(blocking_sleep_duration());
            Ok(1)
        })
        .await;

    assert!(
        matches!(
            timeout_result,
            Err(RequestQueueError::ExecutionTimedOut { timeout }) if timeout == execution_timeout_short()
        ),
        "expected ExecutionTimedOut with short timeout, got: {timeout_result:?}"
    );

    let second_result = tokio::time::timeout(second_request_timeout(), queue.enqueue(|| async { Ok(EXPECTED_SECOND_VALUE) })).await;

    let second_queue_result = second_result.expect("queue should not remain stalled after execution timeout");
    assert_eq!(
        second_queue_result,
        Ok(EXPECTED_SECOND_VALUE),
        "follow-up request should succeed after timeout"
    );

    drop(queue);

    let shutdown_result = tokio::time::timeout(shutdown_timeout(), shutdown.wait()).await;
    assert!(
        shutdown_result.is_ok(),
        "shutdown should complete within timeout, got: {shutdown_result:?}"
    );

    let wait_result = shutdown_result.expect("timeout branch already asserted as impossible");
    assert_eq!(wait_result, Ok(()), "shutdown should complete cleanly");
}

#[test]
fn code_security_fails_when_geiger_fails_even_with_valid_looking_output() {
    let fake_bin_dir = unique_temp_dir("fake-cargo-bin");
    std::fs::create_dir_all(&fake_bin_dir).expect("failed to create fake cargo dir");

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

    let mut file = std::fs::File::create(&fake_cargo_path).expect("failed creating fake cargo script");
    file.write_all(script.as_bytes()).expect("failed writing fake cargo script");

    let metadata = std::fs::metadata(&fake_cargo_path).expect("failed reading fake cargo metadata");
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_cargo_path, permissions).expect("failed making fake cargo executable");

    let original_path = std::env::var("PATH").expect("PATH should be available for test execution");
    let composite_path = format!("{}:{}", fake_bin_dir.display(), original_path);

    let output = Command::new("just")
        .arg("code-security")
        .env("PATH", composite_path)
        .output()
        .expect("failed to run just code-security");

    assert!(
        !output.status.success(),
        "expected code-security to fail when cargo geiger exits non-zero"
    );
}
