use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use grein_tui::events::{AgentEvent, Event, EventHandler};
use std::time::Duration;

#[tokio::test]
async fn test_event_handler_emits_periodic_ticks() {
    let mut handler = EventHandler::new(Duration::from_millis(20));

    // Await next event, expecting a Tick within reasonable timeframe
    let event = tokio::time::timeout(Duration::from_millis(200), handler.next())
        .await
        .expect("timed out waiting for tick")
        .expect("event channel closed unexpectedly");

    match event {
        Event::Tick => {}
        other => panic!("expected Event::Tick, got {other:?}"),
    }
}

#[tokio::test]
async fn test_event_handler_receives_agent_event() {
    let mut handler = EventHandler::new(Duration::from_secs(60));
    let sender = handler.sender();

    let agent_payload = AgentEvent::Output {
        agent_id: "agent-1".to_string(),
        content: "processing task chunk".to_string(),
    };

    sender
        .send(Event::Agent(agent_payload.clone()))
        .expect("failed to send agent event");

    let received = tokio::time::timeout(Duration::from_millis(100), handler.next())
        .await
        .expect("timed out waiting for agent event")
        .expect("event channel closed unexpectedly");

    match received {
        Event::Agent(received_payload) => {
            assert_eq!(received_payload, agent_payload);
        }
        other => panic!("expected Event::Agent, got {other:?}"),
    }
}

#[tokio::test]
async fn test_event_handler_receives_key_event() {
    let mut handler = EventHandler::new(Duration::from_secs(60));
    let sender = handler.sender();

    let key_event = KeyEvent {
        code: KeyCode::Char('q'),
        modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Press,
        state: KeyEventState::empty(),
    };

    sender
        .send(Event::Key(key_event))
        .expect("failed to send key event");

    let received = tokio::time::timeout(Duration::from_millis(100), handler.next())
        .await
        .expect("timed out waiting for key event")
        .expect("event channel closed unexpectedly");

    match received {
        Event::Key(k) => {
            assert_eq!(k.code, KeyCode::Char('q'));
            assert_eq!(k.modifiers, KeyModifiers::CONTROL);
        }
        other => panic!("expected Event::Key, got {other:?}"),
    }
}

#[tokio::test]
async fn test_event_handler_multiplexing_preserves_fifo_order() {
    let mut handler = EventHandler::new(Duration::from_secs(60));
    let sender = handler.sender();

    let key1 = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT);
    let key2 = KeyEvent::new(KeyCode::Char('2'), KeyModifiers::ALT);

    sender.send(Event::Key(key1)).unwrap();
    sender
        .send(Event::Agent(AgentEvent::StatusUpdate {
            status: "idle".to_string(),
        }))
        .unwrap();
    sender.send(Event::Key(key2)).unwrap();

    let first = handler.next().await.expect("expected first event");
    match first {
        Event::Key(k) => assert_eq!(k.code, KeyCode::Char('1')),
        other => panic!("expected Key('1'), got {other:?}"),
    }

    let second = handler.next().await.expect("expected second event");
    match second {
        Event::Agent(AgentEvent::StatusUpdate { status }) => assert_eq!(status, "idle"),
        other => panic!("expected Agent status update, got {other:?}"),
    }

    let third = handler.next().await.expect("expected third event");
    match third {
        Event::Key(k) => assert_eq!(k.code, KeyCode::Char('2')),
        other => panic!("expected Key('2'), got {other:?}"),
    }
}
