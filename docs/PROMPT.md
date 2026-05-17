Lets build this thing, and use the justfile to run it and the checks until all of them pass. Yes: the clean way is to ask Claude/Codex to generate an actor-style queue built on `tokio::sync::mpsc` for inbound work and `tokio::sync::oneshot` for per-request replies. Tokio’s own guidance recommends a dedicated manager task that owns the shared client/resource, receives commands over a bounded `mpsc`, and sends each result back through a `oneshot`, which gives you serialization, backpressure, and async request/response semantics without locking the resource across `.await` points. [docs](https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html)

## Architecture

Use a **single consumer** queue task when you want one shared async resource, such as an HTTP client with rate limits, a model gateway, or a DB/session handle, because `mpsc` is multi-producer/single-consumer by design. Each submitted job should contain the payload plus a `oneshot::Sender<Result<T, E>>`, so callers can `await` their own response independently while the queue task processes jobs in order. [docs](https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html)

A minimal pattern looks like this:

```rust
use tokio::sync::{mpsc, oneshot};
use std::future::Future;
use std::pin::Pin;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
type JobResult<T, E> = Result<T, E>;

struct Job<T, E> {
    task: Box<dyn FnOnce() -> BoxFuture<JobResult<T, E>> + Send>,
    reply_tx: oneshot::Sender<JobResult<T, E>>,
}

#[derive(Clone)]
struct RequestQueue<T, E> {
    tx: mpsc::Sender<Job<T, E>>,
}

impl<T, E> RequestQueue<T, E>
where
    T: Send + 'static,
    E: Send + 'static,
{
    fn new(capacity: usize) -> Self {
        let (tx, mut rx) = mpsc::channel::<Job<T, E>>(capacity);

        tokio::spawn(async move {
            while let Some(job) = rx.recv().await {
                let fut = (job.task)();
                let result = fut.await;
                let _ = job.reply_tx.send(result);
            }
        });

        Self { tx }
    }

    async fn enqueue<F, Fut>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
        E: From<&'static str>,
    {
        let (reply_tx, reply_rx) = oneshot::channel();

        let job = Job {
            task: Box::new(move || Box::pin(f())),
            reply_tx,
        };

        self.tx
            .send(job)
            .await
            .map_err(|_| E::from("queue closed"))?;

        reply_rx.await.map_err(|_| E::from("worker dropped"))?
    }
}
```

This mirrors Tokio’s documented message-passing pattern: producers clone the `Sender`, the manager owns the resource, and responses flow back via `oneshot` channels. [docs](https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html)

## Production tweaks

Use a **bounded** channel, not an unbounded one, because Tokio explicitly warns that bounded queues are the mechanism for backpressure and that unbounded queuing can lead to memory blowups under load. Pick a capacity based on latency budget and upstream concurrency, and decide what happens when the queue is full: wait, reject, or time out. [docs](https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html)

If you need parallelism greater than 1, keep the submission queue but fan work out inside the manager with a semaphore or a small worker pool rather than abandoning the queue abstraction entirely. If you need strict ordering, keep one worker; if you need throughput, allow `N` in-flight jobs and preserve correlation with the `oneshot` reply channel per request. [docs](https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html)

## Single prompt

For a one-shot Claude/Codex prompt, be very explicit about architecture, output format, and constraints, because Anthropic recommends clear, direct instructions, explicit steps, and concrete output requirements for coding tasks. Their guidance also says that well-specified upfront prompts work better than progressively clarifying across many turns for autonomous coding workflows. [tokio](https://tokio.rs/tokio/tutorial/channels)

Use something close to this:

```text
You are implementing a production-grade asynchronous request queue in Rust.

Build a small library crate using Tokio with these requirements:

<requirements>
- Use tokio::sync::mpsc for the inbound queue.
- Use tokio::sync::oneshot for per-request responses.
- The queue must be bounded and enforce backpressure.
- Expose a cloneable RequestQueue handle for producers.
- Spawn a dedicated worker/manager task that owns the resource and processes jobs.
- Provide enqueue() that returns the job result asynchronously.
- Do not hold a mutex guard across .await.
- Include graceful shutdown behavior when all senders are dropped.
- Add timeout support for enqueue and job execution.
- Add unit tests with #[tokio::test] for:
  - basic success
  - queue full/backpressure behavior
  - worker shutdown
  - timeout propagation
- Return complete Cargo.toml and src/lib.rs content.
</requirements>

<output_format>
- First: short architecture summary
- Then: Cargo.toml
- Then: src/lib.rs
- Then: tests explanation
</output_format>

<quality_bar>
- Idiomatic Rust
- No placeholders
- Minimal dependencies
- Comments only where they clarify non-obvious concurrency decisions
</quality_bar>
```

That structure follows Claude’s best-practice advice: explicit instructions, XML-style sections, and a precise definition of success criteria and format. [tokio](https://tokio.rs/tokio/tutorial/channels)

## Practical notes

If your real use case is “many callers, one shared AI client,” the manager task should own the AI client instance and the queued command enum should carry request parameters plus a reply channel, which is exactly the pattern Tokio demonstrates for a shared Redis client. If instead you just need “limit concurrency to 8 requests,” a semaphore may be simpler than a full queue; the queue pattern is best when you need centralized ownership, ordering, buffering, or per-job coordination. [docs](https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html)

For Codex specifically, non-interactive execution is supported through `codex exec PROMPT`, which fits your “single AI prompt” workflow well. Would you like a stricter version of the prompt that targets your exact use case, such as HTTP requests, LLM inference jobs, or database work ? [github](https://github.com/openai/codex/blob/main/codex-rs/README.md)
