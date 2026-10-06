use crate::{
    execution::{ExecutionControl, ExecutionEnvironment, ExecutionRequest},
    model::{WorkerCapabilities, WorkerState},
    persistence::RunStore,
    report::{self, RunSummary},
    scheduler::Scheduler,
};
use std::{
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct LocalWorker {
    pub worker_id: String,
    pub worker_instance_id: String,
    pub capabilities: WorkerCapabilities,
    pub environment: Arc<dyn ExecutionEnvironment>,
}

/// A container worker uses the same environment-neutral worker envelope.
/// The alias makes that invariant explicit without adding scheduler semantics.
pub type ContainerWorker = LocalWorker;

#[derive(Clone, Debug)]
pub struct WorkerPoolConfig {
    pub heartbeat_interval_ms: u64,
    pub idle_poll_ms: u64,
    pub no_progress_timeout_ms: u64,
}

impl Default for WorkerPoolConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval_ms: 250,
            idle_poll_ms: 25,
            no_progress_timeout_ms: 30_000,
        }
    }
}

pub struct WorkerPool {
    run_id: String,
    scheduler: Arc<Mutex<Scheduler>>,
    store: RunStore,
    workers: Vec<LocalWorker>,
    controls: Arc<Mutex<BTreeMap<String, ExecutionControl>>>,
    cancel_poll_interval_ms: u64,
    config: WorkerPoolConfig,
}

#[derive(Clone, Debug)]
pub struct WorkerPoolResult {
    pub summary: RunSummary,
    pub scheduler: Scheduler,
}

impl WorkerPool {
    pub fn new(
        run_id: impl Into<String>,
        scheduler: Scheduler,
        store: RunStore,
        config: WorkerPoolConfig,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            scheduler: Arc::new(Mutex::new(scheduler)),
            store,
            workers: Vec::new(),
            controls: Arc::new(Mutex::new(BTreeMap::new())),
            cancel_poll_interval_ms: 50,
            config,
        }
    }

    pub fn add_local_workers(
        &mut self,
        count: usize,
        capabilities: WorkerCapabilities,
        environment: Arc<dyn ExecutionEnvironment>,
    ) -> Result<(), String> {
        self.add_workers("local-worker", count, capabilities, environment)
    }

    pub fn add_workers(
        &mut self,
        worker_prefix: &str,
        count: usize,
        capabilities: WorkerCapabilities,
        environment: Arc<dyn ExecutionEnvironment>,
    ) -> Result<(), String> {
        if count == 0 {
            return Err("worker count must be greater than zero".into());
        }
        if worker_prefix.is_empty()
            || worker_prefix.len() > 64
            || worker_prefix.chars().any(char::is_control)
        {
            return Err("worker prefix must be non-empty and control-character safe".into());
        }
        if environment.kind() != capabilities.execution_environment.kind {
            return Err("worker capabilities do not match execution environment kind".into());
        }
        let health = environment.health();
        if !health.healthy {
            return Err(format!(
                "execution environment is unhealthy: {}: {}",
                health
                    .error_code
                    .as_deref()
                    .unwrap_or("ENVIRONMENT_UNHEALTHY"),
                health.message.as_deref().unwrap_or("no details")
            ));
        }
        for index in 0..count {
            let worker_id = format!("{worker_prefix}:{index:03}");
            if self
                .workers
                .iter()
                .any(|worker| worker.worker_id == worker_id)
            {
                return Err(format!("duplicate worker id: {worker_id}"));
            }
            self.workers.push(LocalWorker {
                worker_instance_id: format!("{worker_id}:instance:{}", now_unix_ms()),
                worker_id,
                capabilities: capabilities.clone(),
                environment: environment.clone(),
            });
        }
        Ok(())
    }

    pub fn cancel_attempt(&self, attempt_id: &str) -> bool {
        let Ok(controls) = self.controls.lock() else {
            return false;
        };
        let Some(control) = controls.get(attempt_id) else {
            return false;
        };
        control.cancel();
        true
    }

    pub fn run(self) -> Result<WorkerPoolResult, String> {
        if self.workers.is_empty() {
            return Err("worker pool has no workers".into());
        }
        let now = now_unix_ms();
        {
            let mut scheduler = self
                .scheduler
                .lock()
                .map_err(|_| "scheduler lock poisoned".to_string())?;
            if !scheduler.runs.contains_key(&self.run_id) {
                return Err(format!("run not found: {}", self.run_id));
            }
            // A persisted run may still contain the lease of a worker process
            // that disappeared before it could clean up. Expire that instance
            // before registering replacement workers with stable logical IDs.
            scheduler.tick(now)?;
            for worker in &self.workers {
                scheduler.register_worker(
                    &self.run_id,
                    &worker.worker_id,
                    &worker.worker_instance_id,
                    worker.capabilities.clone(),
                    1,
                    now,
                )?;
            }
            self.store.write_all(&scheduler, &self.run_id)?;
        }

        let shutdown = Arc::new(AtomicBool::new(false));
        let last_progress = Arc::new(AtomicU64::new(now));
        let cancel_controller = {
            let scheduler = self.scheduler.clone();
            let controls = self.controls.clone();
            let store = self.store.clone();
            let run_id = self.run_id.clone();
            let shutdown = shutdown.clone();
            let interval = self.cancel_poll_interval_ms;
            thread::spawn(move || {
                run_cancel_controller(scheduler, controls, store, run_id, shutdown, interval)
            })
        };
        let mut handles = Vec::new();
        for worker in self.workers {
            let scheduler = self.scheduler.clone();
            let controls = self.controls.clone();
            let store = self.store.clone();
            let config = self.config.clone();
            let run_id = self.run_id.clone();
            let shutdown = shutdown.clone();
            let last_progress = last_progress.clone();
            handles.push(thread::spawn(move || {
                run_worker(
                    worker,
                    scheduler,
                    controls,
                    store,
                    config,
                    run_id,
                    shutdown,
                    last_progress,
                )
            }));
        }

        let mut errors = Vec::new();
        for handle in handles {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => errors.push(error),
                Err(_) => errors.push("local worker thread panicked".into()),
            }
        }

        shutdown.store(true, Ordering::Release);
        match cancel_controller.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => errors.push(error),
            Err(_) => errors.push("cancellation controller thread panicked".into()),
        }

        let scheduler = self
            .scheduler
            .lock()
            .map_err(|_| "scheduler lock poisoned".to_string())?
            .clone();
        self.store.write_all(&scheduler, &self.run_id)?;
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        scheduler
            .assert_consistent()
            .map_err(|errors| errors.join("; "))?;
        let summary = report::aggregate(&scheduler, &self.run_id)?;
        Ok(WorkerPoolResult { summary, scheduler })
    }
}

