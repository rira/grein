use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

// We assert the contract for Composer and ExternalEditorRunner in grein_tui
use grein_tui::composer::{Composer, ComposerAction, ComposerEvent, KeyCode, KeyModifiers};
use grein_tui::editor::{EditorError, ExternalEditorRunner};

#[derive(Clone, Default)]
struct MockTerminalSuspender {
    suspended: Rc<RefCell<bool>>,
    resumed: Rc<RefCell<bool>>,
}

impl MockTerminalSuspender {
    fn new() -> Self {
        Self::default()
    }
}

#[derive(Clone)]
struct MockEditorRunner {
    suspender: MockTerminalSuspender,
    file_transform: Rc<dyn Fn(&Path) -> Result<(), EditorError>>,
    executed_path: Rc<RefCell<Option<PathBuf>>>,
}

impl MockEditorRunner {
    fn new(
        suspender: MockTerminalSuspender,
        transform: impl Fn(&Path) -> Result<(), EditorError> + 'static,
    ) -> Self {
        Self {
            suspender,
            file_transform: Rc::new(transform),
            executed_path: Rc::new(RefCell::new(None)),
        }
    }
}

impl ExternalEditorRunner for MockEditorRunner {
    fn run_editor(&self, file_path: &Path) -> Result<(), EditorError> {
        // Assert terminal was suspended before running editor
        assert!(
            *self.suspender.suspended.borrow(),
            "Terminal must be suspended before external editor is invoked"
        );
        assert!(
            !*self.suspender.resumed.borrow(),
            "Terminal must not be resumed while external editor is running"
        );

        *self.executed_path.borrow_mut() = Some(file_path.to_path_buf());
        (self.file_transform)(file_path)
    }
}

#[test]
fn test_ctrl_e_spawns_external_editor_and_repopulates_composer() {
    let suspender = MockTerminalSuspender::new();
    let suspender_clone = suspender.clone();

    let editor_runner = MockEditorRunner::new(suspender.clone(), |scratch_file: &Path| {
        // Verify initial draft was written to the scratch file
        let current_content =
            fs::read_to_string(scratch_file).map_err(|e| EditorError::Io(e.to_string()))?;
        assert_eq!(current_content, "Draft before editing");

        // Simulate user editing and saving in $EDITOR
        fs::write(scratch_file, "Draft updated from external editor")
            .map_err(|e| EditorError::Io(e.to_string()))?;
        Ok(())
    });

    let mut composer = Composer::with_editor_runner(editor_runner.clone()).with_terminal_hooks(
        {
            let s = suspender_clone.clone();
            move || *s.suspended.borrow_mut() = true
        },
        {
            let s = suspender_clone.clone();
            move || *s.resumed.borrow_mut() = true
        },
    );

    composer.set_buffer("Draft before editing");
    assert_eq!(composer.buffer(), "Draft before editing");

    // Dispatch Ctrl+E
    let event = ComposerEvent::Key {
        code: KeyCode::Char('e'),
        modifiers: KeyModifiers::CONTROL,
    };

    let action = composer
        .handle_event(event)
        .expect("Handling Ctrl+E must succeed");

    assert_eq!(action, ComposerAction::EditorExited);
    assert_eq!(
        composer.buffer(),
        "Draft updated from external editor",
        "Composer buffer must be re-populated with edited file contents"
    );

    // Assert terminal lifecycle was respected
    assert!(
        *suspender.suspended.borrow(),
        "Terminal must have been suspended"
    );
    assert!(
        *suspender.resumed.borrow(),
        "Terminal must have been resumed on editor exit"
    );

    // Assert temporary scratch file has been cleaned up
    let executed_path = editor_runner
        .executed_path
        .borrow()
        .clone()
        .expect("Editor runner must have captured scratch file path");
    assert!(
        !executed_path.exists(),
        "Scratch file must be cleaned up after composer re-population"
    );
}

#[test]
fn test_ctrl_e_retains_draft_if_editor_returns_error() {
    let suspender = MockTerminalSuspender::new();
    let editor_runner = MockEditorRunner::new(suspender.clone(), |_scratch_file: &Path| {
        Err(EditorError::ExecutionFailed("editor crashed".to_string()))
    });

    let mut composer = Composer::with_editor_runner(editor_runner).with_terminal_hooks(
        {
            let s = suspender.clone();
            move || *s.suspended.borrow_mut() = true
        },
        {
            let s = suspender.clone();
            move || *s.resumed.borrow_mut() = true
        },
    );

    composer.set_buffer("Preserved draft content");

    let event = ComposerEvent::Key {
        code: KeyCode::Char('e'),
        modifiers: KeyModifiers::CONTROL,
    };

    let result = composer.handle_event(event);
    assert!(
        result.is_err(),
        "Expected failure when external editor fails"
    );
    assert_eq!(
        composer.buffer(),
        "Preserved draft content",
        "Original buffer must be retained if editor execution fails"
    );
    assert!(
        *suspender.resumed.borrow(),
        "Terminal must be resumed even upon editor failure"
    );
}
