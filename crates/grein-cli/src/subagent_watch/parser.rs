use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use grein_tui::engine_port::SubagentStatus;
use serde_json::Value;

use super::TrackedEntry;

pub(super) fn subagent_key(thread_id: &str, subagent_id: &str) -> String {
    format!("{thread_id}::{subagent_id}")
}

pub(super) fn child_transcript_path(brain_root: &Path, conversation_id: &str) -> PathBuf {
    brain_root
        .join(conversation_id)
        .join(".system_generated")
        .join("logs")
        .join("transcript.jsonl")
}

pub(super) fn resolve_truncated_line(transcript_path: &Path, val: &Value) -> Option<Value> {
    let has_truncated = val
        .get("truncated_fields")
        .and_then(Value::as_array)
        .is_some_and(|arr| !arr.is_empty());
    if !has_truncated {
        return None;
    }
    let step_idx = val.get("step_index").and_then(Value::as_u64)?;
    let full_path = transcript_path.with_file_name("transcript_full.jsonl");
    let content = fs::read_to_string(full_path).ok()?;
    for line in content.lines() {
        if let Ok(full_val) = serde_json::from_str::<Value>(line) {
            if full_val.get("step_index").and_then(Value::as_u64) == Some(step_idx) {
                return Some(full_val);
            }
        }
    }
    None
}

pub(super) fn read_complete_lines_from(path: &Path, prev_offset: u64) -> (u64, Vec<String>) {
    let Ok(mut file) = File::open(path) else {
        return (prev_offset, Vec::new());
    };
    let Ok(meta) = file.metadata() else {
        return (prev_offset, Vec::new());
    };
    let len = meta.len();
    let start = if len < prev_offset { 0 } else { prev_offset };
    if len == start {
        return (start, Vec::new());
    }
    if file.seek(SeekFrom::Start(start)).is_err() {
        return (start, Vec::new());
    }
    let mut buf = Vec::with_capacity((len - start) as usize);
    if file.read_to_end(&mut buf).is_err() {
        return (start, Vec::new());
    }
    let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') else {
        return (start, Vec::new());
    };
    let complete_slice = &buf[..=last_nl];
    let text = String::from_utf8_lossy(complete_slice);
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    (start + (last_nl as u64) + 1, lines)
}

#[derive(Debug, PartialEq, Eq)]
pub struct DiscoveredSubagent {
    pub conversation_id: String,
    pub role: Option<String>,
    pub agent_type: Option<String>,
}

pub fn extract_created_subagent(text: &str) -> Option<DiscoveredSubagent> {
    const MARKER: &str = "Created the following subagent:";
    let idx = text.find(MARKER)?;
    let rest = &text[idx + MARKER.len()..];
    let brace_start = rest.find('{')?;
    let json_candidate = extract_balanced_json_object(&rest[brace_start..])?;
    let val: Value = serde_json::from_str(json_candidate).ok()?;
    let conversation_id = val
        .get("conversationId")
        .or_else(|| val.get("conversation_id"))
        .or_else(|| val.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();
    let role = val
        .get("role")
        .or_else(|| val.get("Role"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let agent_type = val
        .get("typeName")
        .or_else(|| val.get("TypeName"))
        .or_else(|| val.get("agentType"))
        .or_else(|| val.get("type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some(DiscoveredSubagent {
        conversation_id,
        role,
        agent_type,
    })
}

fn extract_balanced_json_object(s: &str) -> Option<&str> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (idx, ch) in s.char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&s[..=idx]);
                }
            }
            _ => {}
        }
    }
    None
}

pub(super) fn extract_message_senders(content: &str) -> Vec<String> {
    let mut senders = Vec::new();
    for line in content.lines() {
        if !line.contains("[Message]") {
            continue;
        }
        if let Some(pos) = line.find("sender=") {
            let after = &line[pos + "sender=".len()..];
            let id: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if !id.is_empty() {
                senders.push(id);
            }
        }
    }
    senders
}

pub(super) fn find_matching_unlinked_key(
    entries: &HashMap<String, TrackedEntry>,
    role: Option<&str>,
    agent_type: Option<&str>,
) -> Option<String> {
    let mut unlinked: Vec<(&String, &TrackedEntry)> = entries
        .iter()
        .filter(|(_, e)| {
            e.snapshot.status == SubagentStatus::Running && e.child_conversation_id.is_none()
        })
        .collect();
    unlinked.sort_by_key(|(_, e)| e.generation);

    if let (Some(r), Some(t)) = (role, agent_type) {
        if let Some((k, _)) = unlinked.iter().find(|(_, e)| {
            e.snapshot.role.eq_ignore_ascii_case(r)
                && e.snapshot.agent_type.eq_ignore_ascii_case(t)
        }) {
            return Some((*k).clone());
        }
    }
    if let Some(r) = role {
        if let Some((k, _)) = unlinked
            .iter()
            .find(|(_, e)| e.snapshot.role.eq_ignore_ascii_case(r))
        {
            return Some((*k).clone());
        }
    }
    if let Some(t) = agent_type {
        let matches: Vec<_> = unlinked
            .iter()
            .filter(|(_, e)| e.snapshot.agent_type.eq_ignore_ascii_case(t))
            .collect();
        if matches.len() == 1 {
            return Some(matches[0].0.clone());
        }
    }
    unlinked.first().map(|(k, _)| (*k).clone())
}