#[allow(clippy::too_many_arguments)]
fn run_worker(
    worker: LocalWorker,
    scheduler: Arc<Mutex<Scheduler>>,
    controls: Arc<Mutex<BTreeMap<String, ExecutionControl>>>,
    store: RunStore,
    config: WorkerPoolConfig,
    run_id: String,
    shutdown: Arc<AtomicBool>,
    last_progress: Arc<AtomicU64>,
) -> Result<(), String> {
    let idle_poll = Duration::from_millis(config.idle_poll_ms.max(1));
    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        let now = now_unix_ms();
        let grant = {
            let mut scheduler = scheduler
                .lock()
                .map_err(|_| "scheduler lock poisoned".to_string())?;
            if scheduler.run_complete(&run_id) {
                None
            } else {
                scheduler.heartbeat_and_renew(
                    &worker.worker_id,
                    &worker.worker_instance_id,
                    None,
                    now,
                )?;
                scheduler.tick(now)?;
                let grant = scheduler.lease_next(&run_id, &worker.worker_id, now)?;
                if grant.is_some() {
                    store.write_all(&scheduler, &run_id)?;
                } else if scheduler.queue.len() > 0
                    && scheduler.leases.is_empty()
                    && now.saturating_sub(last_progress.load(Ordering::Acquire))
                        > config.no_progress_timeout_ms
                {
                    shutdown.store(true, Ordering::Release);
                    return Err(
                        "queued jobs cannot be matched to any registered worker/resource".into(),
                    );
                }
                grant
            }
        };

        let Some(grant) = grant else {
            let complete = scheduler
                .lock()
                .map_err(|_| "scheduler lock poisoned".to_string())?
                .run_complete(&run_id);
            if complete {
                break;
            }
            thread::sleep(idle_poll);
            continue;
        };

        last_progress.store(now_unix_ms(), Ordering::Release);
        let control = ExecutionControl::default();
        controls
            .lock()
            .map_err(|_| "execution controls lock poisoned".to_string())?
            .insert(grant.attempt.attempt_id.clone(), control.clone());
        {
            let mut scheduler = scheduler
                .lock()
                .map_err(|_| "scheduler lock poisoned".to_string())?;
            scheduler.mark_starting(&grant.lease.lease_id, now_unix_ms())?;
            scheduler.mark_running(&grant.lease.lease_id, now_unix_ms())?;
            store.write_all(&scheduler, &run_id)?;
        }

        let heartbeat_done = Arc::new(AtomicBool::new(false));
        let heartbeat = {
            let scheduler = scheduler.clone();
            let worker_id = worker.worker_id.clone();
            let worker_instance_id = worker.worker_instance_id.clone();
            let lease_id = grant.lease.lease_id.clone();
            let done = heartbeat_done.clone();
            let interval = Duration::from_millis(config.heartbeat_interval_ms.max(1));
            thread::spawn(move || {
                while !done.load(Ordering::Acquire) {
                    thread::sleep(interval);
                    if done.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(mut scheduler) = scheduler.lock() else {
                        break;
                    };
                    if scheduler
                        .heartbeat_and_renew(
                            &worker_id,
                            &worker_instance_id,
                            Some(&lease_id),
                            now_unix_ms(),
                        )
                        .is_err()
                    {
                        break;
                    }
                }
            })
        };

        let request = ExecutionRequest::from(&grant);
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            worker.environment.execute(&request, &control)
        }))
        .unwrap_or_else(|_| {
            crate::scheduler::AttemptOutcome::failed(
                crate::model::FailureClassification::InfrastructureFailure,
            )
        });
        heartbeat_done.store(true, Ordering::Release);
        let _ = heartbeat.join();
        controls
            .lock()
            .map_err(|_| "execution controls lock poisoned".to_string())?
            .remove(&grant.attempt.attempt_id);

        {
            let mut scheduler = scheduler
                .lock()
                .map_err(|_| "scheduler lock poisoned".to_string())?;
            if scheduler.leases.get(&grant.lease.lease_id).is_some() {
                scheduler.complete_attempt(&grant.lease.lease_id, outcome, now_unix_ms())?;
                last_progress.store(now_unix_ms(), Ordering::Release);
                store.write_all(&scheduler, &run_id)?;
            }
        }
    }

    let mut scheduler = scheduler
        .lock()
        .map_err(|_| "scheduler lock poisoned".to_string())?;
    if let Some(record) = scheduler.workers.get(&worker.worker_id) {
        if !matches!(record.state, WorkerState::Lost | WorkerState::Offline) {
            scheduler.workers.drain(&worker.worker_id)?;
            scheduler.workers.unregister(&worker.worker_id)?;
        }
    }
    store.write_all(&scheduler, &run_id)?;
    Ok(())
}

