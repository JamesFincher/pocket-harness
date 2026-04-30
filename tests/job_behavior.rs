use pocket_harness::jobs::{JobState, JobStore, JobStoreError};

#[test]
fn does_not_start_second_job_when_thread_is_already_running() {
    let mut store = JobStore::new(5, 10);
    let first = store.enqueue("main", "echo", "first").unwrap();
    let second = store.enqueue("main", "echo", "second").unwrap();

    assert_eq!(store.start_next("main").unwrap().id, first.id);
    assert!(store.start_next("main").is_none());
    assert_eq!(store.queued_for_thread("main")[0].id, second.id);
}

#[test]
fn can_cancel_running_job_and_then_start_next() {
    let mut store = JobStore::new(5, 10);
    let first = store.enqueue("main", "echo", "first").unwrap();
    let second = store.enqueue("main", "echo", "second").unwrap();

    store.start_next("main").unwrap();
    let receipt = store.cancel_running("main").unwrap();

    assert_eq!(receipt.job_id, first.id);
    assert_eq!(receipt.state, JobState::Canceled);
    assert_eq!(store.start_next("main").unwrap().id, second.id);
}

#[test]
fn finishing_unknown_job_reports_error() {
    let mut store = JobStore::new(5, 10);

    assert_eq!(
        store.finish("missing", JobState::Succeeded, "done"),
        Err(JobStoreError::UnknownJob("missing".to_string()))
    );
}

#[test]
fn cancel_without_running_job_reports_error() {
    let mut store = JobStore::new(5, 10);

    assert_eq!(
        store.cancel_running("main"),
        Err(JobStoreError::NoRunningJob("main".to_string()))
    );
}

#[test]
fn prompt_preview_is_normalized_and_truncated() {
    let mut store = JobStore::new(5, 10);
    let long_prompt = format!("hello\n{}\tgoodbye", "x".repeat(200));
    let job = store.enqueue("main", "echo", long_prompt).unwrap();

    assert!(!job.prompt_preview.contains('\n'));
    assert!(!job.prompt_preview.contains('\t'));
    assert!(job.prompt_preview.len() <= 120);
    assert!(job.prompt_preview.ends_with("..."));
}
