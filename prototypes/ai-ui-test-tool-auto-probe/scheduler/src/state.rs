use crate::model::{FailureClassification, JobRecord, JobState, TestJob};
use std::{error::Error, fmt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionError {
    pub from: JobState,
    pub to: JobState,
}

impl fmt::Display for TransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "illegal job state transition: {:?} -> {:?}",
            self.from, self.to
        )
    }
}

impl Error for TransitionError {}

pub fn can_transition(from: JobState, to: JobState) -> bool {
    use JobState::*;
    matches!(
        (from, to),
        (Queued, Ready | Cancelled | Blocked)
            | (Ready, Leased | Cancelled | Blocked | Skipped)
            | (
                Leased,
                Starting | Ready | Cancelled | WorkerLost | InfrastructureFailed
            )
            | (
                Starting,
                Running | Cancelled | TimedOut | WorkerLost | InfrastructureFailed
            )
            | (
                Running,
                Passed | Failed | TimedOut | Cancelled | WorkerLost | InfrastructureFailed
            )
            | (
                Failed | TimedOut | WorkerLost | InfrastructureFailed,
                RetryPending
            )
            | (RetryPending, Ready | Cancelled)
            | (Blocked, Ready | Cancelled | Skipped)
    )
}

pub fn transition(
    record: &mut JobRecord,
    to: JobState,
    now_ms: u64,
) -> Result<(), TransitionError> {
    if !can_transition(record.state, to) {
        return Err(TransitionError {
            from: record.state,
            to,
        });
    }
    record.state = to;
    record.updated_at_ms = now_ms;
    Ok(())
}

pub fn retry_allowed(
    job: &TestJob,
    classification: FailureClassification,
    attempts_completed: u32,
) -> bool {
    if attempts_completed >= job.retry.max_attempts {
        return false;
    }
    if !job.retry_safe && !job.retry.retry_non_idempotent {
        return false;
    }
    match classification {
        FailureClassification::TestFailure => job.retry.retry_test_failures,
        FailureClassification::InfrastructureFailure => job.retry.retry_infrastructure_failures,
        FailureClassification::Timeout => job.retry.retry_timeouts,
        FailureClassification::Cancelled => false,
        FailureClassification::WorkerLost => job.retry.retry_worker_lost,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ApplicationSpec, ArtifactSpec, ExecutionRequirements, JobSource, RetryPolicy,
        TEST_JOB_VERSION,
    };
    use std::collections::BTreeMap;

    fn job() -> TestJob {
        TestJob {
            version: TEST_JOB_VERSION,
            job_id: "job:state".into(),
            run_id: "run:state".into(),
            application: ApplicationSpec {
                key: "app".into(),
                executable: "app.exe".into(),
                args: Vec::new(),
                max_parallel: 1,
            },
            source: JobSource::Script {
                path: "state.aiui.json".into(),
            },
            fixtures: None,
            priority: 0,
            timeout_ms: 1_000,
            retry: RetryPolicy::default(),
            requirements: ExecutionRequirements::default(),
            artifacts: ArtifactSpec {
                output_dir: "reports/state".into(),
            },
            metadata: BTreeMap::new(),
            created_at_ms: 0,
            retry_safe: true,
        }
    }

    #[test]
    fn legal_path_reaches_passed_and_passed_cannot_restart() {
        let mut record = JobRecord {
            job: job(),
            state: JobState::Queued,
            attempt_ids: Vec::new(),
            active_lease_id: None,
            final_classification: None,
            updated_at_ms: 0,
        };
        for state in [
            JobState::Ready,
            JobState::Leased,
            JobState::Starting,
            JobState::Running,
            JobState::Passed,
        ] {
            transition(&mut record, state, 1).unwrap();
        }
        assert!(transition(&mut record, JobState::Running, 2).is_err());
    }

    #[test]
    fn retries_are_failure_typed_bounded_and_idempotency_guarded() {
        let mut value = job();
        assert!(!retry_allowed(
            &value,
            FailureClassification::TestFailure,
            1
        ));
        assert!(retry_allowed(
            &value,
            FailureClassification::InfrastructureFailure,
            1
        ));
        assert!(retry_allowed(&value, FailureClassification::WorkerLost, 1));
        assert!(!retry_allowed(&value, FailureClassification::Cancelled, 1));
        assert!(!retry_allowed(&value, FailureClassification::WorkerLost, 2));
        value.retry_safe = false;
        assert!(!retry_allowed(&value, FailureClassification::WorkerLost, 1));
        value.retry.retry_non_idempotent = true;
        assert!(retry_allowed(&value, FailureClassification::WorkerLost, 1));
    }
}
