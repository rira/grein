use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Computes a unified diff between two directory trees.
///
/// Returns an empty string if both directories have identical file contents.
/// Formats file headers as `--- a/<rel_path>` and `+++ b/<rel_path>`.
pub fn compute_unified_diff(old_dir: &Path, new_dir: &Path) -> io::Result<String> {
    let mut relative_paths = BTreeSet::new();

    if old_dir.is_dir() {
        collect_relative_files(old_dir, old_dir, &mut relative_paths)?;
    }
    if new_dir.is_dir() {
        collect_relative_files(new_dir, new_dir, &mut relative_paths)?;
    }

    let mut diff_output = String::new();

    for rel_path in relative_paths {
        let old_file = old_dir.join(&rel_path);
        let new_file = new_dir.join(&rel_path);

        let old_content = if old_file.is_file() {
            Some(fs::read_to_string(&old_file)?)
        } else {
            None
        };

        let new_content = if new_file.is_file() {
            Some(fs::read_to_string(&new_file)?)
        } else {
            None
        };

        let rel_path_str = rel_path.to_string_lossy().replace('\\', "/");

        match (old_content, new_content) {
            (Some(old_text), Some(new_text)) => {
                if old_text != new_text {
                    let file_diff = format_unified_diff(
                        &rel_path_str,
                        &rel_path_str,
                        &old_text,
                        &new_text,
                        false,
                        false,
                    );
                    diff_output.push_str(&file_diff);
                }
            }
            (None, Some(new_text)) => {
                let file_diff =
                    format_unified_diff("/dev/null", &rel_path_str, "", &new_text, true, false);
                diff_output.push_str(&file_diff);
            }
            (Some(old_text), None) => {
                let file_diff =
                    format_unified_diff(&rel_path_str, "/dev/null", &old_text, "", false, true);
                diff_output.push_str(&file_diff);
            }
            (None, None) => {}
        }
    }

    Ok(diff_output)
}

fn collect_relative_files(
    base: &Path,
    current: &Path,
    paths: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    if !current.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_relative_files(base, &path, paths)?;
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(base) {
                paths.insert(rel.to_path_buf());
            }
        }
    }
    Ok(())
}

fn format_unified_diff(
    old_path: &str,
    new_path: &str,
    old_text: &str,
    new_text: &str,
    is_added: bool,
    is_deleted: bool,
) -> String {
    let old_lines: Vec<&str> = if old_text.is_empty() {
        Vec::new()
    } else {
        old_text.split_inclusive('\n').collect()
    };
    let new_lines: Vec<&str> = if new_text.is_empty() {
        Vec::new()
    } else {
        new_text.split_inclusive('\n').collect()
    };

    let mut header = String::new();
    if is_added {
        header.push_str(&format!("--- /dev/null\n+++ b/{}\n", new_path));
    } else if is_deleted {
        header.push_str(&format!("--- a/{}\n+++ /dev/null\n", old_path));
    } else {
        header.push_str(&format!("--- a/{}\n+++ b/{}\n", old_path, new_path));
    }

    let edits = compute_lcs_diff(&old_lines, &new_lines);
    if edits.is_empty() {
        return String::new();
    }

    let mut body = String::new();
    for edit in edits {
        match edit {
            DiffOp::Equal(line) => {
                body.push(' ');
                body.push_str(line.trim_end_matches(['\r', '\n']));
                body.push('\n');
            }
            DiffOp::Delete(line) => {
                body.push('-');
                body.push_str(line.trim_end_matches(['\r', '\n']));
                body.push('\n');
            }
            DiffOp::Insert(line) => {
                body.push('+');
                body.push_str(line.trim_end_matches(['\r', '\n']));
                body.push('\n');
            }
        }
    }

    format!("{}{}", header, body)
}

#[derive(Debug, PartialEq, Eq)]
enum DiffOp<T> {
    Equal(T),
    Delete(T),
    Insert(T),
}

fn compute_lcs_diff<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<DiffOp<&'a str>> {
    let m = a.len();
    let n = b.len();

    // DP table of LCS lengths
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 0..m {
        for j in 0..n {
            if a[i] == b[j] {
                dp[i + 1][j + 1] = dp[i][j] + 1;
            } else {
                dp[i + 1][j + 1] = dp[i + 1][j].max(dp[i][j + 1]);
            }
        }
    }

    // Backtrack to build diff operations
    let mut ops = Vec::new();
    let mut i = m;
    let mut j = n;

    while i > 0 || j > 0 {
        if i > 0 && j > 0 && a[i - 1] == b[j - 1] {
            ops.push(DiffOp::Equal(a[i - 1]));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            ops.push(DiffOp::Insert(b[j - 1]));
            j -= 1;
        } else if i > 0 && (j == 0 || dp[i][j - 1] < dp[i - 1][j]) {
            ops.push(DiffOp::Delete(a[i - 1]));
            i -= 1;
        }
    }

    ops.reverse();
    ops
}
