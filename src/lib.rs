//! Actor-style asynchronous request queue based on Tokio channels.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

/// Output type returned by queued jobs.
pub type QueueResult<T> = Result<T, RequestQueueError>;

type BoxJobFuture<T> = Pin<Box<dyn Future<Output = QueueResult<T>> + Send + 'static>>;
type BoxJobFactory<T> = Box<dyn FnOnce() -> BoxJobFuture<T> + Send + 'static>;

struct Job<T> {
    task: BoxJobFactory<T>,
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
                    eprintln!("request was cancelled before worker started processing");
                    continue;
                }

                let mut handle = tokio::spawn(async move { (job.task)().await });

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

                if job.reply_tx.send(result).is_err() {
                    eprintln!("request receiver dropped before worker reply was delivered");
                }
            }

            if done_tx.send(()).is_err() {
                eprintln!("shutdown waiter dropped before worker exit signal");
            }
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
            task: Box::new(move || Box::pin(f())),
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

    fn test_config() -> QueueConfig {
        QueueConfig {
            capacity: 1,
            enqueue_timeout: Duration::from_millis(60),
            execution_timeout: Duration::from_millis(200),
            reply_timeout: Duration::from_secs(2),
        }
    }

    async fn assert_shutdown(shutdown: QueueShutdown) {
        let wait_result = tokio::time::timeout(Duration::from_millis(200), shutdown.wait()).await;

        match wait_result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                panic!("shutdown waiter returned an unexpected error: {error}");
            }
            Err(elapsed) => {
                panic!("worker did not shut down within 200ms: {elapsed}");
            }
        }
    }

    #[tokio::test]
    async fn basic_success() {
        let queue_result = RequestQueue::<u32>::new(test_config());
        let (queue, shutdown) = match queue_result {
            Ok(ok) => ok,
            Err(error) => panic!("queue construction failed unexpectedly: {error}"),
        };

        let response = queue.enqueue(|| async { Ok(7) }).await;

        match response {
            Ok(value) => {
                assert_eq!(value, 7);
            }
            Err(error) => panic!("job failed unexpectedly: {error}"),
        }

        drop(queue);
        assert_shutdown(shutdown).await;
    }

    #[tokio::test]
    async fn queue_full_backpressure_behavior() {
        let queue_result = RequestQueue::<u32>::new(test_config());
        let (queue, shutdown) = match queue_result {
            Ok(ok) => ok,
            Err(error) => panic!("queue construction failed unexpectedly: {error}"),
        };

        let (first_release_tx, first_release_rx) = oneshot::channel::<()>();
        let (second_release_tx, second_release_rx) = oneshot::channel::<()>();

        let first_queue = queue.clone();
        let first_job = tokio::spawn(async move {
            first_queue
                .enqueue(move || async move {
                    match first_release_rx.await {
                        Ok(()) => {}
                        Err(error) => panic!("first release channel closed unexpectedly: {error}"),
                    }
                    Ok(1)
                })
                .await
        });

        let second_queue = queue.clone();
        let second_job = tokio::spawn(async move {
            second_queue
                .enqueue(move || async move {
                    match second_release_rx.await {
                        Ok(()) => {}
                        Err(error) => panic!("second release channel closed unexpectedly: {error}"),
                    }
                    Ok(2)
                })
                .await
        });

        let fill_wait_result = tokio::time::timeout(Duration::from_millis(200), async {
            while queue.tx.capacity() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await;

        assert!(fill_wait_result.is_ok(), "queue did not reach full capacity before timeout");

        let timeout_result = queue.enqueue(|| async { Ok(3) }).await;

        match timeout_result {
            Err(RequestQueueError::EnqueueTimedOut { timeout }) => {
                assert_eq!(timeout, Duration::from_millis(60));
            }
            Ok(value) => panic!("expected enqueue timeout, but request succeeded with value: {value}"),
            Err(error) => panic!("expected enqueue timeout, got different error: {error}"),
        }

        assert!(first_release_tx.send(()).is_ok(), "first release receiver dropped unexpectedly");
        assert!(second_release_tx.send(()).is_ok(), "second release receiver dropped unexpectedly");

        let second_result = match second_job.await {
            Ok(result) => result,
            Err(join_error) => panic!("second job task join failed unexpectedly: {join_error}"),
        };

        match second_result {
            Ok(value) => assert_eq!(value, 2),
            Err(error) => panic!("expected second request to queue successfully, got: {error}"),
        }

        let first_result = match first_job.await {
            Ok(result) => result,
            Err(join_error) => panic!("first job task join failed unexpectedly: {join_error}"),
        };

        match first_result {
            Ok(value) => {
                assert_eq!(value, 1);
            }
            Err(error) => panic!("first request failed unexpectedly: {error}"),
        }

        drop(queue);
        assert_shutdown(shutdown).await;
    }

    #[tokio::test]
    async fn panic_is_reported_and_worker_recovers() {
        let queue_result = RequestQueue::<u32>::new(test_config());
        let (queue, shutdown) = match queue_result {
            Ok(ok) => ok,
            Err(error) => panic!("queue construction failed unexpectedly: {error}"),
        };

        let panic_result = queue.enqueue(|| async { panic!("boom from queued job") }).await;

        match panic_result {
            Err(RequestQueueError::JobPanicked) => {}
            Ok(value) => panic!("expected panic error, got success value: {value}"),
            Err(error) => panic!("expected panic error, got different error: {error}"),
        }

        let follow_up_result = queue.enqueue(|| async { Ok(9) }).await;
        match follow_up_result {
            Ok(value) => assert_eq!(value, 9),
            Err(error) => panic!("queue did not recover after panic: {error}"),
        }

        drop(queue);
        assert_shutdown(shutdown).await;
    }

    #[tokio::test]
    async fn timed_out_job_is_cancelled_and_queue_continues() {
        let queue_result = RequestQueue::<u32>::new(test_config());
        let (queue, shutdown) = match queue_result {
            Ok(ok) => ok,
            Err(error) => panic!("queue construction failed unexpectedly: {error}"),
        };

        let did_run_after_sleep = Arc::new(AtomicBool::new(false));
        let did_run_after_sleep_clone = Arc::clone(&did_run_after_sleep);

        let timeout_result = queue
            .enqueue_with_timeout(Duration::from_millis(10), move || async move {
                tokio::time::sleep(Duration::from_millis(60)).await;
                did_run_after_sleep_clone.store(true, Ordering::SeqCst);
                Ok(1)
            })
            .await;

        match timeout_result {
            Err(RequestQueueError::ExecutionTimedOut { timeout }) => {
                assert_eq!(timeout, Duration::from_millis(10));
            }
            Ok(value) => panic!("expected timeout error, got success value: {value}"),
            Err(error) => panic!("expected timeout error, got different error: {error}"),
        }

        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(
            !did_run_after_sleep.load(Ordering::SeqCst),
            "timed out future continued running after timeout"
        );

        let follow_up_result = queue.enqueue(|| async { Ok(2) }).await;
        match follow_up_result {
            Ok(value) => assert_eq!(value, 2),
            Err(error) => panic!("queue did not process follow-up request after timeout: {error}"),
        }

        drop(queue);
        assert_shutdown(shutdown).await;
    }

    #[tokio::test]
    async fn reply_timeout_is_enforced_for_slow_queue_drain() {
        let config = QueueConfig {
            capacity: 1,
            enqueue_timeout: Duration::from_millis(100),
            execution_timeout: Duration::from_secs(1),
            reply_timeout: Duration::from_millis(40),
        };

        let queue_result = RequestQueue::<u32>::new(config);
        let (queue, shutdown) = match queue_result {
            Ok(ok) => ok,
            Err(error) => panic!("queue construction failed unexpectedly: {error}"),
        };

        let first_queue = queue.clone();
        let first_job = tokio::spawn(async move {
            first_queue
                .enqueue(|| async {
                    tokio::time::sleep(Duration::from_millis(120)).await;
                    Ok(1)
                })
                .await
        });

        tokio::time::sleep(Duration::from_millis(5)).await;

        let second_result = queue.enqueue(|| async { Ok(2) }).await;
        match second_result {
            Err(RequestQueueError::ReplyTimedOut { timeout }) => {
                assert_eq!(timeout, Duration::from_millis(40));
            }
            Ok(value) => panic!("expected reply timeout, got success value: {value}"),
            Err(error) => panic!("expected reply timeout, got different error: {error}"),
        }

        let first_result = match first_job.await {
            Ok(result) => result,
            Err(join_error) => panic!("first job task join failed unexpectedly: {join_error}"),
        };
        match first_result {
            Err(RequestQueueError::ReplyTimedOut { timeout }) => {
                assert_eq!(timeout, Duration::from_millis(40));
            }
            Ok(value) => panic!("expected first request to hit reply timeout, got: {value}"),
            Err(error) => {
                panic!("expected first request reply timeout, got different error: {error}")
            }
        }

        drop(queue);
        assert_shutdown(shutdown).await;
    }

    #[tokio::test]
    async fn stress_concurrent_producers() {
        let config = QueueConfig {
            capacity: 16,
            enqueue_timeout: Duration::from_secs(2),
            execution_timeout: Duration::from_secs(1),
            reply_timeout: Duration::from_secs(5),
        };

        let queue_result = RequestQueue::<usize>::new(config);
        let (queue, shutdown) = match queue_result {
            Ok(ok) => ok,
            Err(error) => panic!("queue construction failed unexpectedly: {error}"),
        };

        let mut handles = Vec::new();
        let total_jobs: usize = 200;
        for value in 0..total_jobs {
            let producer = queue.clone();
            handles.push(tokio::spawn(
                async move { producer.enqueue(move || async move { Ok(value) }).await },
            ));
        }

        let mut results = Vec::with_capacity(total_jobs);
        for handle in handles {
            let join_result = handle.await;
            let queue_result = match join_result {
                Ok(result) => result,
                Err(join_error) => panic!("producer task join failed unexpectedly: {join_error}"),
            };

            match queue_result {
                Ok(value) => results.push(value),
                Err(error) => panic!("concurrent enqueue failed unexpectedly: {error}"),
            }
        }

        results.sort_unstable();
        let expected: Vec<usize> = (0..total_jobs).collect();
        assert_eq!(results, expected);

        drop(queue);
        assert_shutdown(shutdown).await;
    }

    #[tokio::test]
    async fn worker_shutdown() {
        let queue_result = RequestQueue::<u32>::new(test_config());
        let (queue, shutdown) = match queue_result {
            Ok(ok) => ok,
            Err(error) => panic!("queue construction failed unexpectedly: {error}"),
        };

        let clone = queue.clone();
        drop(queue);
        drop(clone);

        assert_shutdown(shutdown).await;
    }

    #[tokio::test]
    async fn timeout_propagation() {
        let queue_result = RequestQueue::<u32>::new(test_config());
        let (queue, shutdown) = match queue_result {
            Ok(ok) => ok,
            Err(error) => panic!("queue construction failed unexpectedly: {error}"),
        };

        let response: QueueResult<u32> = queue
            .enqueue_with_timeout(Duration::from_millis(10), || async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(42)
            })
            .await;

        match response {
            Err(RequestQueueError::ExecutionTimedOut { timeout }) => {
                assert_eq!(timeout, Duration::from_millis(10));
            }
            Ok(value) => panic!("expected execution timeout, but got success: {value}"),
            Err(error) => panic!("expected execution timeout, got different error: {error}"),
        }

        drop(queue);
        assert_shutdown(shutdown).await;
    }
}
