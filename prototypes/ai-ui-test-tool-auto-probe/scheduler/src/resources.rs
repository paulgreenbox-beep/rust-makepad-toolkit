use crate::model::{ExecutionRequirements, TestJob, WINDOWS_INTERACTIVE_DESKTOP};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ApplicationAllocation {
    key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourcePool {
    capacities: BTreeMap<String, usize>,
    used: BTreeMap<String, usize>,
    leases: BTreeMap<String, Vec<String>>,
    application_allocations: BTreeMap<String, ApplicationAllocation>,
    application_used: BTreeMap<String, usize>,
    application_limits: BTreeMap<String, usize>,
}

impl Default for ResourcePool {
    fn default() -> Self {
        Self::new(BTreeMap::from_iter([(
            WINDOWS_INTERACTIVE_DESKTOP.into(),
            1,
        )]))
    }
}

impl ResourcePool {
    pub fn new(capacities: BTreeMap<String, usize>) -> Self {
        Self {
            capacities,
            used: BTreeMap::new(),
            leases: BTreeMap::new(),
            application_allocations: BTreeMap::new(),
            application_used: BTreeMap::new(),
            application_limits: BTreeMap::new(),
        }
    }

    pub fn required(requirements: &ExecutionRequirements) -> Vec<String> {
        if requirements.interactive_desktop || requirements.physical_input {
            vec![WINDOWS_INTERACTIVE_DESKTOP.into()]
        } else {
            Vec::new()
        }
    }

    pub fn can_acquire(&self, lease_id: &str, job: &TestJob) -> bool {
        if self.leases.contains_key(lease_id) || self.application_allocations.contains_key(lease_id)
        {
            return false;
        }
        let resources = Self::required(&job.requirements);
        if resources.iter().any(|resource| {
            let capacity = self.capacities.get(resource).copied().unwrap_or(0);
            let used = self.used.get(resource).copied().unwrap_or(0);
            used >= capacity
        }) {
            return false;
        }
        let key = &job.application.key;
        let configured = self
            .application_limits
            .get(key)
            .copied()
            .unwrap_or(job.application.max_parallel)
            .min(job.application.max_parallel);
        self.application_used.get(key).copied().unwrap_or(0) < configured
    }

    pub fn acquire(&mut self, lease_id: &str, job: &TestJob) -> Result<Vec<String>, String> {
        if !self.can_acquire(lease_id, job) {
            return Err("required scheduler resource or application slot is unavailable".into());
        }
        let resources = Self::required(&job.requirements);
        for resource in &resources {
            *self.used.entry(resource.clone()).or_insert(0) += 1;
        }
        self.leases.insert(lease_id.into(), resources.clone());

        let key = job.application.key.clone();
        let limit = self
            .application_limits
            .entry(key.clone())
            .or_insert(job.application.max_parallel);
        *limit = (*limit).min(job.application.max_parallel);
        *self.application_used.entry(key.clone()).or_insert(0) += 1;
        self.application_allocations
            .insert(lease_id.into(), ApplicationAllocation { key });
        Ok(resources)
    }

    pub fn release(&mut self, lease_id: &str) -> bool {
        let mut released = false;
        if let Some(resources) = self.leases.remove(lease_id) {
            for resource in resources {
                if let Some(used) = self.used.get_mut(&resource) {
                    *used = used.saturating_sub(1);
                }
            }
            released = true;
        }
        if let Some(allocation) = self.application_allocations.remove(lease_id) {
            if let Some(used) = self.application_used.get_mut(&allocation.key) {
                *used = used.saturating_sub(1);
            }
            released = true;
        }
        released
    }

    pub fn used(&self, resource: &str) -> usize {
        self.used.get(resource).copied().unwrap_or(0)
    }

    pub fn application_used(&self, key: &str) -> usize {
        self.application_used.get(key).copied().unwrap_or(0)
    }

    pub fn active_lease_count(&self) -> usize {
        self.leases.len()
    }

    pub fn is_consistent(&self) -> bool {
        self.used
            .iter()
            .all(|(resource, used)| *used <= self.capacities.get(resource).copied().unwrap_or(0))
            && self
                .application_used
                .iter()
                .all(|(key, used)| *used <= self.application_limits.get(key).copied().unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ApplicationSpec, ArtifactSpec, JobSource, RetryPolicy, TEST_JOB_VERSION};
    use std::collections::BTreeMap;

    fn job(id: &str, application: &str, physical: bool) -> TestJob {
        TestJob {
            version: TEST_JOB_VERSION,
            job_id: id.into(),
            run_id: "run:resources".into(),
            application: ApplicationSpec {
                key: application.into(),
                executable: "app.exe".into(),
                args: Vec::new(),
                max_parallel: 1,
            },
            source: JobSource::Script {
                path: "test.aiui.json".into(),
            },
            fixtures: None,
            priority: 0,
            timeout_ms: 1_000,
            retry: RetryPolicy::default(),
            requirements: ExecutionRequirements {
                interactive_desktop: physical,
                physical_input: physical,
                semantic_only: !physical,
                ..ExecutionRequirements::default()
            },
            artifacts: ArtifactSpec {
                output_dir: "reports".into(),
            },
            metadata: BTreeMap::new(),
            created_at_ms: 0,
            retry_safe: true,
        }
    }

    #[test]
    fn desktop_capacity_is_one_and_release_allows_waiter() {
        let mut pool = ResourcePool::default();
        let first = job("job:1", "app-a", true);
        let second = job("job:2", "app-b", true);
        pool.acquire("lease:1", &first).unwrap();
        assert!(!pool.can_acquire("lease:2", &second));
        assert_eq!(pool.used(WINDOWS_INTERACTIVE_DESKTOP), 1);
        assert!(pool.release("lease:1"));
        assert!(pool.can_acquire("lease:2", &second));
    }

    #[test]
    fn per_application_limit_applies_even_without_desktop_resource() {
        let mut pool = ResourcePool::default();
        let first = job("job:1", "shared-app", false);
        let second = job("job:2", "shared-app", false);
        pool.acquire("lease:1", &first).unwrap();
        assert!(!pool.can_acquire("lease:2", &second));
        assert_eq!(pool.application_used("shared-app"), 1);
        pool.release("lease:1");
        assert!(pool.can_acquire("lease:2", &second));
        assert!(pool.is_consistent());
    }
}
