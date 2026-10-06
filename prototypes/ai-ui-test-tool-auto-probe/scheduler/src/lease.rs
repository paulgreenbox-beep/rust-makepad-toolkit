use crate::model::{LeaseRecord, WorkerRecord};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRegistry {
    leases: BTreeMap<String, LeaseRecord>,
    next_sequence: u64,
}

impl LeaseRegistry {
    pub fn preview_id(&self) -> String {
        format!("lease:{:08}", self.next_sequence)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn issue(
        &mut self,
        job_id: &str,
        worker: &WorkerRecord,
        attempt_id: &str,
        application_key: &str,
        now_ms: u64,
        duration_ms: u64,
    ) -> Result<LeaseRecord, String> {
        if duration_ms == 0 {
            return Err("lease duration must be greater than zero".into());
        }
        if self.leases.values().any(|lease| lease.job_id == job_id) {
            return Err(format!("job already has an active lease: {job_id}"));
        }
        let lease_id = self.preview_id();
        self.next_sequence = self.next_sequence.saturating_add(1);
        let lease = LeaseRecord {
            lease_id: lease_id.clone(),
            job_id: job_id.into(),
            worker_id: worker.worker_id.clone(),
            worker_instance_id: worker.worker_instance_id.clone(),
            attempt_id: attempt_id.into(),
            issued_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(duration_ms),
            renewed_at_ms: now_ms,
            resources: Vec::new(),
            application_key: application_key.into(),
        };
        self.leases.insert(lease_id, lease.clone());
        Ok(lease)
    }

    pub fn renew(
        &mut self,
        lease_id: &str,
        worker_id: &str,
        worker_instance_id: &str,
        now_ms: u64,
        duration_ms: u64,
    ) -> Result<LeaseRecord, String> {
        let lease = self
            .leases
            .get_mut(lease_id)
            .ok_or_else(|| format!("lease not found: {lease_id}"))?;
        if lease.worker_id != worker_id || lease.worker_instance_id != worker_instance_id {
            return Err("lease renewal does not match its worker instance".into());
        }
        if now_ms > lease.expires_at_ms {
            return Err("expired lease cannot be renewed".into());
        }
        lease.renewed_at_ms = now_ms;
        lease.expires_at_ms = now_ms.saturating_add(duration_ms);
        Ok(lease.clone())
    }

    pub fn set_resources(&mut self, lease_id: &str, resources: Vec<String>) -> Result<(), String> {
        let lease = self
            .leases
            .get_mut(lease_id)
            .ok_or_else(|| format!("lease not found: {lease_id}"))?;
        lease.resources = resources;
        Ok(())
    }

    pub fn remove(&mut self, lease_id: &str) -> Option<LeaseRecord> {
        self.leases.remove(lease_id)
    }

    pub fn get(&self, lease_id: &str) -> Option<&LeaseRecord> {
        self.leases.get(lease_id)
    }

    pub fn list(&self) -> impl Iterator<Item = &LeaseRecord> {
        self.leases.values()
    }

    pub fn expired(&self, now_ms: u64) -> Vec<String> {
        self.leases
            .values()
            .filter(|lease| now_ms > lease.expires_at_ms)
            .map(|lease| lease.lease_id.clone())
            .collect()
    }

    pub fn by_worker(&self, worker_id: &str) -> Vec<String> {
        self.leases
            .values()
            .filter(|lease| lease.worker_id == worker_id)
            .map(|lease| lease.lease_id.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.leases.len()
    }

    pub fn is_empty(&self) -> bool {
        self.leases.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{WorkerCapabilities, WorkerState};

    fn worker() -> WorkerRecord {
        WorkerRecord {
            worker_id: "worker:1".into(),
            worker_instance_id: "instance:1".into(),
            registered_at_ms: 0,
            last_heartbeat_ms: 0,
            capabilities: WorkerCapabilities::default(),
            capacity: 1,
            current_jobs: Vec::new(),
            state: WorkerState::Idle,
        }
    }

    #[test]
    fn lease_is_unique_renewable_and_expires_deterministically() {
        let mut leases = LeaseRegistry::default();
        let worker = worker();
        let lease = leases
            .issue("job:1", &worker, "attempt:1", "app", 100, 50)
            .unwrap();
        assert!(
            leases
                .issue("job:1", &worker, "attempt:2", "app", 100, 50)
                .is_err()
        );
        leases
            .renew(
                &lease.lease_id,
                &worker.worker_id,
                &worker.worker_instance_id,
                125,
                50,
            )
            .unwrap();
        assert!(leases.expired(175).is_empty());
        assert_eq!(leases.expired(176), [lease.lease_id]);
    }
}
