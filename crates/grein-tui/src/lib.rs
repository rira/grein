pub mod ui;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
pub use ui::tree::{AnchorNode, ThreadNode, TreeNode, TreeState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFocus {
    #[default]
    Tree,
    Composer,
    Viewer,
    Details,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeAction {
    ActivateAnchor(String),
}

#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub focus: PaneFocus,
    pub tree: TreeState,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }
}

pub fn handle_key_event(
    state: &mut AppState,
    key: KeyEvent,
) -> Result<Option<TreeAction>, Box<dyn std::error::Error + Send + Sync>> {
    if key.kind == KeyEventKind::Release {
        return Ok(None);
    }

    match key.code {
        KeyCode::Tab => {
            state.focus = match state.focus {
                PaneFocus::Tree => PaneFocus::Composer,
                PaneFocus::Composer => PaneFocus::Viewer,
                PaneFocus::Viewer => PaneFocus::Details,
                PaneFocus::Details => PaneFocus::Tree,
            };
            Ok(None)
        }
        KeyCode::BackTab => {
            state.focus = match state.focus {
                PaneFocus::Tree => PaneFocus::Details,
                PaneFocus::Composer => PaneFocus::Tree,
                PaneFocus::Viewer => PaneFocus::Composer,
                PaneFocus::Details => PaneFocus::Viewer,
            };
            Ok(None)
        }
        _ => {
            if state.focus == PaneFocus::Tree {
                match key.code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        state.tree.select_next();
                        Ok(None)
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        state.tree.select_previous();
                        Ok(None)
                    }
                    KeyCode::Enter => {
                        if let Some(idx) = state.tree.selected_index() {
                            if let Some(node) = state.tree.nodes.get(idx) {
                                match node {
                                    TreeNode::Anchor(anchor) => {
                                        let anchor_id = anchor.id.clone();
                                        state.tree.set_active_anchor_id(Some(anchor_id.clone()));
                                        return Ok(Some(TreeAction::ActivateAnchor(anchor_id)));
                                    }
                                    TreeNode::Thread(_) => {
                                        return Ok(None);
                                    }
                                }
                            }
                        }
                        Ok(None)
                    }
                    _ => Ok(None),
                }
            } else {
                Ok(None)
            }
        }
    }
}

pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Ok(())
}
