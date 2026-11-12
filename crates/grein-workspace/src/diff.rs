use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

fn compute_file_diff(file_path_str: &str, old_content: &str, new_content: &str) -> String {
    let old_lines: Vec<&str> = old_content.lines().collect();
    let new_lines: Vec<&str> = new_content.lines().collect();

    let n = old_lines.len();
    let m = new_lines.len();
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in 0..n {
        for j in 0..m {
            if old_lines[i] == new_lines[j] {
                dp[i + 1][j + 1] = dp[i][j] + 1;
            } else {
                dp[i + 1][j + 1] = dp[i + 1][j].max(dp[i][j + 1]);
            }
        }
    }

    let mut diff = Vec::new();
    let mut i = n;
    let mut j = m;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old_lines[i - 1] == new_lines[j - 1] {
            diff.push((' ', old_lines[i - 1]));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            diff.push(('+', new_lines[j - 1]));
            j -= 1;
        } else if i > 0 && (j == 0 || dp[i][j - 1] < dp[i - 1][j]) {
            diff.push(('-', old_lines[i - 1]));
            i -= 1;
        }
    }
    diff.reverse();

    let mut out = String::new();
    out.push_str(&format!("--- a/{}\n", file_path_str));
    out.push_str(&format!("+++ b/{}\n", file_path_str));
    out.push_str(&format!("@@ -1,{} +1,{} @@\n", n, m));
    for (op, line) in diff {
        out.push(op);
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn collect_all_relative_files(dir: &Path) -> io::Result<BTreeSet<PathBuf>> {
    let mut set = BTreeSet::new();
    if !dir.exists() {
        return Ok(set);
    }
    fn walk(base: &Path, cur: &Path, set: &mut BTreeSet<PathBuf>) -> io::Result<()> {
        for entry in fs::read_dir(cur)? {
            let entry = entry?;
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                if name == ".grein" || name == ".git" {
                    continue;
                }
            }
            if path.is_dir() {
                walk(base, &path, set)?;
            } else if path.is_file() {
                if let Ok(rel) = path.strip_prefix(base) {
                    set.insert(rel.to_path_buf());
                }
            }
        }
        Ok(())
    }
    walk(dir, dir, &mut set)?;
    Ok(set)
}

pub fn compute_directory_diff(primary: &Path, shadow: &Path) -> io::Result<String> {
    let primary_files = collect_all_relative_files(primary)?;
    let shadow_files = collect_all_relative_files(shadow)?;
    let all_files: BTreeSet<_> = primary_files.union(&shadow_files).cloned().collect();

    let mut diff_output = String::new();
    for rel_path in all_files {
        let p_path = primary.join(&rel_path);
        let s_path = shadow.join(&rel_path);

        let p_content = if p_path.exists() {
            fs::read_to_string(&p_path)?
        } else {
            String::new()
        };
        let s_content = if s_path.exists() {
            fs::read_to_string(&s_path)?
        } else {
            String::new()
        };

        if p_content != s_content {
            let rel_str = rel_path.to_string_lossy();
            let file_diff = compute_file_diff(&rel_str, &p_content, &s_content);
            diff_output.push_str(&file_diff);
        }
    }
    Ok(diff_output)
}
