pub mod allocator;
pub mod diff;
pub mod environment;
pub mod files;
pub mod git;
pub mod git_status;
pub mod port;
pub mod pty_session;
pub mod shadow;
pub mod shell;
pub mod teardown;
pub mod worktrees;

pub use files::list_workspace_files;
pub use grein_core::port::dto::{
    PtyCell, PtyColor, PtyMouseEncoding, PtyMouseMode, PtyScreenSnapshot,
};
pub use port::{
    GitCliInspector, GitInspectorPort, GitWorktreeDriver, PtyShellExecutor, PtySpawnerPort,
    ShadowCopyIsolator, ShellExecutorPort, TerminalPtyHandle, UnixPtySpawner,
    WorkspaceIsolationPort,
};