fn run_cancel_controller(
    scheduler: Arc<Mutex<Scheduler>>,
    controls: Arc<Mutex<BTreeMap<String, ExecutionControl>>>,
    store: RunStore,
    run_id: String,
    shutdown: Arc<AtomicBool>,
    interval_ms: u64,
) -> Result<(), String> {
    let interval = Duration::from_millis(interval_ms.max(1));
    while !shutdown.load(Ordering::Acquire) {
        let complete = scheduler
            .lock()
            .map_err(|_| "scheduler lock poisoned".to_string())?
            .run_complete(&run_id);
        if complete {
            break;
        }
        let Some(request) = store.take_cancel_request()? else {
            thread::sleep(interval);
            continue;
        };
        if request.run_id != run_id {
            store.restore_cancel_request(&request)?;
            return Err(format!(
                "cancellation request targets {}, active run is {run_id}",
                request.run_id
            ));
        }

        let mut scheduler = scheduler
            .lock()
            .map_err(|_| "scheduler lock poisoned".to_string())?;
        let job_ids = if let Some(job_id) = &request.job_id {
            if !scheduler.jobs.contains_key(job_id) {
                return Err(format!("cancellation job not found: {job_id}"));
            }
            vec![job_id.clone()]
        } else {
            scheduler
                .runs
                .get(&run_id)
                .map(|run| run.job_ids.clone())
                .ok_or_else(|| format!("run not found: {run_id}"))?
        };
        let mut retry_request = false;
        for job_id in job_ids {
            let active_attempt = scheduler
                .jobs
                .get(&job_id)
                .and_then(|job| job.active_lease_id.as_deref())
                .and_then(|lease_id| scheduler.leases.get(lease_id))
                .map(|lease| lease.attempt_id.clone());
            if let Some(attempt_id) = active_attempt {
                let controls = controls
                    .lock()
                    .map_err(|_| "execution controls lock poisoned".to_string())?;
                if let Some(control) = controls.get(&attempt_id) {
                    control.cancel();
                } else {
                    retry_request = true;
                }
            } else if scheduler
                .jobs
                .get(&job_id)
                .is_some_and(|job| !job.state.is_terminal())
            {
                scheduler.cancel_job(&job_id, now_unix_ms())?;
            }
        }
        store.write_all(&scheduler, &run_id)?;
        drop(scheduler);
        if retry_request {
            store.restore_cancel_request(&request)?;
        }
    }
    Ok(())
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        environment::{
            EnvironmentIsolation, ExecutionEnvironmentKind, ExecutionEnvironmentRequirement,
            WorkerEnvironmentCapabilities,
        },
        execution::{ExecutionControl, ExecutionRequest},
        model::{
            ApplicationSpec, ArtifactSpec, ExecutionRequirements, FailureClassification, JobSource,
            JobState, RetryPolicy, RunStatus, TEST_JOB_VERSION, TestJob,
            WINDOWS_INTERACTIVE_DESKTOP,
        },
        persistence::{CancellationRequest, PERSISTENCE_VERSION},
        scheduler::{AttemptOutcome, SchedulerConfig},
    };
    use std::{
        collections::BTreeMap,
        sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        time::{Duration, Instant, SystemTime},
    };

    struct ParallelFake {
        active: AtomicUsize,
        max_active: AtomicUsize,
    }

    impl ExecutionEnvironment for ParallelFake {
        fn execute(
            &self,
            _request: &ExecutionRequest,
            _control: &ExecutionControl,
        ) -> AttemptOutcome {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(15));
            self.active.fetch_sub(1, Ordering::SeqCst);
            AttemptOutcome::passed()
        }
    }

    struct KindPassFake(ExecutionEnvironmentKind);

    impl ExecutionEnvironment for KindPassFake {
        fn kind(&self) -> ExecutionEnvironmentKind {
            self.0
        }

        fn execute(
            &self,
            _request: &ExecutionRequest,
            _control: &ExecutionControl,
        ) -> AttemptOutcome {
            AttemptOutcome::passed()
        }
    }

    struct CancellableFake {
        started: Mutex<Option<mpsc::Sender<String>>>,
    }

    impl ExecutionEnvironment for CancellableFake {
        fn execute(
            &self,
            request: &ExecutionRequest,
            control: &ExecutionControl,
        ) -> AttemptOutcome {
            if let Some(sender) = self.started.lock().unwrap().take() {
                sender.send(request.attempt.attempt_id.clone()).unwrap();
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while !control.is_cancelled() {
                if Instant::now() >= deadline {
                    return AttemptOutcome::failed(FailureClassification::InfrastructureFailure);
                }
                thread::sleep(Duration::from_millis(2));
            }
            AttemptOutcome::failed(FailureClassification::Cancelled)
        }
    }

    fn job(index: usize, root: &std::path::Path) -> TestJob {
        TestJob {
            version: TEST_JOB_VERSION,
            job_id: format!("job:{index:03}"),
            run_id: "run:pool".into(),
            application: ApplicationSpec {
                key: format!("app:{index:03}"),
                executable: "fixture.exe".into(),
                args: Vec::new(),
                max_parallel: 1,
            },
            source: JobSource::Script {
                path: "fixture.aiui.json".into(),
            },
            fixtures: None,
            priority: (index % 3) as i32,
            timeout_ms: 5_000,
            retry: RetryPolicy::default(),
            requirements: ExecutionRequirements {
                interactive_desktop: false,
                physical_input: false,
                semantic_only: true,
                ..ExecutionRequirements::default()
            },
            artifacts: ArtifactSpec {
                output_dir: root.join(format!("jobs/{index:03}")),
            },
            metadata: BTreeMap::new(),
            created_at_ms: 0,
            retry_safe: true,
        }
    }

    #[test]
    fn four_local_workers_execute_semantic_jobs_with_backpressure() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("ai-ui-worker-pool-{}-{stamp}", std::process::id()));
        let jobs = (0..20).map(|index| job(index, &root)).collect();
        let mut scheduler = Scheduler::new(SchedulerConfig::default());
        scheduler
            .submit_run("run:pool", jobs, now_unix_ms())
            .unwrap();

        let environment = Arc::new(ParallelFake {
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        });
        let mut pool = WorkerPool::new(
            "run:pool",
            scheduler,
            RunStore::new(&root),
            WorkerPoolConfig::default(),
        );
        pool.add_local_workers(4, WorkerCapabilities::default(), environment.clone())
            .unwrap();
        let result = pool.run().unwrap();
        assert_eq!(result.summary.passed, 20);
        assert_eq!(result.summary.attempts, 20);
        assert!(environment.max_active.load(Ordering::SeqCst) >= 2);
        assert!(result.scheduler.queue.is_empty());
        assert!(result.scheduler.leases.is_empty());
        result.scheduler.assert_consistent().unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn running_job_cancellation_releases_attempt_lease_and_resource() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "ai-ui-worker-pool-cancel-{}-{stamp}",
            std::process::id()
        ));
        let mut cancelled_job = job(0, &root);
        cancelled_job.requirements = ExecutionRequirements {
            interactive_desktop: true,
            physical_input: true,
            semantic_only: false,
            ..ExecutionRequirements::default()
        };
        let job_id = cancelled_job.job_id.clone();
        let application_key = cancelled_job.application.key.clone();
        let mut scheduler = Scheduler::new(SchedulerConfig::default());
        scheduler
            .submit_run("run:pool", vec![cancelled_job], now_unix_ms())
            .unwrap();

        let (started_tx, started_rx) = mpsc::channel();
        let environment = Arc::new(CancellableFake {
            started: Mutex::new(Some(started_tx)),
        });
        let store = RunStore::new(&root);
        let mut pool = WorkerPool::new(
            "run:pool",
            scheduler,
            store.clone(),
            WorkerPoolConfig::default(),
        );
        pool.add_local_workers(1, WorkerCapabilities::default(), environment)
            .unwrap();
        let handle = thread::spawn(move || pool.run());

        let attempt_id = started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("worker did not start cancellable attempt");
        store
            .request_cancel(&CancellationRequest {
                version: PERSISTENCE_VERSION,
                run_id: "run:pool".into(),
                job_id: Some(job_id.clone()),
                requested_at_ms: now_unix_ms(),
            })
            .unwrap();
        let result = handle.join().unwrap().unwrap();

        assert_eq!(result.summary.status, RunStatus::Cancelled);
        assert_eq!(result.summary.cancelled, 1);
        assert_eq!(result.summary.attempts, 1);
        let record = &result.scheduler.jobs[&job_id];
        assert_eq!(record.state, JobState::Cancelled);
        assert_eq!(
            record.final_classification,
            Some(FailureClassification::Cancelled)
        );
        assert_eq!(record.attempt_ids, [attempt_id.clone()]);
        let attempt = &result.scheduler.attempts[&attempt_id];
        assert_eq!(attempt.status, JobState::Cancelled);
        assert_eq!(
            attempt.classification,
            Some(FailureClassification::Cancelled)
        );
        assert!(attempt.finished_at_ms.is_some());
        assert!(result.scheduler.queue.is_empty());
        assert!(result.scheduler.leases.is_empty());
        assert_eq!(result.scheduler.resources.active_lease_count(), 0);
        assert_eq!(
            result.scheduler.resources.used(WINDOWS_INTERACTIVE_DESKTOP),
            0
        );
        assert_eq!(
            result
                .scheduler
                .resources
                .application_used(&application_key),
            0
        );
        assert!(result.scheduler.events.iter().any(|event| {
            event.event == "attempt_finished"
                && event.attempt_id.as_deref() == Some(attempt_id.as_str())
        }));
        assert!(
            result
                .scheduler
                .events
                .iter()
                .any(|event| event.event == "resource_released")
        );
        assert!(!root.join("cancel-request.json").is_file());
        result.scheduler.assert_consistent().unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mixed_local_and_container_workers_match_only_their_environment_jobs() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "ai-ui-worker-pool-mixed-{}-{stamp}",
            std::process::id()
        ));
        let local_job = job(0, &root);
        let local_job_id = local_job.job_id.clone();
        let mut container_job = job(1, &root);
        container_job.requirements.execution_environment = ExecutionEnvironmentRequirement {
            environment_key: "container-test".into(),
            kind: ExecutionEnvironmentKind::Container,
            minimum_isolation: EnvironmentIsolation::Filesystem,
        };
        let container_job_id = container_job.job_id.clone();
        let mut scheduler = Scheduler::new(SchedulerConfig::default());
        scheduler
            .submit_run("run:pool", vec![local_job, container_job], now_unix_ms())
            .unwrap();

        let mut pool = WorkerPool::new(
            "run:pool",
            scheduler,
            RunStore::new(&root),
            WorkerPoolConfig::default(),
        );
        pool.add_local_workers(
            1,
            WorkerCapabilities::default(),
            Arc::new(KindPassFake(ExecutionEnvironmentKind::LocalProcess)),
        )
        .unwrap();
        let mut container_capabilities = WorkerCapabilities::default();
        container_capabilities.execution_environment = WorkerEnvironmentCapabilities {
            environment_key: "container-test".into(),
            kind: ExecutionEnvironmentKind::Container,
            isolation: EnvironmentIsolation::Filesystem,
            gui_level: Default::default(),
            reusable: false,
        };
        pool.add_workers(
            "container-worker",
            1,
            container_capabilities,
            Arc::new(KindPassFake(ExecutionEnvironmentKind::Container)),
        )
        .unwrap();
        let result = pool.run().unwrap();

        assert_eq!(result.summary.status, RunStatus::Passed);
        assert_eq!(result.summary.passed, 2);
        let local_attempt_id = &result.scheduler.jobs[&local_job_id].attempt_ids[0];
        let container_attempt_id = &result.scheduler.jobs[&container_job_id].attempt_ids[0];
        assert!(
            result.scheduler.attempts[local_attempt_id]
                .worker_id
                .starts_with("local-worker:")
        );
        assert!(
            result.scheduler.attempts[container_attempt_id]
                .worker_id
                .starts_with("container-worker:")
        );
        assert!(result.scheduler.queue.is_empty());
        assert!(result.scheduler.leases.is_empty());
        assert_eq!(result.scheduler.resources.active_lease_count(), 0);
        result.scheduler.assert_consistent().unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resumed_pool_expires_lost_instance_before_registering_replacement() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "ai-ui-worker-pool-resume-loss-{}-{stamp}",
            std::process::id()
        ));
        let now = now_unix_ms();
        let mut config = SchedulerConfig::default();
        config.worker_timeout_ms = 10;
        config.lease_duration_ms = 20;
        let mut scheduler = Scheduler::new(config);
        scheduler
            .submit_run("run:pool", vec![job(1, &root)], now.saturating_sub(100))
            .unwrap();
        scheduler
            .register_worker(
                "run:pool",
                "local-worker:000",
                "local-worker:000:old-instance",
                WorkerCapabilities::default(),
                1,
                now.saturating_sub(100),
            )
            .unwrap();
        let first = scheduler
            .lease_next("run:pool", "local-worker:000", now.saturating_sub(90))
            .unwrap()
            .unwrap();
        scheduler
            .mark_starting(&first.lease.lease_id, now.saturating_sub(89))
            .unwrap();
        scheduler
            .mark_running(&first.lease.lease_id, now.saturating_sub(88))
            .unwrap();

        let store = RunStore::new(&root);
        store.write_all(&scheduler, "run:pool").unwrap();
        let mut pool = WorkerPool::new("run:pool", scheduler, store, WorkerPoolConfig::default());
        pool.add_local_workers(
            1,
            WorkerCapabilities::default(),
            Arc::new(KindPassFake(ExecutionEnvironmentKind::LocalProcess)),
        )
        .unwrap();
        let result = pool.run().unwrap();

        let job = &result.scheduler.jobs["job:001"];
        assert_eq!(job.attempt_ids.len(), 2);
        assert_eq!(
            result.scheduler.attempts[&job.attempt_ids[0]].status,
            JobState::WorkerLost
        );
        assert_eq!(
            result.scheduler.attempts[&job.attempt_ids[1]].status,
            JobState::Passed
        );
        assert_ne!(job.attempt_ids[0], job.attempt_ids[1]);
        assert_eq!(result.summary.status, RunStatus::Passed);
        assert!(result.scheduler.leases.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
}
