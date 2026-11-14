#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PaneFocus {
    #[default]
    Workspace,
    Composer,
    Blackboard,
}
