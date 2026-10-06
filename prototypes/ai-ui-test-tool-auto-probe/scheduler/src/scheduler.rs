use crate::{
    environment::ExecutionEnvironmentMetadata,
    lease::LeaseRegistry,
    model::{
        AttemptRecord, FailureClassification, JobRecord, JobState, LeaseRecord, RunRecord,
        RunStatus, RuntimeDiagnostics, SchedulerEvent, TestJob, WINDOWS_INTERACTIVE_DESKTOP,
        WorkerCapabilities, WorkerState,
    },
    queue::JobQueue,
    resources::ResourcePool,
    state::{can_transition, retry_allowed, transition},
    worker::{WorkerRegistry, capabilities_match},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerConfig {
    pub lease_duration_ms: u64,
    pub worker_timeout_ms: u64,
    pub resource_capacities: BTreeMap<String, usize>,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            lease_duration_ms: 5_000,
            worker_timeout_ms: 15_000,
            resource_capacities: BTreeMap::from_iter([(WINDOWS_INTERACTIVE_DESKTOP.into(), 1)]),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptOutcome {
    #[serde(default)]
    pub classification: Option<FailureClassification>,
    #[serde(default)]
    pub script_report: Option<PathBuf>,
    #[serde(default)]
    pub evidence_refs: Vec<PathBuf>,
    #[serde(default)]
    pub runtime: Option<RuntimeDiagnostics>,
    #[serde(default)]
    pub execution_environment: Option<ExecutionEnvironmentMetadata>,
    #[serde(default)]
    pub environment_report: Option<PathBuf>,
}

impl AttemptOutcome {
    pub fn passed() -> Self {
        Self {
            classification: None,
            script_report: None,
            evidence_refs: Vec::new(),
            runtime: None,
            execution_environment: None,
            environment_report: None,
        }
    }

    pub fn failed(classification: FailureClassification) -> Self {
        Self {
            classification: Some(classification),
            script_report: None,
            evidence_refs: Vec::new(),
            runtime: None,
            execution_environment: None,
            environment_report: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeaseGrant {
    pub lease: LeaseRecord,
    pub job: TestJob,
    pub attempt: AttemptRecord,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelDirective {
    pub worker_id: String,
    pub worker_instance_id: String,
    pub lease_id: String,
    pub attempt_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scheduler {
    pub version: u32,
    pub config: SchedulerConfig,
    pub runs: BTreeMap<String, RunRecord>,
    pub jobs: BTreeMap<String, JobRecord>,
    pub attempts: BTreeMap<String, AttemptRecord>,
    pub queue: JobQueue,
    pub workers: WorkerRegistry,
    pub leases: LeaseRegistry,
    pub resources: ResourcePool,
    pub events: Vec<SchedulerEvent>,
    next_event_sequence: u64,
}

impl Scheduler {
    pub fn new(config: SchedulerConfig) -> Self {
        let resources = ResourcePool::new(config.resource_capacities.clone());
        Self {
            version: crate::model::SCHEDULER_SNAPSHOT_VERSION,
            config,
            runs: BTreeMap::new(),
            jobs: BTreeMap::new(),
            attempts: BTreeMap::new(),
            queue: JobQueue::default(),
            workers: WorkerRegistry::default(),
            leases: LeaseRegistry::default(),
            resources,
            events: Vec::new(),
            next_event_sequence: 0,
        }
    }

    pub fn submit_run(
        &mut self,
        run_id: impl Into<String>,
        jobs: Vec<TestJob>,
        now_ms: u64,
    ) -> Result<(), String> {
        let run_id = run_id.into();
        if run_id.trim().is_empty() {
            return Err("run_id must not be empty".into());
        }
        if self.runs.contains_key(&run_id) {
            return Err(format!("run already exists: {run_id}"));
        }
        if jobs.is_empty() {
            return Err("run must contain at least one job".into());
        }

        let mut seen = BTreeSet::new();
        for job in &jobs {
            job.validate()
                .map_err(|errors| format!("invalid job {}: {}", job.job_id, errors.join("; ")))?;
            if job.run_id != run_id {
                return Err(format!(
                    "job {} belongs to {}, expected {run_id}",
                    job.job_id, job.run_id
                ));
            }
            if self.jobs.contains_key(&job.job_id) || !seen.insert(job.job_id.clone()) {
                return Err(format!("duplicate job_id: {}", job.job_id));
            }
        }

        let job_ids = jobs.iter().map(|job| job.job_id.clone()).collect();
        self.runs.insert(
            run_id.clone(),
            RunRecord {
                run_id: run_id.clone(),
                status: RunStatus::Running,
                job_ids,
                created_at_ms: now_ms,
                started_at_ms: Some(now_ms),
                finished_at_ms: None,
            },
        );

        for job in jobs {
            let job_id = job.job_id.clone();
            let priority = job.priority;
            self.jobs.insert(
                job_id.clone(),
                JobRecord {
                    job,
                    state: JobState::Queued,
                    attempt_ids: Vec::new(),
                    active_lease_id: None,
                    final_classification: None,
                    updated_at_ms: now_ms,
                },
            );
            self.emit(
                &run_id,
                Some(&job_id),
                None,
                None,
                "job_enqueued",
                BTreeMap::from_iter([("priority".into(), json!(priority))]),
                now_ms,
            );
            {
                let record = self.jobs.get_mut(&job_id).expect("inserted job");
                transition(record, JobState::Ready, now_ms).map_err(|error| error.to_string())?;
            }
            self.queue.enqueue(&job_id, priority)?;
            self.emit(
                &run_id,
                Some(&job_id),
                None,
                None,
                "job_ready",
                BTreeMap::new(),
                now_ms,
            );
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn register_worker(
        &mut self,
        run_id: &str,
        worker_id: impl Into<String>,
        worker_instance_id: impl Into<String>,
        capabilities: WorkerCapabilities,
        capacity: usize,
        now_ms: u64,
    ) -> Result<(), String> {
        let worker_id = worker_id.into();
        let worker_instance_id = worker_instance_id.into();
        self.workers.register(
            worker_id.clone(),
            worker_instance_id.clone(),
            capabilities,
            capacity,
            now_ms,
        )?;
        self.emit(
            run_id,
            None,
            None,
            Some(&worker_id),
            "worker_registered",
            BTreeMap::from_iter([(
                "worker_instance_id".into(),
                Value::String(worker_instance_id),
            )]),
            now_ms,
        );
        Ok(())
    }

    pub fn heartbeat_and_renew(
        &mut self,
        worker_id: &str,
        worker_instance_id: &str,
        lease_id: Option<&str>,
        now_ms: u64,
    ) -> Result<(), String> {
        self.workers
            .heartbeat(worker_id, worker_instance_id, now_ms)?;
        if let Some(lease_id) = lease_id {
            self.leases.renew(
                lease_id,
                worker_id,
                worker_instance_id,
                now_ms,
                self.config.lease_duration_ms,
            )?;
        }
        Ok(())
    }

    pub fn lease_next(
        &mut self,
        run_id: &str,
        worker_id: &str,
        now_ms: u64,
    ) -> Result<Option<LeaseGrant>, String> {
        if !self.workers.available(worker_id) {
            return Ok(None);
        }
        let worker = self
            .workers
            .get(worker_id)
            .cloned()
            .ok_or_else(|| format!("worker not found: {worker_id}"))?;
        let preview_lease_id = self.leases.preview_id();
        let jobs = &self.jobs;
        let resources = &self.resources;
        let candidate = self.queue.pop_matching(|job_id| {
            jobs.get(job_id).is_some_and(|record| {
                record.job.run_id == run_id
                    && record.state == JobState::Ready
                    && capabilities_match(&worker.capabilities, &record.job.requirements)
                    && resources.can_acquire(&preview_lease_id, &record.job)
            })
        });
        let Some(entry) = candidate else {
            return Ok(None);
        };
        let job = self
            .jobs
            .get(&entry.job_id)
            .map(|record| record.job.clone())
            .ok_or_else(|| "queued job record disappeared".to_string())?;
        let attempt_number = self
            .jobs
            .get(&entry.job_id)
            .map(|record| record.attempt_ids.len() as u32 + 1)
            .unwrap_or(1);
        let attempt_id = format!("attempt:{}:{attempt_number:03}", entry.job_id);
        let lease = match self.leases.issue(
            &entry.job_id,
            &worker,
            &attempt_id,
            &job.application.key,
            now_ms,
            self.config.lease_duration_ms,
        ) {
            Ok(lease) => lease,
            Err(error) => {
                self.queue.enqueue(&entry.job_id, entry.priority)?;
                return Err(error);
            }
        };
        let acquired_resources = match self.resources.acquire(&lease.lease_id, &job) {
            Ok(resources) => resources,
            Err(error) => {
                self.leases.remove(&lease.lease_id);
                self.queue.enqueue(&entry.job_id, entry.priority)?;
                return Err(error);
            }
        };
        if let Err(error) = self.workers.assign(worker_id, &entry.job_id) {
            self.resources.release(&lease.lease_id);
            self.leases.remove(&lease.lease_id);
            self.queue.enqueue(&entry.job_id, entry.priority)?;
            return Err(error);
        }
        if let Err(error) = self
            .leases
            .set_resources(&lease.lease_id, acquired_resources.clone())
        {
            self.workers.release(worker_id, &entry.job_id)?;
            self.resources.release(&lease.lease_id);
            self.leases.remove(&lease.lease_id);
            self.queue.enqueue(&entry.job_id, entry.priority)?;
            return Err(error);
        }

        {
            let record = self
                .jobs
                .get_mut(&entry.job_id)
                .ok_or_else(|| "job record disappeared".to_string())?;
            if let Err(error) = transition(record, JobState::Leased, now_ms) {
                self.workers.release(worker_id, &entry.job_id)?;
                self.resources.release(&lease.lease_id);
                self.leases.remove(&lease.lease_id);
                self.queue.enqueue(&entry.job_id, entry.priority)?;
                return Err(error.to_string());
            }
            record.active_lease_id = Some(lease.lease_id.clone());
            record.attempt_ids.push(attempt_id.clone());
        }

        let mut lease = self
            .leases
            .get(&lease.lease_id)
            .cloned()
            .ok_or_else(|| "issued lease disappeared".to_string())?;
        lease.resources = acquired_resources.clone();
        let attempt = AttemptRecord {
            attempt_id: attempt_id.clone(),
            job_id: entry.job_id.clone(),
            number: attempt_number,
            worker_id: worker.worker_id.clone(),
            worker_instance_id: worker.worker_instance_id.clone(),
            lease_id: lease.lease_id.clone(),
            status: JobState::Leased,
            leased_at_ms: now_ms,
            started_at_ms: None,
            finished_at_ms: None,
            classification: None,
            script_report: None,
            evidence_refs: Vec::new(),
            runtime: None,
            execution_environment: None,
            environment_report: None,
        };
        self.attempts.insert(attempt_id.clone(), attempt.clone());

        for resource in &acquired_resources {
            self.emit(
                run_id,
                Some(&entry.job_id),
                Some(&attempt_id),
                Some(worker_id),
                "resource_acquired",
                BTreeMap::from_iter([("resource".into(), Value::String(resource.clone()))]),
                now_ms,
            );
        }
        self.emit(
            run_id,
            Some(&entry.job_id),
            Some(&attempt_id),
            Some(worker_id),
            "job_leased",
            BTreeMap::from_iter([
                ("lease_id".into(), Value::String(lease.lease_id.clone())),
                ("attempt_number".into(), json!(attempt_number)),
            ]),
            now_ms,
        );
        Ok(Some(LeaseGrant {
            lease,
            job,
            attempt,
        }))
    }

    pub fn mark_starting(&mut self, lease_id: &str, now_ms: u64) -> Result<(), String> {
        self.advance_attempt(lease_id, JobState::Starting, "attempt_started", now_ms)
    }

    pub fn mark_running(&mut self, lease_id: &str, now_ms: u64) -> Result<(), String> {
        self.advance_attempt(lease_id, JobState::Running, "attempt_running", now_ms)
    }

    fn advance_attempt(
        &mut self,
        lease_id: &str,
        to: JobState,
        event: &str,
        now_ms: u64,
    ) -> Result<(), String> {
        let lease = self
            .leases
            .get(lease_id)
            .cloned()
            .ok_or_else(|| format!("lease not found: {lease_id}"))?;
        let run_id = self
            .jobs
            .get(&lease.job_id)
            .map(|record| record.job.run_id.clone())
            .ok_or_else(|| "leased job not found".to_string())?;
        {
            let record = self
                .jobs
                .get_mut(&lease.job_id)
                .ok_or_else(|| "leased job not found".to_string())?;
            transition(record, to, now_ms).map_err(|error| error.to_string())?;
        }
        {
            let attempt = self
                .attempts
                .get_mut(&lease.attempt_id)
                .ok_or_else(|| "leased attempt not found".to_string())?;
            if !can_transition(attempt.status, to) {
                return Err(format!(
                    "illegal attempt transition: {:?} -> {:?}",
                    attempt.status, to
                ));
            }
            attempt.status = to;
            if attempt.started_at_ms.is_none() {
                attempt.started_at_ms = Some(now_ms);
            }
        }
        self.emit(
            &run_id,
            Some(&lease.job_id),
            Some(&lease.attempt_id),
            Some(&lease.worker_id),
            event,
            BTreeMap::new(),
            now_ms,
        );
        Ok(())
    }

    pub fn complete_attempt(
        &mut self,
        lease_id: &str,
        outcome: AttemptOutcome,
        now_ms: u64,
    ) -> Result<(), String> {
        let lease = self
            .leases
            .get(lease_id)
            .cloned()
            .ok_or_else(|| format!("lease not found: {lease_id}"))?;
        let run_id = self
            .jobs
            .get(&lease.job_id)
            .map(|record| record.job.run_id.clone())
            .ok_or_else(|| "leased job not found".to_string())?;
        let final_state = outcome
            .classification
            .map(FailureClassification::state)
            .unwrap_or(JobState::Passed);

        {
            let record = self
                .jobs
                .get_mut(&lease.job_id)
                .ok_or_else(|| "leased job not found".to_string())?;
            transition(record, final_state, now_ms).map_err(|error| error.to_string())?;
            record.active_lease_id = None;
            record.final_classification = outcome.classification;
        }
        {
            let attempt = self
                .attempts
                .get_mut(&lease.attempt_id)
                .ok_or_else(|| "leased attempt not found".to_string())?;
            if !can_transition(attempt.status, final_state) {
                return Err(format!(
                    "illegal attempt transition: {:?} -> {:?}",
                    attempt.status, final_state
                ));
            }
            attempt.status = final_state;
            attempt.finished_at_ms = Some(now_ms);
            attempt.classification = outcome.classification;
            attempt.script_report = outcome.script_report;
            attempt.evidence_refs = outcome.evidence_refs;
            attempt.runtime = outcome.runtime;
            attempt.execution_environment = outcome.execution_environment;
            attempt.environment_report = outcome.environment_report;
        }

        self.leases.remove(lease_id);
        self.resources.release(lease_id);
        self.workers.release(&lease.worker_id, &lease.job_id)?;

        for resource in &lease.resources {
            self.emit(
                &run_id,
                Some(&lease.job_id),
                Some(&lease.attempt_id),
                Some(&lease.worker_id),
                "resource_released",
                BTreeMap::from_iter([("resource".into(), Value::String(resource.clone()))]),
                now_ms,
            );
        }
        self.emit(
            &run_id,
            Some(&lease.job_id),
            Some(&lease.attempt_id),
            Some(&lease.worker_id),
            "attempt_finished",
            BTreeMap::from_iter([
                ("status".into(), json!(final_state)),
                ("classification".into(), json!(outcome.classification)),
            ]),
            now_ms,
        );

        let retry = outcome.classification.is_some_and(|classification| {
            self.jobs.get(&lease.job_id).is_some_and(|record| {
                retry_allowed(&record.job, classification, record.attempt_ids.len() as u32)
            })
        });
        if retry {
            let priority;
            {
                let record = self
                    .jobs
                    .get_mut(&lease.job_id)
                    .ok_or_else(|| "job disappeared before retry".to_string())?;
                transition(record, JobState::RetryPending, now_ms)
                    .map_err(|error| error.to_string())?;
                transition(record, JobState::Ready, now_ms).map_err(|error| error.to_string())?;
                record.final_classification = None;
                priority = record.job.priority;
            }
            self.queue.enqueue(&lease.job_id, priority)?;
            self.emit(
                &run_id,
                Some(&lease.job_id),
                Some(&lease.attempt_id),
                Some(&lease.worker_id),
                "retry_scheduled",
                BTreeMap::from_iter([(
                    "next_attempt".into(),
                    json!(self.jobs[&lease.job_id].attempt_ids.len() + 1),
                )]),
                now_ms,
            );
            self.emit(
                &run_id,
                Some(&lease.job_id),
                None,
                None,
                "job_ready",
                BTreeMap::new(),
                now_ms,
            );
        } else {
            self.queue.mark_completed(&lease.job_id);
        }
        self.refresh_run(&run_id, now_ms);
        Ok(())
    }

    pub fn cancel_job(
        &mut self,
        job_id: &str,
        now_ms: u64,
    ) -> Result<Option<CancelDirective>, String> {
        let record = self
            .jobs
            .get(job_id)
            .cloned()
            .ok_or_else(|| format!("job not found: {job_id}"))?;
        if record.state.is_terminal() {
            return Ok(None);
        }
        if let Some(lease_id) = record.active_lease_id {
            let lease = self
                .leases
                .get(&lease_id)
                .cloned()
                .ok_or_else(|| "job references missing active lease".to_string())?;
            let directive = CancelDirective {
                worker_id: lease.worker_id.clone(),
                worker_instance_id: lease.worker_instance_id.clone(),
                lease_id: lease.lease_id.clone(),
                attempt_id: lease.attempt_id.clone(),
            };
            self.complete_attempt(
                &lease_id,
                AttemptOutcome::failed(FailureClassification::Cancelled),
                now_ms,
            )?;
            return Ok(Some(directive));
        }

        self.queue.remove(job_id);
        {
            let record = self
                .jobs
                .get_mut(job_id)
                .ok_or_else(|| "job disappeared during cancellation".to_string())?;
            transition(record, JobState::Cancelled, now_ms).map_err(|error| error.to_string())?;
            record.final_classification = Some(FailureClassification::Cancelled);
        }
        self.queue.mark_completed(job_id);
        self.emit(
            &record.job.run_id,
            Some(job_id),
            None,
            None,
            "job_cancelled",
            BTreeMap::new(),
            now_ms,
        );
        self.refresh_run(&record.job.run_id, now_ms);
        Ok(None)
    }

    pub fn cancel_run(
        &mut self,
        run_id: &str,
        now_ms: u64,
    ) -> Result<Vec<CancelDirective>, String> {
        let job_ids = self
            .runs
            .get(run_id)
            .map(|run| run.job_ids.clone())
            .ok_or_else(|| format!("run not found: {run_id}"))?;
        let mut directives = Vec::new();
        for job_id in job_ids {
            if let Some(directive) = self.cancel_job(&job_id, now_ms)? {
                directives.push(directive);
            }
        }
        self.refresh_run(run_id, now_ms);
        Ok(directives)
    }

    pub fn tick(&mut self, now_ms: u64) -> Result<(), String> {
        let lost_workers = self
            .workers
            .list()
            .filter(|worker| {
                matches!(
                    worker.state,
                    WorkerState::Idle | WorkerState::Busy | WorkerState::Draining
                ) && now_ms.saturating_sub(worker.last_heartbeat_ms) > self.config.worker_timeout_ms
            })
            .map(|worker| worker.worker_id.clone())
            .collect::<Vec<_>>();

        for worker_id in lost_workers {
            let lease_ids = self.leases.by_worker(&worker_id);
            self.workers.mark_lost(&worker_id)?;
            let event_run = lease_ids
                .first()
                .and_then(|lease_id| self.leases.get(lease_id))
                .and_then(|lease| self.jobs.get(&lease.job_id))
                .map(|record| record.job.run_id.clone())
                .unwrap_or_else(|| "scheduler".into());
            self.emit(
                &event_run,
                None,
                None,
                Some(&worker_id),
                "worker_lost",
                BTreeMap::new(),
                now_ms,
            );
            for lease_id in lease_ids {
                if self.leases.get(&lease_id).is_some() {
                    self.complete_attempt(
                        &lease_id,
                        AttemptOutcome::failed(FailureClassification::WorkerLost),
                        now_ms,
                    )?;
                }
            }
        }

        let expired = self.leases.expired(now_ms);
        for lease_id in expired {
            if self.leases.get(&lease_id).is_some() {
                self.complete_attempt(
                    &lease_id,
                    AttemptOutcome::failed(FailureClassification::WorkerLost),
                    now_ms,
                )?;
            }
        }
        Ok(())
    }

    pub fn run_complete(&self, run_id: &str) -> bool {
        self.runs
            .get(run_id)
            .is_some_and(|run| run.status.is_terminal())
    }

    pub fn assert_consistent(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        for entry in self.queue.entries() {
            match self.jobs.get(&entry.job_id) {
                Some(record) if record.state == JobState::Ready => {}
                Some(record) => errors.push(format!(
                    "queued job {} has state {:?}",
                    entry.job_id, record.state
                )),
                None => errors.push(format!("queue references missing job {}", entry.job_id)),
            }
        }
        for (job_id, record) in &self.jobs {
            if let Some(lease_id) = &record.active_lease_id {
                match self.leases.get(lease_id) {
                    Some(lease) if lease.job_id == *job_id => {}
                    Some(_) => {
                        errors.push(format!("job {job_id} active lease belongs to another job"))
                    }
                    None => errors.push(format!("job {job_id} references missing lease")),
                }
            } else if matches!(
                record.state,
                JobState::Leased | JobState::Starting | JobState::Running
            ) {
                errors.push(format!("active job {job_id} has no lease"));
            }
            if record.state.is_terminal() && record.active_lease_id.is_some() {
                errors.push(format!("terminal job {job_id} retains a lease"));
            }
        }
        for lease in self.leases.list() {
            if self
                .jobs
                .get(&lease.job_id)
                .and_then(|record| record.active_lease_id.as_deref())
                != Some(lease.lease_id.as_str())
            {
                errors.push(format!("lease {} is not owned by its job", lease.lease_id));
            }
            if !self.attempts.contains_key(&lease.attempt_id) {
                errors.push(format!(
                    "lease {} references missing attempt {}",
                    lease.lease_id, lease.attempt_id
                ));
            }
        }
        for worker in self.workers.list() {
            for job_id in &worker.current_jobs {
                if !self
                    .leases
                    .list()
                    .any(|lease| lease.worker_id == worker.worker_id && lease.job_id == *job_id)
                {
                    errors.push(format!(
                        "worker {} retains job {job_id} without lease",
                        worker.worker_id
                    ));
                }
            }
        }
        if self.resources.active_lease_count() != self.leases.len() {
            errors.push("resource allocations and active leases differ".into());
        }
        if !self.resources.is_consistent() {
            errors.push("resource accounting exceeds configured capacity".into());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn refresh_run(&mut self, run_id: &str, now_ms: u64) {
        let Some(job_ids) = self.runs.get(run_id).map(|run| run.job_ids.clone()) else {
            return;
        };
        let states = job_ids
            .iter()
            .filter_map(|job_id| self.jobs.get(job_id).map(|record| record.state))
            .collect::<Vec<_>>();
        if states.len() != job_ids.len() || !states.iter().all(|state| state.is_terminal()) {
            if let Some(run) = self.runs.get_mut(run_id) {
                run.status = RunStatus::Running;
                run.started_at_ms.get_or_insert(now_ms);
            }
            return;
        }
        let status = if states
            .iter()
            .all(|state| matches!(state, JobState::Passed | JobState::Skipped))
        {
            RunStatus::Passed
        } else if states.iter().all(|state| *state == JobState::Cancelled) {
            RunStatus::Cancelled
        } else {
            RunStatus::Failed
        };
        let already_terminal = self
            .runs
            .get(run_id)
            .is_some_and(|run| run.status.is_terminal());
        if let Some(run) = self.runs.get_mut(run_id) {
            run.status = status;
            run.finished_at_ms = Some(now_ms);
        }
        if !already_terminal {
            self.emit(
                run_id,
                None,
                None,
                None,
                "run_completed",
                BTreeMap::from_iter([("status".into(), json!(status))]),
                now_ms,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit(
        &mut self,
        run_id: &str,
        job_id: Option<&str>,
        attempt_id: Option<&str>,
        worker_id: Option<&str>,
        event: &str,
        metadata: BTreeMap<String, Value>,
        now_ms: u64,
    ) {
        let sequence = self.next_event_sequence;
        self.next_event_sequence = self.next_event_sequence.saturating_add(1);
        self.events.push(SchedulerEvent {
            sequence,
            timestamp_ms: now_ms,
            run_id: run_id.into(),
            job_id: job_id.map(str::to_owned),
            attempt_id: attempt_id.map(str::to_owned),
            worker_id: worker_id.map(str::to_owned),
            event: event.into(),
            metadata,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ApplicationSpec, ArtifactSpec, ExecutionRequirements, JobSource, RetryPolicy,
        TEST_JOB_VERSION,
    };

    fn job(run_id: &str, id: &str, app: &str, priority: i32, physical: bool) -> TestJob {
        TestJob {
            version: TEST_JOB_VERSION,
            job_id: id.into(),
            run_id: run_id.into(),
            application: ApplicationSpec {
                key: app.into(),
                executable: format!("{app}.exe").into(),
                args: Vec::new(),
                max_parallel: 1,
            },
            source: JobSource::Script {
                path: format!("{id}.aiui.json").into(),
            },
            fixtures: None,
            priority,
            timeout_ms: 10_000,
            retry: RetryPolicy::default(),
            requirements: ExecutionRequirements {
                interactive_desktop: physical,
                physical_input: physical,
                semantic_only: !physical,
                ..ExecutionRequirements::default()
            },
            artifacts: ArtifactSpec {
                output_dir: format!("reports/{id}").into(),
            },
            metadata: BTreeMap::new(),
            created_at_ms: 0,
            retry_safe: true,
        }
    }

    fn register(scheduler: &mut Scheduler, run_id: &str, id: &str, now_ms: u64) {
        scheduler
            .register_worker(
                run_id,
                id,
                format!("{id}:instance"),
                WorkerCapabilities::default(),
                1,
                now_ms,
            )
            .unwrap();
    }

    #[test]
    fn run_job_attempt_lifecycle_passes_without_leaks() {
        let mut scheduler = Scheduler::new(SchedulerConfig::default());
        scheduler
            .submit_run(
                "run:pass",
                vec![job("run:pass", "job:pass", "app", 1, false)],
                0,
            )
            .unwrap();
        register(&mut scheduler, "run:pass", "worker:1", 0);
        let grant = scheduler
            .lease_next("run:pass", "worker:1", 1)
            .unwrap()
            .unwrap();
        scheduler.mark_starting(&grant.lease.lease_id, 2).unwrap();
        scheduler.mark_running(&grant.lease.lease_id, 3).unwrap();
        scheduler
            .complete_attempt(&grant.lease.lease_id, AttemptOutcome::passed(), 4)
            .unwrap();

        assert_eq!(scheduler.runs["run:pass"].status, RunStatus::Passed);
        assert_eq!(scheduler.jobs["job:pass"].state, JobState::Passed);
        assert_eq!(scheduler.attempts.len(), 1);
        assert!(scheduler.leases.is_empty());
        assert!(scheduler.queue.is_empty());
        scheduler.assert_consistent().unwrap();
    }

    #[test]
    fn worker_loss_preserves_attempt_and_requeues_to_another_worker() {
        let mut config = SchedulerConfig::default();
        config.worker_timeout_ms = 10;
        config.lease_duration_ms = 20;
        let mut scheduler = Scheduler::new(config);
        scheduler
            .submit_run(
                "run:loss",
                vec![job("run:loss", "job:loss", "app", 1, false)],
                0,
            )
            .unwrap();
        register(&mut scheduler, "run:loss", "worker:1", 0);
        let first = scheduler
            .lease_next("run:loss", "worker:1", 1)
            .unwrap()
            .unwrap();
        scheduler.mark_starting(&first.lease.lease_id, 2).unwrap();
        scheduler.mark_running(&first.lease.lease_id, 3).unwrap();
        scheduler.tick(12).unwrap();

        assert_eq!(
            scheduler.attempts[&first.attempt.attempt_id].status,
            JobState::WorkerLost
        );
        assert_eq!(scheduler.jobs["job:loss"].state, JobState::Ready);
        register(&mut scheduler, "run:loss", "worker:2", 12);
        let second = scheduler
            .lease_next("run:loss", "worker:2", 13)
            .unwrap()
            .unwrap();
        assert_ne!(first.attempt.attempt_id, second.attempt.attempt_id);
        scheduler.mark_starting(&second.lease.lease_id, 14).unwrap();
        scheduler.mark_running(&second.lease.lease_id, 15).unwrap();
        scheduler
            .complete_attempt(&second.lease.lease_id, AttemptOutcome::passed(), 16)
            .unwrap();

        assert_eq!(scheduler.jobs["job:loss"].attempt_ids.len(), 2);
        assert_eq!(scheduler.runs["run:loss"].status, RunStatus::Passed);
        scheduler.assert_consistent().unwrap();
    }

    #[test]
    fn physical_desktop_and_per_application_slots_backpressure_workers() {
        let mut scheduler = Scheduler::new(SchedulerConfig::default());
        scheduler
            .submit_run(
                "run:resources",
                vec![
                    job("run:resources", "job:a", "app-a", 1, true),
                    job("run:resources", "job:b", "app-b", 1, true),
                ],
                0,
            )
            .unwrap();
        register(&mut scheduler, "run:resources", "worker:1", 0);
        register(&mut scheduler, "run:resources", "worker:2", 0);
        let first = scheduler
            .lease_next("run:resources", "worker:1", 1)
            .unwrap()
            .unwrap();
        assert!(
            scheduler
                .lease_next("run:resources", "worker:2", 1)
                .unwrap()
                .is_none()
        );
        scheduler.mark_starting(&first.lease.lease_id, 2).unwrap();
        scheduler.mark_running(&first.lease.lease_id, 3).unwrap();
        scheduler
            .complete_attempt(&first.lease.lease_id, AttemptOutcome::passed(), 4)
            .unwrap();
        assert!(
            scheduler
                .lease_next("run:resources", "worker:2", 5)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn incompatible_worker_does_not_receive_interactive_job() {
        let mut scheduler = Scheduler::new(SchedulerConfig::default());
        scheduler
            .submit_run(
                "run:caps",
                vec![job("run:caps", "job:physical", "app", 1, true)],
                0,
            )
            .unwrap();
        let mut capabilities = WorkerCapabilities::default();
        capabilities.interactive_desktop = false;
        capabilities.physical_input = false;
        scheduler
            .register_worker(
                "run:caps",
                "worker:semantic",
                "instance:semantic",
                capabilities,
                1,
                0,
            )
            .unwrap();
        assert!(
            scheduler
                .lease_next("run:caps", "worker:semantic", 1)
                .unwrap()
                .is_none()
        );
        assert!(scheduler.queue.contains("job:physical"));
    }

    #[test]
    fn failure_types_drive_retry_and_cancellation_is_terminal() {
        let mut scheduler = Scheduler::new(SchedulerConfig::default());
        scheduler
            .submit_run(
                "run:failures",
                vec![
                    job("run:failures", "job:test", "app-a", 2, false),
                    job("run:failures", "job:infra", "app-b", 1, false),
                    job("run:failures", "job:cancel", "app-c", 0, false),
                ],
                0,
            )
            .unwrap();
        register(&mut scheduler, "run:failures", "worker:1", 0);

        let test = scheduler
            .lease_next("run:failures", "worker:1", 1)
            .unwrap()
            .unwrap();
        scheduler.mark_starting(&test.lease.lease_id, 2).unwrap();
        scheduler.mark_running(&test.lease.lease_id, 3).unwrap();
        scheduler
            .complete_attempt(
                &test.lease.lease_id,
                AttemptOutcome::failed(FailureClassification::TestFailure),
                4,
            )
            .unwrap();
        assert_eq!(scheduler.jobs["job:test"].state, JobState::Failed);

        let infra = scheduler
            .lease_next("run:failures", "worker:1", 5)
            .unwrap()
            .unwrap();
        scheduler.mark_starting(&infra.lease.lease_id, 6).unwrap();
        scheduler.mark_running(&infra.lease.lease_id, 7).unwrap();
        scheduler
            .complete_attempt(
                &infra.lease.lease_id,
                AttemptOutcome::failed(FailureClassification::InfrastructureFailure),
                8,
            )
            .unwrap();
        assert_eq!(scheduler.jobs["job:infra"].state, JobState::Ready);
        assert_eq!(scheduler.jobs["job:infra"].attempt_ids.len(), 1);

        scheduler.cancel_job("job:cancel", 9).unwrap();
        assert_eq!(scheduler.jobs["job:cancel"].state, JobState::Cancelled);
        scheduler.assert_consistent().unwrap();
    }
}
