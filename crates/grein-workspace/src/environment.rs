use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceEnvironment {
    GitRepository,
    NonGitDirectory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitEnvironmentStatus {
    pub git_repo_detected: bool,
    pub git_binary_present: bool,
    pub environment: WorkspaceEnvironment,
}

#[derive(Debug, Clone)]
pub struct EnvironmentDetector {
    git_binary_available: bool,
}

impl Default for EnvironmentDetector {
    fn default() -> Self {
        Self {
            git_binary_available: check_git_binary_in_path(),
        }
    }
}

impl EnvironmentDetector {
    pub fn new(git_binary_available: bool) -> Self {
        Self {
            git_binary_available,
        }
    }

    pub fn is_git_binary_available(&self) -> bool {
        self.git_binary_available
    }

    pub fn detect<P: AsRef<Path>>(&self, path: P) -> GitEnvironmentStatus {
        let path = path.as_ref();
        let git_repo_detected = path.ancestors().any(|p| p.join(".git").exists());
        let git_binary_present = self.git_binary_available;

        let environment = if git_repo_detected && git_binary_present {
            WorkspaceEnvironment::GitRepository
        } else {
            WorkspaceEnvironment::NonGitDirectory
        };

        GitEnvironmentStatus {
            git_repo_detected,
            git_binary_present,
            environment,
        }
    }
}

pub fn detect_environment<P: AsRef<Path>>(path: P) -> GitEnvironmentStatus {
    EnvironmentDetector::default().detect(path)
}

fn check_git_binary_in_path() -> bool {
    let path_env = std::env::var_os("PATH");
    if let Some(paths) = path_env {
        std::env::split_paths(&paths).any(|dir| {
            dir.join("git").is_file()
                || dir.join("git.exe").is_file()
                || dir.join("git.cmd").is_file()
        })
    } else {
        false
    }
}
