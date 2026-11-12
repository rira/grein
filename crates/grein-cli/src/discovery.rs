use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AcpAgentKind {
    Agy,
    Claude,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpAgent {
    pub kind: AcpAgentKind,
    pub executable_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    IoError(String),
}

fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            meta.is_file() && (meta.permissions().mode() & 0o111 != 0)
        } else {
            false
        }
    }
    #[cfg(not(unix))]
    {
        if let Ok(meta) = fs::metadata(path) {
            meta.is_file()
        } else {
            false
        }
    }
}

pub fn discover_acp_agents(search_path: Option<&str>) -> Vec<AcpAgent> {
    let dirs: Vec<PathBuf> = match search_path {
        Some(p) => std::env::split_paths(p).collect(),
        None => std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default(),
    };

    let candidates = [("agy", AcpAgentKind::Agy), ("claude", AcpAgentKind::Claude)];

    let mut discovered = Vec::new();

    for (name, kind) in candidates {
        for dir in &dirs {
            let candidate_path = dir.join(name);
            if is_executable_file(&candidate_path) {
                discovered.push(AcpAgent {
                    kind,
                    executable_path: candidate_path,
                });
                break;
            }
        }
    }

    discovered
}

pub fn resolve_agents_configuration<P: AsRef<Path>>(
    config_path: Option<P>,
    search_path: Option<&str>,
) -> Result<Vec<AcpAgent>, DiscoveryError> {
    match config_path {
        Some(ref path) if path.as_ref().exists() => Ok(Vec::new()),
        _ => Ok(discover_acp_agents(search_path)),
    }
}
