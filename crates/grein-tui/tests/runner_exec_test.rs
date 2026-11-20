use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use grein_core::model::{AuthorRole, ThreadId};
use grein_core::storage::sqlite::SqliteStorage;
use grein_tui::app::App;
use grein_tui::engine_port::{
    HydratedBlackboardEntry, HydratedSession, HydratedThread, HydratedTurn, TuiEngineEvent,
    TuiEnginePort,
};
use tokio::sync::mpsc;

/// Plain test double: records dispatched prompts, replays a scripted event stream
/// and hydrates from a fixed session. No shell execution, no mockall.
struct RecordingEngine {
    session: HydratedSession,
    sent: Mutex<Vec<(String, String)>>,
    scripted_events: Mutex<Option<Vec<TuiEngineEvent>>>,
}

impl RecordingEngine {
    fn new(session: HydratedSession, events: Vec<TuiEngineEvent>) -> Self {
        Self {
            session,
            sent: Mutex::new(Vec::new()),
            scripted_events: Mutex::new(Some(events)),
        }
    }

    fn sent_turns(&self) -> Vec<(String, String)> {
        self.sent.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl TuiEnginePort for RecordingEngine {
    async fn hydrate_session(&self) -> io::Result<HydratedSession> {
        Ok(self.session.clone())
    }

    async fn send_turn(&self, thread_id: &str, prompt: &str) -> io::Result<()> {
        self.sent
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?
            .push((thread_id.to_string(), prompt.to_string()));
        Ok(())
    }

    async fn fork_thread(&self, thread_id: &str, _anchor_id: &str) -> io::Result<String> {
        Ok(format!("{thread_id}-fork"))
    }

    async fn merge_thread(&self, _source_id: &str, _target_id: &str) -> io::Result<()> {
        Ok(())
    }

    async fn resolve_permission(&self, _request_id: &str, _approved: bool) -> io::Result<()> {
        Ok(())
    }

    async fn subscribe_events(&self) -> io::Result<mpsc::UnboundedReceiver<TuiEngineEvent>> {
        let events = self
            .scripted_events
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?
            .take()
            .unwrap_or_default();
        let (tx, rx) = mpsc::unbounded_channel();
        for event in events {
            tx.send(event)
                .map_err(|e| io::Error::other(e.to_string()))?;
        }
        Ok(rx)
    }
}

/// Engine that hydrates from a real SqliteStorage file (no in-memory fakes).
struct SqliteBackedEngine {
    db_path: PathBuf,
}

#[async_trait::async_trait]
impl TuiEnginePort for SqliteBackedEngine {
    async fn hydrate_session(&self) -> io::Result<HydratedSession> {
        let storage =
            SqliteStorage::open(&self.db_path).map_err(|e| io::Error::other(format!("{e:?}")))?;
        let tid_entry = storage
            .get_blackboard_entry("active_thread_id")
            .map_err(|e| io::Error::other(format!("{e:?}")))?
            .ok_or_else(|| io::Error::other("missing active_thread_id"))?;
        let tid: ThreadId = tid_entry
            .value
            .parse()
            .ok()
            .map(ThreadId)
            .ok_or_else(|| io::Error::other("bad thread id"))?;
        let thread = storage
            .get_thread(tid)
            .map_err(|e| io::Error::other(format!("{e:?}")))?
            .ok_or_else(|| io::Error::other("thread row missing"))?;
        let trunk = storage
            .get_trunk_path(thread.active_leaf_id)
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        let turns = trunk
            .iter()
            .enumerate()
            .map(|(i, node)| HydratedTurn {
                id: format!("turn-{i}"),
                role: format!("{:?}", node.role),
                content: String::new(),
            })
            .collect();
        Ok(HydratedSession {
            active_thread_id: tid_entry.value.clone(),
            threads: vec![HydratedThread {
                id: tid_entry.value.clone(),
                title: "persisted".to_string(),
                turns,
            }],
            blackboard: Vec::new(),
        })
    }

    async fn send_turn(&self, _thread_id: &str, _prompt: &str) -> io::Result<()> {
        Ok(())
    }

    async fn fork_thread(&self, thread_id: &str, _anchor_id: &str) -> io::Result<String> {
        Ok(thread_id.to_string())
    }

    async fn merge_thread(&self, _source_id: &str, _target_id: &str) -> io::Result<()> {
        Ok(())
    }

    async fn resolve_permission(&self, _request_id: &str, _approved: bool) -> io::Result<()> {
        Ok(())
    }

    async fn subscribe_events(&self) -> io::Result<mpsc::UnboundedReceiver<TuiEngineEvent>> {
        let (_tx, rx) = mpsc::unbounded_channel();
        Ok(rx)
    }
}

fn sample_session() -> HydratedSession {
    HydratedSession {
        active_thread_id: "t-1".to_string(),
        threads: vec![HydratedThread {
            id: "t-1".to_string(),
            title: "Main".to_string(),
            turns: vec![
                HydratedTurn {
                    id: "u1".to_string(),
                    role: "User".to_string(),
                    content: "hello".to_string(),
                },
                HydratedTurn {
                    id: "a1".to_string(),
                    role: "Assistant".to_string(),
                    content: "world".to_string(),
                },
            ],
        }],
        blackboard: vec![HydratedBlackboardEntry {
            key: "goal".to_string(),
            value: "ship".to_string(),
        }],
    }
}

#[tokio::test]
async fn test_engine_port_prompt_dispatch_records_thread_and_prompt() {
    let engine = Arc::new(RecordingEngine::new(sample_session(), Vec::new()));
    let port: Arc<dyn TuiEnginePort> = engine.clone();

    let session = port.hydrate_session().await.unwrap();
    assert_eq!(session.active_thread_id, "t-1");
    assert_eq!(session.threads.len(), 1);
    assert_eq!(session.threads[0].title, "Main");
    assert_eq!(session.threads[0].turns[1].role, "Assistant");
    assert_eq!(session.threads[0].turns[1].content, "world");
    assert_eq!(session.blackboard[0].key, "goal");
    assert_eq!(session.blackboard[0].value, "ship");

    port.send_turn(&session.active_thread_id, "echo first")
        .await
        .unwrap();
    port.send_turn(&session.active_thread_id, "second")
        .await
        .unwrap();

    assert_eq!(
        engine.sent_turns(),
        vec![
            ("t-1".to_string(), "echo first".to_string()),
            ("t-1".to_string(), "second".to_string()),
        ]
    );
    let forked = port.fork_thread("t-1", "u1").await.unwrap();
    assert_eq!(forked, "t-1-fork");
}

#[tokio::test]
async fn test_engine_port_streaming_events_ingested_into_app_transcript() {
    let engine = RecordingEngine::new(
        sample_session(),
        vec![
            TuiEngineEvent::StreamChunk {
                thread_id: "t-1".to_string(),
                content: "stream_".to_string(),
            },
            TuiEngineEvent::StreamChunk {
                thread_id: "t-1".to_string(),
                content: "ok".to_string(),
            },
            TuiEngineEvent::TurnCompleted {
                thread_id: "t-1".to_string(),
            },
        ],
    );

    let mut rx = engine.subscribe_events().await.unwrap();
    let mut app = App::new();
    app.submit_composer_prompt("go".to_string());

    let mut buffer = String::new();
    let mut completed = false;
    while let Some(event) = rx.recv().await {
        match event {
            TuiEngineEvent::StreamChunk { thread_id, content } => {
                assert_eq!(thread_id, "t-1");
                buffer.push_str(&content);
            }
            TuiEngineEvent::TurnCompleted { thread_id } => {
                assert_eq!(thread_id, "t-1");
                completed = true;
                app.apply_agent_response(&buffer);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    assert!(completed, "TurnCompleted must terminate the stream");
    assert_eq!(buffer, "stream_ok");
    assert!(app.active_transcript_md.contains("stream_ok"));
    assert_eq!(app.last_submitted_prompt(), Some("go".to_string()));
}

#[tokio::test]
async fn test_engine_port_hydrates_from_real_sqlite_storage() {
    let db_path =
        std::env::temp_dir().join(format!("grein-runner-test-{}.sqlite3", ThreadId::now_v7()));
    let _ = std::fs::remove_file(&db_path);

    let mut app = App::new();
    app.submit_composer_prompt("echo first".to_string());
    app.apply_agent_response("first_output");
    app.persist_state_to_sqlite(&db_path);

    let meta_path = PathBuf::from(format!("{}.meta", db_path.display()));
    assert!(!meta_path.exists(), "Sidecar .meta file must not exist");

    let storage = SqliteStorage::open(&db_path).unwrap();
    let tid_entry = storage
        .get_blackboard_entry("active_thread_id")
        .unwrap()
        .expect("active_thread_id must be persisted in blackboard_entries");
    let tid: ThreadId = tid_entry.value.parse().ok().map(ThreadId).unwrap();
    let thread = storage
        .get_thread(tid)
        .unwrap()
        .expect("Thread row must exist");
    let trunk = storage.get_trunk_path(thread.active_leaf_id).unwrap();
    assert_eq!(trunk.len(), 2);
    assert_eq!(trunk[0].role, AuthorRole::User);
    assert_eq!(trunk[1].role, AuthorRole::Assistant);
    drop(storage);

    let engine = SqliteBackedEngine {
        db_path: db_path.clone(),
    };
    let session = engine.hydrate_session().await.unwrap();
    assert_eq!(session.active_thread_id, tid_entry.value);
    assert_eq!(session.threads.len(), 1);
    let roles: Vec<&str> = session.threads[0]
        .turns
        .iter()
        .map(|t| t.role.as_str())
        .collect();
    assert_eq!(roles, vec!["User", "Assistant"]);

    let mut loaded = App::new();
    loaded.load_state_from_sqlite(&db_path);
    assert_eq!(loaded.threads.len(), 1);
    assert_ne!(loaded.threads[0].id, "thread-1");
    assert!(loaded.active_transcript_md.contains("first_output"));

    let _ = std::fs::remove_file(&db_path);
}
