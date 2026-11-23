use std::path::Path;
use std::process::{Command, Stdio};

pub fn list_workspace_files(root: &Path, max_entries: usize) -> Vec<String> {
    if max_entries == 0 || !root.is_dir() {
        return Vec::new();
    }

    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
    else {
        return Vec::new();
    };

    if !output.status.success() {
        return Vec::new();
    }

    let mut files: Vec<String> = output
        .stdout
        .split(|&byte| byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| String::from_utf8_lossy(entry).into_owned())
        .collect();
    files.sort();
    files.dedup();
    files.truncate(max_entries);
    files
}
