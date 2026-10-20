//! Durable background workers. Jobs are claimed from PostgreSQL with leases;
//! concurrency is bounded by a semaphore; failures retry with exponential
//! backoff and dead-letter after `max_attempts`.

pub mod extract;
pub mod handlers;

use antibody_core::model::JobKind;
use antibody_db::PgPool;
use antibody_db::jobs::{self, ClaimedJob, FailOutcome, NewJob};
use antibody_providers::Providers;
use antibody_search::SearchEngine;
use chrono::{Duration as ChronoDuration, Utc};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub worker_id: String,
    pub concurrency: usize,
    pub poll_interval: Duration,
    pub lease: Duration,
    pub retry_base: Duration,
    pub retry_cap: Duration,
    pub maintenance_interval: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            worker_id: format!("worker-{}", uuid::Uuid::new_v4().simple()),
            concurrency: 4,
            poll_interval: Duration::from_millis(500),
            lease: Duration::from_secs(120),
            retry_base: Duration::from_secs(5),
            retry_cap: Duration::from_secs(30 * 60),
            maintenance_interval: Duration::from_secs(60),
        }
    }
}

#[derive(Clone)]
pub struct WorkerContext {
    pub pool: PgPool,
    pub providers: Providers,
    pub search: SearchEngine,
}

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("retryable: {0}")]
    Retry(String),
    #[error("permanent: {0}")]
    Permanent(String),
}

impl From<antibody_db::DbError> for JobError {
    fn from(e: antibody_db::DbError) -> Self {
        match e {
            antibody_db::DbError::Domain(d) => JobError::Permanent(d.to_string()),
            other => JobError::Retry(other.to_string()),
        }
    }
}

impl From<sqlx::Error> for JobError {
    fn from(e: sqlx::Error) -> Self {
        JobError::Retry(e.to_string())
    }
}

impl From<anyhow::Error> for JobError {
    fn from(e: anyhow::Error) -> Self {
        JobError::Retry(e.to_string())
    }
}

struct HeartbeatGuard(tokio::task::JoinHandle<()>);

impl Drop for HeartbeatGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn with_heartbeat<T>(
    heartbeat: impl Future<Output = ()> + Send + 'static,
    handler: impl Future<Output = T>,
) -> T {
    // Dropping a plain JoinHandle detaches its task. Abort the heartbeat on
    // every handler exit, including panic unwinding and cancellation.
    let _heartbeat = HeartbeatGuard(tokio::spawn(heartbeat));
    handler.await
}

/// Run one job to completion, recording success or failure. Exposed for
/// tests that need deterministic, single-job execution.
pub async fn run_job(ctx: &WorkerContext, cfg: &WorkerConfig, job: ClaimedJob) {
    // Keep the lease alive while the handler runs.
    let hb_pool = ctx.pool.clone();
    let hb_id = job.id;
    let hb_worker = cfg.worker_id.clone();
    let lease = ChronoDuration::from_std(cfg.lease).unwrap_or(ChronoDuration::seconds(120));
    let hb_interval = cfg.lease / 3;
    let heartbeat = async move {
        loop {
            tokio::time::sleep(hb_interval).await;
            match jobs::heartbeat(&hb_pool, hb_id, &hb_worker, lease).await {
                Ok(true) => {}
                _ => break,
            }
        }
    };

    let started = std::time::Instant::now();
    let span = tracing::info_span!("job", id = %job.id, kind = %job.kind, attempt = job.attempts);
    let result = with_heartbeat(heartbeat, handlers::dispatch(ctx, &job).instrument(span)).await;

    match result {
        Ok(()) => {
            if let Err(e) = jobs::complete(&ctx.pool, job.id, &cfg.worker_id).await {
                tracing::error!(job = %job.id, error = %e, "failed to mark job complete");
            }
            tracing::info!(job = %job.id, kind = %job.kind, ms = started.elapsed().as_millis() as u64, "job succeeded");
        }
        Err(err) => {
            let (permanent, message) = match &err {
                JobError::Permanent(m) => (true, m.clone()),
                JobError::Retry(m) => (false, m.clone()),
            };
            let delay = jobs::backoff(
                job.attempts,
                ChronoDuration::from_std(cfg.retry_base).unwrap_or(ChronoDuration::seconds(5)),
                ChronoDuration::from_std(cfg.retry_cap).unwrap_or(ChronoDuration::minutes(30)),
            );
            match jobs::fail(
                &ctx.pool,
                job.id,
                &cfg.worker_id,
                &message,
                permanent,
                delay,
            )
            .await
            {
                Ok(FailOutcome::Dead) => {
                    tracing::error!(job = %job.id, kind = %job.kind, error = %message, "job dead-lettered");
                    if job.kind == JobKind::ExtractSession.as_str()
                        && let Some(sid) = job.session_id
                    {
                        let _ = antibody_db::sessions::mark_extraction_failed(
                            &ctx.pool, sid, &message, true,
                        )
                        .await;
                    }
                }
                Ok(FailOutcome::Retrying) => {
                    tracing::warn!(job = %job.id, kind = %job.kind, error = %message, retry_in_ms = delay.num_milliseconds(), "job failed; retrying");
                }
                Ok(FailOutcome::LeaseLost) => {
                    tracing::warn!(job = %job.id, "lease lost before failure was recorded")
                }
                Err(e) => {
                    tracing::error!(job = %job.id, error = %e, "failed to record job failure")
                }
            }
        }
    }
}

