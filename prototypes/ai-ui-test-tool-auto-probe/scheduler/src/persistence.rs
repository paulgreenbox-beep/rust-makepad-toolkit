use crate::{report, scheduler::Scheduler};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const PERSISTENCE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancellationRequest {
    pub version: u32,
    pub run_id: String,
    #[serde(default)]
    pub job_id: Option<String>,
    pub requested_at_ms: u64,
}

#[derive(Clone, Debug)]
pub struct RunStore {
    root: PathBuf,
}

impl RunStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn request_cancel(&self, request: &CancellationRequest) -> Result<(), String> {
        if request.version != PERSISTENCE_VERSION {
            return Err(format!(
                "cancellation request version must be {PERSISTENCE_VERSION}"
            ));
        }
        fs::create_dir_all(&self.root).map_err(|error| error.to_string())?;
        fs::write(
            self.root.join("cancel-request.json"),
            serde_json::to_vec_pretty(request).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())
    }

    pub fn take_cancel_request(&self) -> Result<Option<CancellationRequest>, String> {
        let path = self.root.join("cancel-request.json");
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(|error| error.to_string())?;
        let request = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        fs::remove_file(path).map_err(|error| error.to_string())?;
        Ok(Some(request))
    }

    pub fn restore_cancel_request(&self, request: &CancellationRequest) -> Result<(), String> {
        self.request_cancel(request)
    }

    pub fn write_all(&self, scheduler: &Scheduler, run_id: &str) -> Result<(), String> {
        fs::create_dir_all(&self.root).map_err(|error| error.to_string())?;
        self.write_json("scheduler-state.json", scheduler)?;
        self.write_json(
            "run.json",
            scheduler
                .runs
                .get(run_id)
                .ok_or_else(|| format!("run not found: {run_id}"))?,
        )?;

        let jobs = scheduler
            .runs
            .get(run_id)
            .into_iter()
            .flat_map(|run| &run.job_ids)
            .filter_map(|job_id| scheduler.jobs.get(job_id))
            .collect::<Vec<_>>();
        let attempts = jobs
            .iter()
            .flat_map(|job| &job.attempt_ids)
            .filter_map(|attempt_id| scheduler.attempts.get(attempt_id))
            .collect::<Vec<_>>();
        let workers = scheduler.workers.list().collect::<Vec<_>>();
        let events = scheduler
            .events
            .iter()
            .filter(|event| event.run_id == run_id || event.run_id == "scheduler")
            .collect::<Vec<_>>();

        self.write_json("jobs.json", &jobs)?;
        self.write_json("attempts.json", &attempts)?;
        self.write_json("workers.json", &workers)?;
        self.write_json("scheduler-events.json", &events)?;

        let mut jsonl = Vec::new();
        for event in events {
            serde_json::to_writer(&mut jsonl, event).map_err(|error| error.to_string())?;
            jsonl.push(b'\n');
        }
        fs::write(self.root.join("scheduler-events.jsonl"), jsonl)
            .map_err(|error| error.to_string())?;

        let summary = report::aggregate(scheduler, run_id)?;
        report::write(&self.root, &summary)?;
        Ok(())
    }

    pub fn load_scheduler(&self) -> Result<Scheduler, String> {
        let bytes =
            fs::read(self.root.join("scheduler-state.json")).map_err(|error| error.to_string())?;
        let scheduler: Scheduler =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if scheduler.version != crate::model::SCHEDULER_SNAPSHOT_VERSION {
            return Err(format!(
                "unsupported scheduler snapshot version {}",
                scheduler.version
            ));
        }
        scheduler
            .assert_consistent()
            .map_err(|errors| format!("inconsistent scheduler snapshot: {}", errors.join("; ")))?;
        Ok(scheduler)
    }

    fn write_json(&self, name: &str, value: &impl Serialize) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
        fs::write(self.root.join(name), bytes).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{
            ApplicationSpec, ArtifactSpec, ExecutionRequirements, JobSource, RetryPolicy,
            TEST_JOB_VERSION, TestJob,
        },
        scheduler::SchedulerConfig,
    };
    use std::{
        collections::BTreeMap,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn persisted_snapshot_and_event_log_round_trip() {
        let root = std::env::temp_dir().join(format!(
            "ai-ui-scheduler-store-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut scheduler = Scheduler::new(SchedulerConfig::default());
        let job = TestJob {
            version: TEST_JOB_VERSION,
            job_id: "job:persist".into(),
            run_id: "run:persist".into(),
            application: ApplicationSpec {
                key: "app".into(),
                executable: "app.exe".into(),
                args: Vec::new(),
                max_parallel: 1,
            },
            source: JobSource::Script {
                path: "persist.aiui.json".into(),
            },
            fixtures: None,
            priority: 0,
            timeout_ms: 1_000,
            retry: RetryPolicy::default(),
            requirements: ExecutionRequirements::default(),
            artifacts: ArtifactSpec {
                output_dir: root.join("jobs/job-persist"),
            },
            metadata: BTreeMap::new(),
            created_at_ms: 0,
            retry_safe: true,
        };
        scheduler.submit_run("run:persist", vec![job], 0).unwrap();
        let store = RunStore::new(&root);
        store.write_all(&scheduler, "run:persist").unwrap();
        let loaded = store.load_scheduler().unwrap();
        assert_eq!(loaded.queue.len(), 1);
        assert!(root.join("jobs.json").is_file());
        assert!(root.join("attempts.json").is_file());
        assert!(root.join("workers.json").is_file());
        assert!(root.join("scheduler-events.jsonl").is_file());
        assert!(root.join("run-summary.json").is_file());
        let _ = fs::remove_dir_all(root);
    }
}
