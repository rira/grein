use std::path::{Path, PathBuf};

mod acp;
mod acp_session;
mod pty;
mod rpc;
mod thread_merge;
mod threads;

#[cfg(unix)]
pub(super) fn write_executable_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, body).expect("write script");
    let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod");
    path
}
