use std::fmt;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::editor::{EditorError, ExternalEditorRunner, SystemEditorRunner};
pub use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerAction {
    None,
    EditorExited,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerEvent {
    Key {
        code: KeyCode,
        modifiers: KeyModifiers,
    },
}

impl From<KeyEvent> for ComposerEvent {
    fn from(key: KeyEvent) -> Self {
        Self::Key {
            code: key.code,
            modifiers: key.modifiers,
        }
    }
}

pub struct Composer<R = SystemEditorRunner> {
    buffer: String,
    editor_runner: R,
    suspend_terminal: Box<dyn FnMut()>,
    resume_terminal: Box<dyn FnMut()>,
}

impl<R: fmt::Debug> fmt::Debug for Composer<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Composer")
            .field("buffer", &self.buffer)
            .field("editor_runner", &self.editor_runner)
            .finish()
    }
}

impl Composer<SystemEditorRunner> {
    pub fn new() -> Self {
        Self::with_editor_runner(SystemEditorRunner)
    }
}

impl Default for Composer<SystemEditorRunner> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: ExternalEditorRunner> Composer<R> {
    pub fn with_editor_runner(runner: R) -> Self {
        Self {
            buffer: String::new(),
            editor_runner: runner,
            suspend_terminal: Box::new(|| {}),
            resume_terminal: Box::new(|| {}),
        }
    }

    pub fn with_terminal_hooks<S, T>(mut self, suspend: S, resume: T) -> Self
    where
        S: FnMut() + 'static,
        T: FnMut() + 'static,
    {
        self.suspend_terminal = Box::new(suspend);
        self.resume_terminal = Box::new(resume);
        self
    }

    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    pub fn set_buffer(&mut self, buffer: impl Into<String>) {
        self.buffer = buffer.into();
    }

    pub fn handle_event(&mut self, event: ComposerEvent) -> Result<ComposerAction, EditorError> {
        match event {
            ComposerEvent::Key {
                code: KeyCode::Char('e'),
                modifiers,
            } if modifiers.contains(KeyModifiers::CONTROL) => self.spawn_external_editor(),
            _ => Ok(ComposerAction::None),
        }
    }

    fn spawn_external_editor(&mut self) -> Result<ComposerAction, EditorError> {
        let temp_dir = std::env::temp_dir();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let scratch_path = temp_dir.join(format!("grein_composer_{pid}_{timestamp}.txt"));

        fs::write(&scratch_path, &self.buffer).map_err(|e| EditorError::Io(e.to_string()))?;

        (self.suspend_terminal)();
        let editor_res = self.editor_runner.run_editor(&scratch_path);
        (self.resume_terminal)();

        match editor_res {
            Ok(()) => {
                let updated = fs::read_to_string(&scratch_path)
                    .map_err(|e| EditorError::Io(e.to_string()))?;
                let _ = fs::remove_file(&scratch_path);
                self.buffer = updated;
                Ok(ComposerAction::EditorExited)
            }
            Err(err) => {
                let _ = fs::remove_file(&scratch_path);
                Err(err)
            }
        }
    }
}