pub fn shorten_model_name(raw: &str) -> String {
    let base = raw.trim().rsplit('/').next().unwrap_or(raw.trim());
    let without_at = base.split('@').next().unwrap_or(base);
    let mut result = without_at.to_string();

    if let Some(idx) = result.find("-preview") {
        result.truncate(idx);
    }
    if let Some(idx) = result.find("-exp-") {
        result.truncate(idx);
    }
    if let Some((head, tail)) = result.rsplit_once('-') {
        if tail.len() == 8 && tail.chars().all(|c| c.is_ascii_digit()) {
            result = head.to_string();
        }
    }
    result
}

fn strip_extra_quotes(s: &str) -> &str {
    s.trim().trim_matches('"')
}

fn format_tool_activity(tc: &Value) -> Option<String> {
    let name = tc
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let args = tc.get("args");
    let detail = args.and_then(|a| {
        if let Some(p) = a.get("Path").or_else(|| a.get("path")).and_then(Value::as_str) {
            let clean = strip_extra_quotes(p);
            return Path::new(clean)
                .file_name()
                .and_then(|f| f.to_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty());
        }
        if let Some(cmd) = a
            .get("CommandLine")
            .or_else(|| a.get("command"))
            .and_then(Value::as_str)
        {
            let clean = strip_extra_quotes(cmd);
            return Some(truncate_chars(clean, 28));
        }
        if let Some(q) = a.get("query").or_else(|| a.get("Query")).and_then(Value::as_str) {
            let clean = strip_extra_quotes(q);
            return Some(truncate_chars(clean, 28));
        }
        None
    });
    Some(match detail {
        Some(d) if !d.is_empty() => format!("{name} {d}"),
        _ => name.to_string(),
    })
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let single_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = single_line.chars();
    let taken: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() && max_chars > 1 {
        let mut out: String = taken.chars().take(max_chars - 1).collect();
        out.push('…');
        out
    } else {
        taken
    }
}

pub(super) fn extract_child_activity(val: &Value) -> Option<String> {
    if let Some(calls) = val.get("tool_calls").and_then(Value::as_array) {
        if let Some(last_tc) = calls.last() {
            if let Some(act) = format_tool_activity(last_tc) {
                return Some(act);
            }
        }
    }
    if let Some(name) = val
        .get("tool_name")
        .or_else(|| val.get("name"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        return Some(name.trim().to_string());
    }
    if let Some(thinking) = val.get("thinking").and_then(Value::as_str) {
        let snippet = truncate_chars(thinking, 36);
        if !snippet.is_empty() {
            return Some(format!("thinking: {snippet}"));
        }
    }
    None
}

pub(super) fn detect_child_terminal_state(val: &Value) -> Option<(SubagentStatus, String)> {
    let ty = val.get("type").and_then(Value::as_str).unwrap_or("");
    let status = val.get("status").and_then(Value::as_str).unwrap_or("");

    if ty.eq_ignore_ascii_case("ERROR")
        || ty.eq_ignore_ascii_case("FAILED")
        || ty.eq_ignore_ascii_case("TASK_FAILED")
        || status.eq_ignore_ascii_case("ERROR")
        || status.eq_ignore_ascii_case("FAILED")
    {
        let err_act = val
            .get("error")
            .or_else(|| val.get("content"))
            .and_then(Value::as_str)
            .map(|s| truncate_chars(s, 40))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "failed".to_string());
        return Some((SubagentStatus::Failed, err_act));
    }

    if ty.eq_ignore_ascii_case("COMPLETED")
        || ty.eq_ignore_ascii_case("FINAL_RESPONSE")
        || ty.eq_ignore_ascii_case("TURN_COMPLETE")
        || ty.eq_ignore_ascii_case("TASK_COMPLETE")
    {
        return Some((SubagentStatus::Completed, "done".to_string()));
    }

    if ty == "PLANNER_RESPONSE" && status.eq_ignore_ascii_case("DONE") {
        let no_tool_calls = val
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_none_or(|arr| arr.is_empty());
        let has_final_text = val
            .get("response")
            .or_else(|| val.get("content"))
            .or_else(|| val.get("text"))
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty());
        if no_tool_calls && has_final_text {
            return Some((SubagentStatus::Completed, "done".to_string()));
        }
    }

    None
}
