use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub job_id: String,
    pub priority: i32,
    pub sequence: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobQueue {
    entries: BTreeMap<String, QueueEntry>,
    next_sequence: u64,
    completed_history: Vec<String>,
}

impl JobQueue {
    /// Larger numbers have higher priority. Equal priority is strict FIFO.
    pub fn enqueue(&mut self, job_id: impl Into<String>, priority: i32) -> Result<u64, String> {
        let job_id = job_id.into();
        if self.entries.contains_key(&job_id) {
            return Err(format!("job is already queued: {job_id}"));
        }
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.entries.insert(
            job_id.clone(),
            QueueEntry {
                job_id,
                priority,
                sequence,
            },
        );
        Ok(sequence)
    }

    pub fn remove(&mut self, job_id: &str) -> Option<QueueEntry> {
        self.entries.remove(job_id)
    }

    pub fn contains(&self, job_id: &str) -> bool {
        self.entries.contains_key(job_id)
    }

    pub fn pop_matching(&mut self, mut predicate: impl FnMut(&str) -> bool) -> Option<QueueEntry> {
        let selected = self
            .entries
            .values()
            .filter(|entry| predicate(&entry.job_id))
            .max_by(|left, right| {
                left.priority
                    .cmp(&right.priority)
                    .then_with(|| right.sequence.cmp(&left.sequence))
            })
            .map(|entry| entry.job_id.clone())?;
        self.entries.remove(&selected)
    }

    pub fn mark_completed(&mut self, job_id: impl Into<String>) {
        let job_id = job_id.into();
        if !self.completed_history.contains(&job_id) {
            self.completed_history.push(job_id);
        }
    }

    pub fn completed_history(&self) -> &[String] {
        &self.completed_history
    }

    pub fn entries(&self) -> impl Iterator<Item = &QueueEntry> {
        self.entries.values()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn larger_priority_wins_and_equal_priority_is_fifo() {
        let mut queue = JobQueue::default();
        queue.enqueue("normal-first", 10).unwrap();
        queue.enqueue("urgent", 50).unwrap();
        queue.enqueue("normal-second", 10).unwrap();

        assert_eq!(queue.pop_matching(|_| true).unwrap().job_id, "urgent");
        assert_eq!(queue.pop_matching(|_| true).unwrap().job_id, "normal-first");
        assert_eq!(
            queue.pop_matching(|_| true).unwrap().job_id,
            "normal-second"
        );
    }

    #[test]
    fn cancellation_retry_and_matching_do_not_duplicate_active_entry() {
        let mut queue = JobQueue::default();
        queue.enqueue("a", 1).unwrap();
        queue.enqueue("b", 1).unwrap();
        assert!(queue.enqueue("a", 1).is_err());
        assert_eq!(queue.pop_matching(|job| job == "b").unwrap().job_id, "b");
        assert!(queue.contains("a"));
        queue.remove("a").unwrap();
        queue.enqueue("a", 1).unwrap();
        assert_eq!(queue.len(), 1);
    }
}
