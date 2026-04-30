use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub thread_id: String,
    pub connector: String,
    pub prompt_preview: String,
    pub state: JobState,
    pub created_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobReceipt {
    pub job_id: String,
    pub thread_id: String,
    pub connector: String,
    pub state: JobState,
    pub message: String,
    pub sequence: u64,
}

#[derive(Debug, Clone)]
pub struct JobStore {
    max_queue_depth: usize,
    max_receipts: usize,
    next_sequence: u64,
    queued: BTreeMap<String, VecDeque<Job>>,
    running: BTreeMap<String, Job>,
    receipts: VecDeque<JobReceipt>,
}

impl JobStore {
    pub fn new(max_queue_depth: usize, max_receipts: usize) -> Self {
        Self {
            max_queue_depth,
            max_receipts,
            next_sequence: 1,
            queued: BTreeMap::new(),
            running: BTreeMap::new(),
            receipts: VecDeque::new(),
        }
    }

    pub fn enqueue(
        &mut self,
        thread_id: impl Into<String>,
        connector: impl Into<String>,
        prompt: impl AsRef<str>,
    ) -> Result<Job, JobStoreError> {
        let thread_id = thread_id.into();
        if self.queued.get(&thread_id).map(VecDeque::len).unwrap_or(0) >= self.max_queue_depth {
            return Err(JobStoreError::QueueFull(thread_id));
        }

        let created_sequence = self.take_sequence();
        let job = Job {
            id: Uuid::new_v4().to_string(),
            thread_id,
            connector: connector.into(),
            prompt_preview: prompt_preview(prompt.as_ref()),
            state: JobState::Queued,
            created_sequence,
        };

        self.queued
            .entry(job.thread_id.clone())
            .or_default()
            .push_back(job.clone());
        Ok(job)
    }

    pub fn start_next(&mut self, thread_id: &str) -> Option<Job> {
        if self.running.contains_key(thread_id) {
            return None;
        }

        let queue = self.queued.get_mut(thread_id)?;
        let mut job = queue.pop_front()?;
        job.state = JobState::Running;
        self.running.insert(thread_id.to_string(), job.clone());
        Some(job)
    }

    pub fn finish(
        &mut self,
        job_id: &str,
        state: JobState,
        message: impl Into<String>,
    ) -> Result<JobReceipt, JobStoreError> {
        let (thread_id, job) = self
            .running
            .iter()
            .find(|(_thread_id, job)| job.id == job_id)
            .map(|(thread_id, job)| (thread_id.clone(), job.clone()))
            .ok_or_else(|| JobStoreError::UnknownJob(job_id.to_string()))?;

        self.running.remove(&thread_id);

        let receipt = JobReceipt {
            job_id: job.id,
            thread_id: job.thread_id,
            connector: job.connector,
            state,
            message: message.into(),
            sequence: self.take_sequence(),
        };

        self.push_receipt(receipt.clone());
        Ok(receipt)
    }

    pub fn cancel_running(&mut self, thread_id: &str) -> Result<JobReceipt, JobStoreError> {
        let job = self
            .running
            .remove(thread_id)
            .ok_or_else(|| JobStoreError::NoRunningJob(thread_id.to_string()))?;

        let receipt = JobReceipt {
            job_id: job.id,
            thread_id: job.thread_id,
            connector: job.connector,
            state: JobState::Canceled,
            message: "canceled by parent gateway".to_string(),
            sequence: self.take_sequence(),
        };

        self.push_receipt(receipt.clone());
        Ok(receipt)
    }

    pub fn queued_for_thread(&self, thread_id: &str) -> Vec<Job> {
        self.queued
            .get(thread_id)
            .map(|jobs| jobs.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn running_for_thread(&self, thread_id: &str) -> Option<&Job> {
        self.running.get(thread_id)
    }

    pub fn receipts(&self) -> Vec<JobReceipt> {
        self.receipts.iter().cloned().collect()
    }

    fn push_receipt(&mut self, receipt: JobReceipt) {
        self.receipts.push_back(receipt);

        while self.receipts.len() > self.max_receipts {
            self.receipts.pop_front();
        }
    }

    fn take_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        sequence
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobStoreError {
    #[error("queue for thread `{0}` is full")]
    QueueFull(String),
    #[error("unknown running job `{0}`")]
    UnknownJob(String),
    #[error("thread `{0}` has no running job")]
    NoRunningJob(String),
}

fn prompt_preview(prompt: &str) -> String {
    let normalized = prompt.split_whitespace().collect::<Vec<_>>().join(" ");

    if normalized.chars().count() <= 120 {
        normalized
    } else {
        normalized.chars().take(117).collect::<String>() + "..."
    }
}

#[cfg(test)]
mod tests {
    use super::{JobState, JobStore};

    #[test]
    fn queues_starts_finishes_and_records_receipts() {
        let mut store = JobStore::new(2, 10);
        let job = store.enqueue("main", "echo", "hello").unwrap();

        assert_eq!(store.queued_for_thread("main").len(), 1);
        let running = store.start_next("main").unwrap();
        assert_eq!(running.id, job.id);
        assert!(store.running_for_thread("main").is_some());

        let receipt = store
            .finish(&job.id, JobState::Succeeded, "done")
            .expect("finish running job");

        assert_eq!(receipt.state, JobState::Succeeded);
        assert_eq!(store.receipts().len(), 1);
        assert!(store.running_for_thread("main").is_none());
    }

    #[test]
    fn enforces_queue_depth_and_receipt_retention() {
        let mut store = JobStore::new(1, 1);
        let first = store.enqueue("main", "echo", "first").unwrap();
        assert!(store.enqueue("main", "echo", "second").is_err());

        store.start_next("main").unwrap();
        store
            .finish(&first.id, JobState::Succeeded, "first")
            .unwrap();

        let second = store.enqueue("main", "echo", "second").unwrap();
        store.start_next("main").unwrap();
        store
            .finish(&second.id, JobState::Failed, "second")
            .unwrap();

        let receipts = store.receipts();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].job_id, second.id);
    }
}
