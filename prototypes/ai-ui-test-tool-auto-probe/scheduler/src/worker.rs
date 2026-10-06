use crate::model::{ExecutionRequirements, WorkerCapabilities, WorkerRecord, WorkerState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRegistry {
    workers: BTreeMap<String, WorkerRecord>,
}

impl WorkerRegistry {
    pub fn register(
        &mut self,
        worker_id: impl Into<String>,
        worker_instance_id: impl Into<String>,
        capabilities: WorkerCapabilities,
        capacity: usize,
        now_ms: u64,
    ) -> Result<WorkerRecord, String> {
        let worker_id = worker_id.into();
        let worker_instance_id = worker_instance_id.into();
        if worker_id.trim().is_empty() || worker_instance_id.trim().is_empty() {
            return Err("worker_id and worker_instance_id must not be empty".into());
        }
        if capacity == 0 || capabilities.max_parallel == 0 {
            return Err("worker capacity must be greater than zero".into());
        }
        if capacity > capabilities.max_parallel {
            return Err("worker capacity exceeds advertised max_parallel".into());
        }
        if self.workers.get(&worker_id).is_some_and(|worker| {
            !matches!(worker.state, WorkerState::Offline | WorkerState::Lost)
                || !worker.current_jobs.is_empty()
        }) {
            return Err(format!("worker is already registered: {worker_id}"));
        }
        let record = WorkerRecord {
            worker_id: worker_id.clone(),
            worker_instance_id,
            registered_at_ms: now_ms,
            last_heartbeat_ms: now_ms,
            capabilities,
            capacity,
            current_jobs: Vec::new(),
            state: WorkerState::Idle,
        };
        self.workers.insert(worker_id, record.clone());
        Ok(record)
    }

    pub fn heartbeat(
        &mut self,
        worker_id: &str,
        worker_instance_id: &str,
        now_ms: u64,
    ) -> Result<(), String> {
        let worker = self
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| format!("worker not found: {worker_id}"))?;
        if worker.worker_instance_id != worker_instance_id {
            return Err("stale worker instance heartbeat".into());
        }
        if matches!(worker.state, WorkerState::Offline | WorkerState::Lost) {
            return Err("offline/lost worker must register a new instance".into());
        }
        worker.last_heartbeat_ms = now_ms;
        Ok(())
    }

    pub fn drain(&mut self, worker_id: &str) -> Result<(), String> {
        let worker = self
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| format!("worker not found: {worker_id}"))?;
        worker.state = WorkerState::Draining;
        Ok(())
    }

    pub fn unregister(&mut self, worker_id: &str) -> Result<(), String> {
        let worker = self
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| format!("worker not found: {worker_id}"))?;
        if !worker.current_jobs.is_empty() {
            return Err("busy worker must drain before unregister".into());
        }
        worker.state = WorkerState::Offline;
        Ok(())
    }

    pub fn mark_lost(&mut self, worker_id: &str) -> Result<Vec<String>, String> {
        let worker = self
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| format!("worker not found: {worker_id}"))?;
        worker.state = WorkerState::Lost;
        Ok(worker.current_jobs.clone())
    }

    pub fn assign(&mut self, worker_id: &str, job_id: &str) -> Result<(), String> {
        let worker = self
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| format!("worker not found: {worker_id}"))?;
        if !matches!(worker.state, WorkerState::Idle | WorkerState::Busy) {
            return Err("worker is not accepting jobs".into());
        }
        if worker.current_jobs.len() >= worker.capacity {
            return Err("worker has no remaining capacity".into());
        }
        if worker.current_jobs.iter().any(|job| job == job_id) {
            return Err("job is already assigned to worker".into());
        }
        worker.current_jobs.push(job_id.into());
        worker.state = WorkerState::Busy;
        Ok(())
    }

    pub fn release(&mut self, worker_id: &str, job_id: &str) -> Result<(), String> {
        let worker = self
            .workers
            .get_mut(worker_id)
            .ok_or_else(|| format!("worker not found: {worker_id}"))?;
        worker.current_jobs.retain(|job| job != job_id);
        if worker.current_jobs.is_empty() && worker.state == WorkerState::Busy {
            worker.state = WorkerState::Idle;
        }
        Ok(())
    }

    pub fn available(&self, worker_id: &str) -> bool {
        self.workers.get(worker_id).is_some_and(|worker| {
            matches!(worker.state, WorkerState::Idle | WorkerState::Busy)
                && worker.current_jobs.len() < worker.capacity
        })
    }

    pub fn get(&self, worker_id: &str) -> Option<&WorkerRecord> {
        self.workers.get(worker_id)
    }

    pub fn get_mut(&mut self, worker_id: &str) -> Option<&mut WorkerRecord> {
        self.workers.get_mut(worker_id)
    }

    pub fn list(&self) -> impl Iterator<Item = &WorkerRecord> {
        self.workers.values()
    }
}

pub fn capabilities_match(
    capabilities: &WorkerCapabilities,
    requirements: &ExecutionRequirements,
) -> bool {
    capabilities
        .platform
        .eq_ignore_ascii_case(&requirements.platform)
        && (!requirements.interactive_desktop || capabilities.interactive_desktop)
        && (!requirements.gpu || capabilities.gpu)
        && (!requirements.semantic_ui || capabilities.semantic_ui)
        && (!requirements.physical_input || capabilities.physical_input)
        && (!requirements.makepad || capabilities.makepad)
        && capabilities
            .execution_environment
            .matches(&requirements.execution_environment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_matching_is_conservative() {
        let mut capabilities = WorkerCapabilities::default();
        let mut requirements = ExecutionRequirements::default();
        requirements.interactive_desktop = true;
        requirements.physical_input = true;
        assert!(capabilities_match(&capabilities, &requirements));
        capabilities.physical_input = false;
        assert!(!capabilities_match(&capabilities, &requirements));
        capabilities.physical_input = true;
        capabilities.platform = "linux".into();
        assert!(!capabilities_match(&capabilities, &requirements));
    }

    #[test]
    fn registration_heartbeat_capacity_and_drain_are_explicit() {
        let mut registry = WorkerRegistry::default();
        registry
            .register(
                "worker:1",
                "instance:1",
                WorkerCapabilities::default(),
                1,
                10,
            )
            .unwrap();
        registry.heartbeat("worker:1", "instance:1", 20).unwrap();
        registry.assign("worker:1", "job:1").unwrap();
        assert!(!registry.available("worker:1"));
        assert!(registry.unregister("worker:1").is_err());
        registry.release("worker:1", "job:1").unwrap();
        registry.drain("worker:1").unwrap();
        assert!(!registry.available("worker:1"));
        registry.unregister("worker:1").unwrap();
    }
}
