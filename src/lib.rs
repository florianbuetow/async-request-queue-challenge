//! Actor-style asynchronous request queue based on Tokio channels.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::future::Future;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

/// Output type returned by queued jobs.
pub type QueueResult<T> = Result<T, RequestQueueError>;

struct Job<T> {
    task: Box<dyn FnOnce() -> tokio::task::JoinHandle<QueueResult<T>> + Send + 'static>,
    reply_tx: oneshot::Sender<QueueResult<T>>,
    execution_timeout: Duration,
}

/// Build-time configuration for a [`RequestQueue`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueueConfig {
    /// Bounded queue capacity. Must be at least `1`.
    pub capacity: usize,
    /// Maximum time to wait while trying to enqueue when the queue is full.
    pub enqueue_timeout: Duration,
    /// Default maximum execution time for each queued job.
    pub execution_timeout: Duration,
    /// Maximum time to wait for a queued job response once accepted.
    pub reply_timeout: Duration,
}

/// Error returned when constructing a [`RequestQueue`].
#[derive(Debug, Error, Eq, PartialEq)]
pub enum RequestQueueBuildError {
    /// Queue capacity was zero.
    #[error("queue capacity must be at least 1")]
    InvalidCapacity,
}

/// Runtime errors produced by queue operations.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RequestQueueError {
    /// The queue is closed because the worker has stopped.
    #[error("queue closed")]
    QueueClosed,
    /// Waiting to enqueue exceeded the configured timeout.
    #[error("enqueue timed out after {timeout:?}")]
    EnqueueTimedOut {
        /// Timeout value that elapsed.
        timeout: Duration,
    },
    /// The worker exceeded the job timeout.
    #[error("job execution timed out after {timeout:?}")]
    ExecutionTimedOut {
        /// Timeout value that elapsed.
        timeout: Duration,
    },
    /// Waiting for a queued job response exceeded the configured timeout.
    #[error("reply timed out after {timeout:?}")]
    ReplyTimedOut {
        /// Timeout value that elapsed.
        timeout: Duration,
    },
    /// A queued job panicked during execution.
    #[error("job panicked during execution")]
    JobPanicked,
    /// The worker dropped before replying to the request.
    #[error("worker dropped before replying")]
    WorkerDropped,
    /// A queued job was cancelled before producing a result.
    #[error("job was cancelled before producing a result")]
    JobCancelled,
}

/// Handle used to wait for the queue worker task to finish.
#[derive(Debug)]
pub struct QueueShutdown {
    done_rx: oneshot::Receiver<()>,
}

impl QueueShutdown {
    /// Waits until the queue worker exits.
    ///
    /// The worker exits after all senders are dropped and pending jobs are drained.
    ///
    /// # Errors
    ///
    /// Returns [`RequestQueueError::WorkerDropped`] if the internal shutdown signal
    /// cannot be received.
    pub async fn wait(self) -> QueueResult<()> {
        self.done_rx.await.map_err(|_recv_error| RequestQueueError::WorkerDropped)
    }
}

/// Cloneable producer handle for the request queue.
#[derive(Clone, Debug)]
pub struct RequestQueue<T> {
    tx: mpsc::Sender<Job<T>>,
    enqueue_timeout: Duration,
    execution_timeout: Duration,
    reply_timeout: Duration,
}