/// Claim and run every job that is currently due, sequentially. Used by
/// tests and by `antibody-worker --once`.
pub async fn drain(ctx: &WorkerContext, cfg: &WorkerConfig) -> anyhow::Result<usize> {
    let lease = ChronoDuration::from_std(cfg.lease)?;
    let mut n = 0;
    loop {
        let batch = jobs::claim(&ctx.pool, &cfg.worker_id, 16, lease).await?;
        if batch.is_empty() {
            return Ok(n);
        }
        for job in batch {
            run_job(ctx, cfg, job).await;
            n += 1;
        }
    }
}

async fn maintenance(ctx: &WorkerContext) {
    match jobs::reap_expired(&ctx.pool).await {
        Ok(n) if n > 0 => {
            tracing::warn!(reaped = n, "dead-lettered jobs with expired final leases")
        }
        Ok(_) => {}
        Err(e) => tracing::error!(error = %e, "lease reaping failed"),
    }
    let hour = Utc::now().format("%Y-%m-%dT%H").to_string();
    if let Err(e) = jobs::enqueue(
        &ctx.pool,
        NewJob::new(
            JobKind::RetentionSweep,
            format!("retention:{hour}"),
            serde_json::json!({}),
        ),
    )
    .await
    {
        tracing::error!(error = %e, "failed to enqueue retention sweep");
    }
}

/// Main loop: poll, claim up to free capacity, run concurrently.
pub async fn run(
    ctx: WorkerContext,
    cfg: WorkerConfig,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let semaphore = Arc::new(Semaphore::new(cfg.concurrency));
    let lease = ChronoDuration::from_std(cfg.lease)?;
    let mut last_maintenance = std::time::Instant::now() - cfg.maintenance_interval;
    match handlers::backfill_embeddings(&ctx).await {
        Ok(n) if n > 0 => tracing::info!(enqueued = n, "embedding backfill enqueued"),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "embedding backfill failed"),
    }
    tracing::info!(worker = %cfg.worker_id, concurrency = cfg.concurrency, "worker started");

    while !shutdown.is_cancelled() {
        if last_maintenance.elapsed() >= cfg.maintenance_interval {
            maintenance(&ctx).await;
            last_maintenance = std::time::Instant::now();
        }
        let free = semaphore.available_permits();
        let mut claimed = 0;
        if free > 0 {
            match jobs::claim(&ctx.pool, &cfg.worker_id, free as i64, lease).await {
                Ok(batch) => {
                    claimed = batch.len();
                    for job in batch {
                        let permit = semaphore.clone().acquire_owned().await?;
                        let ctx = ctx.clone();
                        let cfg = cfg.clone();
                        tokio::spawn(async move {
                            run_job(&ctx, &cfg, job).await;
                            drop(permit);
                        });
                    }
                }
                Err(e) => tracing::error!(error = %e, "claim failed"),
            }
        }
        if claimed == 0 {
            tokio::select! {
                _ = tokio::time::sleep(cfg.poll_interval) => {}
                _ = shutdown.cancelled() => {}
            }
        }
    }

    tracing::info!("worker draining in-flight jobs");
    let _ = tokio::time::timeout(
        Duration::from_secs(30),
        semaphore.acquire_many(cfg.concurrency as u32),
    )
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::with_heartbeat;
    use std::future::{Future, pending};
    use std::time::Duration;
    use tokio::sync::oneshot;

    fn pending_heartbeat() -> (
        impl Future<Output = ()>,
        oneshot::Receiver<()>,
        oneshot::Receiver<()>,
    ) {
        let (started_tx, started_rx) = oneshot::channel();
        let (stopped_tx, stopped_rx) = oneshot::channel::<()>();
        let heartbeat = async move {
            // The receiver closes only when this running heartbeat is dropped.
            let _stopped = stopped_tx;
            started_tx.send(()).unwrap();
            pending::<()>().await;
        };
        (heartbeat, started_rx, stopped_rx)
    }

    async fn assert_heartbeat_stopped(stopped: oneshot::Receiver<()>) {
        tokio::time::timeout(Duration::from_secs(1), stopped)
            .await
            .expect("heartbeat must stop with its handler")
            .expect_err("heartbeat signals termination by dropping its sender");
    }

    #[tokio::test]
    async fn heartbeat_stops_when_handler_returns() {
        let (heartbeat, started, stopped) = pending_heartbeat();
        let result = with_heartbeat(heartbeat, async move {
            started.await.unwrap();
            42
        })
        .await;

        assert_eq!(result, 42);
        assert_heartbeat_stopped(stopped).await;
    }

    #[tokio::test]
    async fn heartbeat_stops_when_handler_panics() {
        let (heartbeat, started, stopped) = pending_heartbeat();
        let job = tokio::spawn(with_heartbeat(heartbeat, async move {
            started.await.unwrap();
            panic!("test handler panic");
        }));

        assert!(job.await.unwrap_err().is_panic());
        assert_heartbeat_stopped(stopped).await;
    }

    #[tokio::test]
    async fn heartbeat_stops_when_handler_is_cancelled() {
        let (heartbeat, started, stopped) = pending_heartbeat();
        let job = tokio::spawn(with_heartbeat(heartbeat, pending::<()>()));
        started.await.unwrap();

        job.abort();

        assert!(job.await.unwrap_err().is_cancelled());
        assert_heartbeat_stopped(stopped).await;
    }
}
