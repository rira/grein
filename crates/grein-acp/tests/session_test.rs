use grein_acp::session::{
    BranchThreadId, SessionError, SessionHandle, SessionIsolation, SessionMultiplexer,
};
use std::collections::HashSet;
use std::sync::Arc;
use std::thread;

#[test]
fn test_maps_distinct_dag_branch_threads_to_distinct_sessions() {
    let multiplexer = SessionMultiplexer::new(SessionIsolation::ProcessPerSession);

    let branch_a = BranchThreadId::new("dag-branch-feature-a");
    let branch_b = BranchThreadId::new("dag-branch-feature-b");

    let session_a = multiplexer
        .get_or_create_session(&branch_a)
        .expect("session A creation should succeed");
    let session_b = multiplexer
        .get_or_create_session(&branch_b)
        .expect("session B creation should succeed");

    assert_ne!(
        session_a.session_id(),
        session_b.session_id(),
        "parallel DAG branches must map to distinct ACP sessions"
    );
    assert_eq!(session_a.isolation(), SessionIsolation::ProcessPerSession);
    assert_eq!(session_b.isolation(), SessionIsolation::ProcessPerSession);
}

#[test]
fn test_same_dag_branch_thread_reuses_existing_active_session() {
    let multiplexer = SessionMultiplexer::new(SessionIsolation::ProcessPerSession);
    let branch = BranchThreadId::new("dag-branch-review-gate");

    let session_first = multiplexer
        .get_or_create_session(&branch)
        .expect("first session lookup should succeed");
    let session_second = multiplexer
        .get_or_create_session(&branch)
        .expect("subsequent session lookup should succeed");

    assert_eq!(
        session_first.session_id(),
        session_second.session_id(),
        "requests for the same branch thread must resolve to the identical session"
    );
    assert_eq!(multiplexer.active_session_count(), 1);
}

#[test]
fn test_concurrent_session_multiplexing_across_parallel_threads() {
    let multiplexer = Arc::new(SessionMultiplexer::new(SessionIsolation::ProcessPerSession));
    let thread_count = 10;
    let mut handles = Vec::new();

    for i in 0..thread_count {
        let multi_clone = Arc::clone(&multiplexer);
        handles.push(thread::spawn(move || {
            let branch_id = BranchThreadId::new(format!("dag-worker-thread-{}", i));
            let session = multi_clone
                .get_or_create_session(&branch_id)
                .expect("concurrent session creation must succeed");
            session.session_id().clone()
        }));
    }

    let mut session_ids = HashSet::new();
    for handle in handles {
        let session_id = handle.join().expect("thread join should succeed");
        session_ids.insert(session_id);
    }

    assert_eq!(
        session_ids.len(),
        thread_count,
        "every parallel thread must receive a unique ACP session under process-per-session isolation"
    );
    assert_eq!(multiplexer.active_session_count(), thread_count);
}

#[test]
fn test_terminate_session_removes_thread_mapping() {
    let multiplexer = SessionMultiplexer::new(SessionIsolation::ProcessPerSession);
    let branch = BranchThreadId::new("dag-transient-branch");

    let session = multiplexer
        .get_or_create_session(&branch)
        .expect("initial session creation");
    let initial_session_id = session.session_id().clone();

    multiplexer
        .terminate_session(&branch)
        .expect("session termination should succeed");
    assert_eq!(multiplexer.active_session_count(), 0);

    let new_session = multiplexer
        .get_or_create_session(&branch)
        .expect("session re-creation after termination");
    assert_ne!(
        new_session.session_id(),
        &initial_session_id,
        "re-created session must allocate a new unique session identifier"
    );
}

#[test]
fn test_rejects_empty_branch_identifier() {
    let multiplexer = SessionMultiplexer::new(SessionIsolation::ProcessPerSession);
    let empty_branch = BranchThreadId::new("");

    let result = multiplexer.get_or_create_session(&empty_branch);
    assert!(matches!(
        result,
        Err(SessionError::InvalidBranchIdentifier(_))
    ));
}