impl<T> RequestQueue<T>
where
    T: Send + 'static,
{
    /// Creates a queue and starts the dedicated worker task.
    ///
    /// Returns a cloneable queue handle and a shutdown waiter.
    ///
    /// # Errors
    ///
    /// Returns [`RequestQueueBuildError::InvalidCapacity`] when `config.capacity == 0`.
    ///
    /// # Panics
    ///
    /// Panics if called outside a Tokio runtime because this function spawns
    /// the dedicated worker task with [`tokio::spawn`].
    pub fn new(config: QueueConfig) -> Result<(Self, QueueShutdown), RequestQueueBuildError> {
        if config.capacity == 0 {
            return Err(RequestQueueBuildError::InvalidCapacity);
        }

        let (tx, mut rx) = mpsc::channel::<Job<T>>(config.capacity);
        let (done_tx, done_rx) = oneshot::channel::<()>();

        tokio::spawn(async move {
            while let Some(job) = rx.recv().await {
                if job.reply_tx.is_closed() {
                    continue;
                }

                let mut handle = (job.task)();

                let result = match tokio::time::timeout(job.execution_timeout, &mut handle).await {
                    Ok(Ok(job_result)) => job_result,
                    Ok(Err(join_error)) => {
                        if join_error.is_panic() {
                            Err(RequestQueueError::JobPanicked)
                        } else if join_error.is_cancelled() {
                            Err(RequestQueueError::JobCancelled)
                        } else {
                            Err(RequestQueueError::WorkerDropped)
                        }
                    }
                    Err(_elapsed) => {
                        handle.abort();
                        Err(RequestQueueError::ExecutionTimedOut {
                            timeout: job.execution_timeout,
                        })
                    }
                };

                if job.reply_tx.send(result).is_err() {}
            }

            if done_tx.send(()).is_err() {}
        });

        Ok((
            Self {
                tx,
                enqueue_timeout: config.enqueue_timeout,
                execution_timeout: config.execution_timeout,
                reply_timeout: config.reply_timeout,
            },
            QueueShutdown { done_rx },
        ))
    }

    /// Enqueues a job using the default execution timeout.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`RequestQueueError::EnqueueTimedOut`] when backpressure persists past `enqueue_timeout`.
    /// - [`RequestQueueError::QueueClosed`] when the worker is no longer running.
    /// - [`RequestQueueError::ExecutionTimedOut`] when the job exceeds the execution timeout.
    /// - [`RequestQueueError::ReplyTimedOut`] when waiting for the job response exceeds `reply_timeout`.
    /// - [`RequestQueueError::WorkerDropped`] when the worker exits before replying.
    /// - [`RequestQueueError::JobPanicked`] when the queued job panics.
    pub async fn enqueue<F, Fut>(&self, f: F) -> QueueResult<T>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = QueueResult<T>> + Send + 'static,
    {
        self.enqueue_with_timeout(self.execution_timeout, f).await
    }

    /// Enqueues a job and overrides its execution timeout.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`RequestQueueError::EnqueueTimedOut`] when backpressure persists past `enqueue_timeout`.
    /// - [`RequestQueueError::QueueClosed`] when the worker is no longer running.
    /// - [`RequestQueueError::ExecutionTimedOut`] when the job exceeds `execution_timeout`.
    /// - [`RequestQueueError::ReplyTimedOut`] when waiting for the job response exceeds `reply_timeout`.
    /// - [`RequestQueueError::WorkerDropped`] when the worker exits before replying.
    /// - [`RequestQueueError::JobPanicked`] when the queued job panics.
    pub async fn enqueue_with_timeout<F, Fut>(&self, execution_timeout: Duration, f: F) -> QueueResult<T>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = QueueResult<T>> + Send + 'static,
    {
        let (reply_tx, reply_rx) = oneshot::channel::<QueueResult<T>>();

        let job = Job {
            task: Box::new(move || tokio::spawn(async move { f().await })),
            reply_tx,
            execution_timeout,
        };

        let send_result = tokio::time::timeout(self.enqueue_timeout, self.tx.send(job)).await;

        let send_outcome = send_result.map_err(|_elapsed| RequestQueueError::EnqueueTimedOut {
            timeout: self.enqueue_timeout,
        })?;

        send_outcome.map_err(|_send_error| RequestQueueError::QueueClosed)?;

        let receive_result = tokio::time::timeout(self.reply_timeout, reply_rx).await;
        match receive_result {
            Ok(Ok(job_result)) => job_result,
            Ok(Err(_recv_error)) => Err(RequestQueueError::WorkerDropped),
            Err(_elapsed) => Err(RequestQueueError::ReplyTimedOut {
                timeout: self.reply_timeout,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{QueueConfig, QueueResult, QueueShutdown, RequestQueue, RequestQueueError};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tokio::sync::oneshot;

    const DEFAULT_CAPACITY: usize = 1;
    const DEFAULT_ENQUEUE_TIMEOUT_MS: u64 = 60;
    const DEFAULT_EXECUTION_TIMEOUT_MS: u64 = 200;
    const DEFAULT_REPLY_TIMEOUT_S: u64 = 2;
    const SHUTDOWN_WAIT_TIMEOUT_MS: u64 = 200;
    const SHORT_EXECUTION_TIMEOUT_MS: u64 = 10;
    const LONG_JOB_SLEEP_MS: u64 = 60;
    const POST_TIMEOUT_WAIT_MS: u64 = 80;
    const REPLY_TEST_ENQUEUE_TIMEOUT_MS: u64 = 100;
    const REPLY_TEST_EXECUTION_TIMEOUT_S: u64 = 1;
    const REPLY_TEST_REPLY_TIMEOUT_MS: u64 = 40;
    const REPLY_TEST_LONG_JOB_MS: u64 = 120;
    const REPLY_TEST_STAGGER_MS: u64 = 5;
    const STRESS_CAPACITY: usize = 16;
    const STRESS_ENQUEUE_TIMEOUT_S: u64 = 2;
    const STRESS_EXECUTION_TIMEOUT_S: u64 = 1;
    const STRESS_REPLY_TIMEOUT_S: u64 = 5;
    const STRESS_TOTAL_JOBS: usize = 200;
    const TIMEOUT_PROPAGATION_SLEEP_MS: u64 = 50;

    const fn default_enqueue_timeout() -> Duration {
        Duration::from_millis(DEFAULT_ENQUEUE_TIMEOUT_MS)
    }

    const fn default_execution_timeout() -> Duration {
        Duration::from_millis(DEFAULT_EXECUTION_TIMEOUT_MS)
    }

    const fn default_reply_timeout() -> Duration {
        Duration::from_secs(DEFAULT_REPLY_TIMEOUT_S)
    }

    const fn shutdown_wait_timeout() -> Duration {
        Duration::from_millis(SHUTDOWN_WAIT_TIMEOUT_MS)
    }

    const fn short_execution_timeout() -> Duration {
        Duration::from_millis(SHORT_EXECUTION_TIMEOUT_MS)
    }

    const fn long_job_sleep() -> Duration {
        Duration::from_millis(LONG_JOB_SLEEP_MS)
    }

    const fn post_timeout_wait() -> Duration {
        Duration::from_millis(POST_TIMEOUT_WAIT_MS)
    }

    const fn reply_test_enqueue_timeout() -> Duration {
        Duration::from_millis(REPLY_TEST_ENQUEUE_TIMEOUT_MS)
    }

    const fn reply_test_execution_timeout() -> Duration {
        Duration::from_secs(REPLY_TEST_EXECUTION_TIMEOUT_S)
    }

    const fn reply_test_reply_timeout() -> Duration {
        Duration::from_millis(REPLY_TEST_REPLY_TIMEOUT_MS)
    }

    const fn reply_test_long_job_sleep() -> Duration {
        Duration::from_millis(REPLY_TEST_LONG_JOB_MS)
    }

    const fn reply_test_stagger() -> Duration {
        Duration::from_millis(REPLY_TEST_STAGGER_MS)
    }

    const fn stress_enqueue_timeout() -> Duration {
        Duration::from_secs(STRESS_ENQUEUE_TIMEOUT_S)
    }

    const fn stress_execution_timeout() -> Duration {
        Duration::from_secs(STRESS_EXECUTION_TIMEOUT_S)
    }

    const fn stress_reply_timeout() -> Duration {
        Duration::from_secs(STRESS_REPLY_TIMEOUT_S)
    }

    const fn timeout_propagation_sleep() -> Duration {
        Duration::from_millis(TIMEOUT_PROPAGATION_SLEEP_MS)
    }

    fn test_config() -> QueueConfig {
        QueueConfig {
            capacity: DEFAULT_CAPACITY,
            enqueue_timeout: default_enqueue_timeout(),
            execution_timeout: default_execution_timeout(),
            reply_timeout: default_reply_timeout(),
        }
    }

    fn new_test_queue<T>() -> (RequestQueue<T>, QueueShutdown)
    where
        T: Send + 'static,
    {
        RequestQueue::<T>::new(test_config()).expect("queue construction should succeed for valid test config")
    }

    async fn assert_shutdown(shutdown: QueueShutdown) {
        let wait_result = tokio::time::timeout(shutdown_wait_timeout(), shutdown.wait()).await;
        assert!(wait_result.is_ok(), "worker should shut down before timeout, got: {wait_result:?}");
        let shutdown_result = wait_result.expect("timeout branch already asserted as impossible");
        assert_eq!(shutdown_result, Ok(()), "shutdown waiter should complete cleanly");
    }

    fn panic_before_future() -> impl Future<Output = QueueResult<u32>> + Send {
        let should_panic = true;
        assert!(!should_panic, "boom from queued job");
        async { Ok(0) }
    }

    #[tokio::test]
    async fn basic_success() {
        let (queue, shutdown) = new_test_queue::<u32>();
        let response = queue.enqueue(|| async { Ok(7) }).await;
        assert_eq!(response, Ok(7), "queued job should return expected value");

        drop(queue);
        assert_shutdown(shutdown).await;
    }

    #[tokio::test]
    async fn queue_full_backpressure_behavior() {
        let (queue, shutdown) = new_test_queue::<u32>();

        let (first_release_tx, first_release_rx) = oneshot::channel::<()>();
        let (second_release_tx, second_release_rx) = oneshot::channel::<()>();

        let first_queue = queue.clone();
        let first_job = tokio::spawn(async move {
            first_queue
                .enqueue(move || async move {
                    first_release_rx.await.expect("first release channel should be signalled");
                    Ok(1)
                })
                .await
        });

        let second_queue = queue.clone();
        let second_job = tokio::spawn(async move {
            second_queue
                .enqueue(move || async move {
                    second_release_rx.await.expect("second release channel should be signalled");
                    Ok(2)
                })
                .await
        });

        let fill_wait_result = tokio::time::timeout(shutdown_wait_timeout(), async {
            while queue.tx.capacity() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await;

        assert!(fill_wait_result.is_ok(), "queue did not reach full capacity before timeout");

        let timeout_result = queue.enqueue(|| async { Ok(3) }).await;

        assert!(
            matches!(
                timeout_result,
                Err(RequestQueueError::EnqueueTimedOut { timeout }) if timeout == default_enqueue_timeout()
            ),
            "expected EnqueueTimedOut, got: {timeout_result:?}"
        );

        assert!(first_release_tx.send(()).is_ok(), "first release receiver dropped unexpectedly");
        assert!(second_release_tx.send(()).is_ok(), "second release receiver dropped unexpectedly");

        let second_result = second_job.await.expect("second job task should not panic");
        assert_eq!(second_result, Ok(2), "second queued request should succeed");

        let first_result = first_job.await.expect("first job task should not panic");
        assert_eq!(first_result, Ok(1), "first queued request should succeed");

        drop(queue);
        assert_shutdown(shutdown).await;
    }

    #[tokio::test]
    async fn panic_is_reported_and_worker_recovers() {
        let (queue, shutdown) = new_test_queue::<u32>();

        let panic_result = queue.enqueue(panic_before_future).await;
        assert!(
            matches!(panic_result, Err(RequestQueueError::JobPanicked)),
            "expected JobPanicked, got: {panic_result:?}"
        );

        let follow_up_result = queue.enqueue(|| async { Ok(9) }).await;
        assert_eq!(follow_up_result, Ok(9), "queue should recover and handle a follow-up request");

        drop(queue);
        assert_shutdown(shutdown).await;
    }

    #[tokio::test]
    async fn timed_out_job_is_cancelled_and_queue_continues() {
        let (queue, shutdown) = new_test_queue::<u32>();

        let did_run_after_sleep = Arc::new(AtomicBool::new(false));
        let did_run_after_sleep_clone = Arc::clone(&did_run_after_sleep);

        let timeout_result = queue
            .enqueue_with_timeout(short_execution_timeout(), move || async move {
                tokio::time::sleep(long_job_sleep()).await;
                did_run_after_sleep_clone.store(true, Ordering::SeqCst);
                Ok(1)
            })
            .await;

        assert!(
            matches!(
                timeout_result,
                Err(RequestQueueError::ExecutionTimedOut { timeout }) if timeout == short_execution_timeout()
            ),
            "expected ExecutionTimedOut, got: {timeout_result:?}"
        );

        tokio::time::sleep(post_timeout_wait()).await;
        assert!(
            !did_run_after_sleep.load(Ordering::SeqCst),
            "timed out future continued running after timeout"
        );

        let follow_up_result = queue.enqueue(|| async { Ok(2) }).await;
        assert_eq!(follow_up_result, Ok(2), "queue should process follow-up request after timeout");

        drop(queue);
        assert_shutdown(shutdown).await;
    }

    #[tokio::test]
    async fn reply_timeout_is_enforced_for_slow_queue_drain() {
        let config = QueueConfig {
            capacity: DEFAULT_CAPACITY,
            enqueue_timeout: reply_test_enqueue_timeout(),
            execution_timeout: reply_test_execution_timeout(),
            reply_timeout: reply_test_reply_timeout(),
        };

        let (queue, shutdown) = RequestQueue::<u32>::new(config).expect("queue construction should succeed for reply-timeout config");

        let first_queue = queue.clone();
        let first_job = tokio::spawn(async move {
            first_queue
                .enqueue(|| async {
                    tokio::time::sleep(reply_test_long_job_sleep()).await;
                    Ok(1)
                })
                .await
        });

        tokio::time::sleep(reply_test_stagger()).await;

        let second_result = queue.enqueue(|| async { Ok(2) }).await;
        assert!(
            matches!(
                second_result,
                Err(RequestQueueError::ReplyTimedOut { timeout }) if timeout == reply_test_reply_timeout()
            ),
            "expected second request reply timeout, got: {second_result:?}"
        );

        let first_result = first_job.await.expect("first job task should not panic");
        assert!(
            matches!(
                first_result,
                Err(RequestQueueError::ReplyTimedOut { timeout }) if timeout == reply_test_reply_timeout()
            ),
            "expected first request reply timeout, got: {first_result:?}"
        );

        drop(queue);
        assert_shutdown(shutdown).await;
    }

    #[tokio::test]
    async fn stress_concurrent_producers() {
        let config = QueueConfig {
            capacity: STRESS_CAPACITY,
            enqueue_timeout: stress_enqueue_timeout(),
            execution_timeout: stress_execution_timeout(),
            reply_timeout: stress_reply_timeout(),
        };

        let (queue, shutdown) = RequestQueue::<usize>::new(config).expect("queue construction should succeed for stress config");

        let mut handles = Vec::new();
        let total_jobs: usize = STRESS_TOTAL_JOBS;
        for value in 0..total_jobs {
            let producer = queue.clone();
            handles.push(tokio::spawn(
                async move { producer.enqueue(move || async move { Ok(value) }).await },
            ));
        }

        let mut results = Vec::with_capacity(total_jobs);
        for handle in handles {
            let queue_result = handle.await.expect("producer task should not panic");
            assert!(queue_result.is_ok(), "concurrent enqueue should succeed, got: {queue_result:?}");
            results.push(queue_result.expect("value presence already asserted via is_ok"));
        }

        results.sort_unstable();
        let expected: Vec<usize> = (0..total_jobs).collect();
        assert_eq!(results, expected);

        drop(queue);
        assert_shutdown(shutdown).await;
    }

    #[tokio::test]
    async fn worker_shutdown() {
        let (queue, shutdown) = new_test_queue::<u32>();

        let clone = queue.clone();
        drop(queue);
        drop(clone);

        assert_shutdown(shutdown).await;
    }

    #[tokio::test]
    async fn timeout_propagation() {
        let (queue, shutdown) = new_test_queue::<u32>();

        let response: QueueResult<u32> = queue
            .enqueue_with_timeout(short_execution_timeout(), || async {
                tokio::time::sleep(timeout_propagation_sleep()).await;
                Ok(42)
            })
            .await;

        assert!(
            matches!(
                response,
                Err(RequestQueueError::ExecutionTimedOut { timeout }) if timeout == short_execution_timeout()
            ),
            "expected ExecutionTimedOut, got: {response:?}"
        );

        drop(queue);
        assert_shutdown(shutdown).await;
    }
}
