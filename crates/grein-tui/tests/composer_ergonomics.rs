use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_tui::diff_card::{DiffCard, DiffCardAction};
use grein_tui::pager::{MockPagerRunner, PagerRunner};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[test]
fn test_contextual_reply_anchor_badge_rendering() {
    let anchor = "#### 2. Ingress Routing";
    let diff_card = DiffCard::builder()
        .reply_anchor(anchor)
        .diff_content("diff --git a/src/main.rs b/src/main.rs\n+println!(\"hello\");")
        .build();

    let mut buffer = Buffer::empty(Rect::new(0, 0, 60, 10));
    diff_card.render_badge(&mut buffer, Rect::new(0, 0, 60, 1));

    let rendered_line: String = (0..buffer.area.width)
        .map(|x| buffer.get(x, 0).symbol())
        .collect();

    assert!(
        rendered_line.contains("[Replying to: #### 2. Ingress Routing]"),
        "Expected contextual reply badge to contain targeted anchor format, got: {}",
        rendered_line
    );
}

#[test]
fn test_compact_diff_preview_card_syntax_highlight_lines() {
    let raw_diff = r#"--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 fn existing() {}
+fn added_feature() {}
-fn removed_legacy() {}
"#;

    let card = DiffCard::builder()
        .diff_content(raw_diff)
        .max_preview_lines(5)
        .build();

    let preview_lines = card.preview_lines();
    assert!(
        !preview_lines.is_empty(),
        "Preview lines should not be empty"
    );

    // Verify compact truncation
    assert!(
        preview_lines.len() <= 5,
        "Preview lines must honor max preview boundary"
    );

    // Verify syntax tag/style categorization for additions and deletions
    let has_addition = preview_lines.iter().any(|line| line.is_addition());
    let has_deletion = preview_lines.iter().any(|line| line.is_deletion());
    assert!(
        has_addition,
        "Expected at least one addition line with syntax distinction"
    );
    assert!(
        has_deletion,
        "Expected at least one deletion line with syntax distinction"
    );
}

#[test]
fn test_diff_preview_card_o_key_suspends_terminal_and_delegates_to_pager() {
    let diff_content = "diff --git a/a.rs b/a.rs\n+let x = 42;";
    let pager_invoked = Arc::new(AtomicBool::new(false));
    let terminal_suspended = Arc::new(AtomicBool::new(false));

    let mock_pager = MockPagerRunner::new({
        let pager_invoked = Arc::clone(&pager_invoked);
        let terminal_suspended = Arc::clone(&terminal_suspended);
        move |payload: &str| {
            assert_eq!(payload, diff_content);
            assert!(
                terminal_suspended.load(Ordering::SeqCst),
                "Terminal must be suspended before invoking pager"
            );
            pager_invoked.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    let mut card = DiffCard::builder()
        .diff_content(diff_content)
        .pager_runner(mock_pager)
        .build();

    // Key event 'o' to open in pager
    let key_event = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE);
    let action = card.handle_key(key_event, || {
        terminal_suspended.store(true, Ordering::SeqCst);
    });

    assert_eq!(action, DiffCardAction::OpenedPager);
    assert!(
        terminal_suspended.load(Ordering::SeqCst),
        "Terminal suspension callback must be triggered"
    );
    assert!(
        pager_invoked.load(Ordering::SeqCst),
        "External diff pager must be invoked with diff payload"
    );
}

#[test]
fn test_diff_preview_card_ignores_unrelated_key() {
    let card = DiffCard::builder().diff_content("+new code").build();

    let key_event = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
    let action = card.handle_key(key_event, || {
        panic!("Terminal should not suspend on unrelated key")
    });

    assert_eq!(action, DiffCardAction::Ignored);
}
