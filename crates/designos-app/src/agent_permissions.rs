//! Agent permission modes (`Skip` vs `Ask`), session hook generators, file-backed
//! approval bridge (`<session_dir>/approvals/`), and unfocused-window OS notifications (`#656`).

use crate::comments::settings;
use crate::terminal::AgentCliPreset;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const SETTING_KEY: &str = "agent_permission_mode";
pub const APPROVAL_HOOK_TIMEOUT_SECS: u64 = 300;
pub const MAX_INPUT_SUMMARY_CHARS: usize = 200;

/// Global agent permission mode persisted in `<library>/settings.json` under `"agent_permission_mode"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AgentPermissionMode {
    #[default]
    Skip,
    Ask,
}

impl AgentPermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Skip => "skip",
            Self::Ask => "ask",
        }
    }

    pub fn parse(raw: &str) -> Self {
        if raw.trim().eq_ignore_ascii_case("ask") {
            Self::Ask
        } else {
            Self::Skip
        }
    }
}

/// Reads `"agent_permission_mode"` from `<root>/settings.json`, defaulting to [`AgentPermissionMode::Skip`].
pub fn load_permission_mode(root: &Path) -> AgentPermissionMode {
    settings::load_str(root, SETTING_KEY).map(|s| AgentPermissionMode::parse(&s)).unwrap_or_default()
}

/// Persists `"agent_permission_mode"` (`"skip"` or `"ask"`) to `<root>/settings.json`.
pub fn save_permission_mode(root: &Path, mode: AgentPermissionMode) -> Result<(), String> {
    settings::save_str(root, SETTING_KEY, mode.as_str())
}

/// Strips the auto-approve / skip-permissions flag from a CLI command string when in `Ask` mode.
pub fn strip_skip_permission_flags(cmd: &str) -> String {
    cmd.replace(" --dangerously-skip-permissions", "").replace(" --dangerously-bypass-approvals-and-sandbox", "").replace(" --always-approve", "")
}

/// Interactive CLI launch command for `preset` under `mode`.
pub fn cli_launch_command_for_mode(preset: AgentCliPreset, mode: AgentPermissionMode) -> &'static str {
    match mode {
        AgentPermissionMode::Skip => preset.cli_launch_command(),
        AgentPermissionMode::Ask => match preset {
            AgentCliPreset::Claude => {
                "claude --mcp-config \"$DESIGNOS_MCP_CONFIG\" --settings \"$DESIGNOS_SESSION_DIR/hooks.json\" --append-system-prompt \"You are inside DESIGN:OS Studio bound exclusively to file '$DESIGNOS_DOC_NAME' (session $DESIGNOS_SESSION_DIR). Always read $DESIGNOS_CONTEXT_JSON for the current selected page (current_page_id / current_page_name) and deliver all designs onto that file and page via designos-mcp tools (designos_apply_spec, designos_create_node, designos_update_node) or by writing NodeSpec JSON to $DESIGNOS_LIVE_SPEC.\""
            }
            AgentCliPreset::Codex => concat!(
                "codex",
                " -c 'mcp_servers.designos.command=\"'\"$DESIGNOS_MCP_BIN\"'\"'",
                " -c 'mcp_servers.designos.args=[\"--workspace\",\"'\"$DESIGNOS_WORKSPACE\"'\"]'",
                " -c 'mcp_servers.designos.env_vars=[\"DESIGNOS_SESSION_DIR\",\"DESIGNOS_LIVE_CANVAS\",\"DESIGNOS_LIVE_DOC_SYNC\",",
                "\"DESIGNOS_LIVE_DOC_PATCH\",\"DESIGNOS_LIVE_SPEC\",\"DESIGNOS_CONTEXT_JSON\",\"DESIGNOS_TARGET_PAGE_ID\",",
                "\"DESIGNOS_TARGET_PAGE_NAME\",\"DESIGNOS_DOC_NAME\",\"DESIGNOS_FILE\"]'"
            ),
            AgentCliPreset::Grok => "grok --cwd \"$DESIGNOS_SESSION_DIR\"",
            AgentCliPreset::Antigravity => "agy --add-dir \"$DESIGNOS_SESSION_DIR\"",
            AgentCliPreset::Shell | AgentCliPreset::DesignOsHelp => "",
        },
    }
}

/// Mode-aware wrapper around [`crate::terminal::build_real_cli_command`].
pub fn build_real_cli_command_for_mode(preset: AgentCliPreset, mode: AgentPermissionMode, prompt: &str, live_spec: &Path, selection_json: &Path) -> String {
    let base = crate::terminal::build_real_cli_command(preset, prompt, live_spec, selection_json);
    match mode {
        AgentPermissionMode::Skip => base,
        AgentPermissionMode::Ask => strip_skip_permission_flags(&base),
    }
}

/// Mode-aware wrapper around [`crate::agent_sessions::build_new_session_command`].
pub fn build_new_session_command_for_mode(preset: AgentCliPreset, mode: AgentPermissionMode, session_uuid: Option<&str>) -> String {
    let base = crate::agent_sessions::build_new_session_command(preset, session_uuid);
    match mode {
        AgentPermissionMode::Skip => base,
        AgentPermissionMode::Ask => strip_skip_permission_flags(&base),
    }
}

/// Mode-aware wrapper around [`crate::agent_sessions::build_resume_command`].
pub fn build_resume_command_for_mode(entry: &crate::agent_sessions::SessionEntry, agy_cwd_scoped: bool, mode: AgentPermissionMode) -> Option<String> {
    let base = crate::agent_sessions::build_resume_command(entry, agy_cwd_scoped)?;
    Some(match mode {
        AgentPermissionMode::Skip => base,
        AgentPermissionMode::Ask => strip_skip_permission_flags(&base),
    })
}

/// Generates the POSIX `sh` script written to `<session_dir>/approve-hook.sh` for Claude Code's
/// `PermissionRequest` hook in `Ask` mode.
pub fn approval_hook_script(session_dir: &Path) -> String {
    let session_dir_escaped = session_dir.display().to_string().replace('\'', "'\\''");
    format!(
        r#"#!/bin/sh
set -eu

DEFAULT_SESSION_DIR='{session_dir_escaped}'
SESSION_DIR="${{DESIGNOS_SESSION_DIR:-$DEFAULT_SESSION_DIR}}"
APPROVALS_DIR="$SESSION_DIR/approvals"
STATUS_FILE="$SESSION_DIR/agent-status.json"

mkdir -p "$APPROVALS_DIR"
printf '{{"state":"waiting"}}\n' > "$STATUS_FILE" 2>/dev/null || true

REQ_ID="${{DESIGNOS_APPROVAL_REQUEST_ID:-req-$$-$(date +%s)}}"
REQ_TMP="$APPROVALS_DIR/$REQ_ID.request.tmp"
REQ_FILE="$APPROVALS_DIR/$REQ_ID.request.json"
ANS_FILE="$APPROVALS_DIR/$REQ_ID.answer"
NOW_TS="$(date +%s)"

awk -v req_id="$REQ_ID" -v ts="$NOW_TS" '
{{
    buf = buf " " $0
}}
END {{
    gsub(/[\r\n\t]+/, " ", buf)
    tool = "tool"
    if (match(buf, /"tool_name"[[:space:]]*:[[:space:]]*"[^"]*"/)) {{
        s = substr(buf, RSTART, RLENGTH)
        sub(/^"tool_name"[[:space:]]*:[[:space:]]*"/, "", s)
        sub(/"$/, "", s)
        if (length(s) > 0) tool = s
    }}
    summary = ""
    if (match(buf, /"command"[[:space:]]*:[[:space:]]*"[^"]*"/)) {{
        s = substr(buf, RSTART, RLENGTH)
        sub(/^"command"[[:space:]]*:[[:space:]]*"/, "", s)
        sub(/"$/, "", s)
        summary = s
    }} else if (match(buf, /"file_path"[[:space:]]*:[[:space:]]*"[^"]*"/)) {{
        s = substr(buf, RSTART, RLENGTH)
        sub(/^"file_path"[[:space:]]*:[[:space:]]*"/, "", s)
        sub(/"$/, "", s)
        summary = s
    }} else if (match(buf, /"path"[[:space:]]*:[[:space:]]*"[^"]*"/)) {{
        s = substr(buf, RSTART, RLENGTH)
        sub(/^"path"[[:space:]]*:[[:space:]]*"/, "", s)
        sub(/"$/, "", s)
        summary = s
    }} else if (match(buf, /"pattern"[[:space:]]*:[[:space:]]*"[^"]*"/)) {{
        s = substr(buf, RSTART, RLENGTH)
        sub(/^"pattern"[[:space:]]*:[[:space:]]*"/, "", s)
        sub(/"$/, "", s)
        summary = s
    }}
    if (length(summary) == 0) {{
        summary = tool
    }}
    gsub(/["\\]/, "", tool)
    gsub(/["\\]/, "", summary)
    if (length(summary) > 200) {{
        summary = substr(summary, 1, 200)
    }}
    printf "{{\"id\":\"%s\",\"tool_name\":\"%s\",\"input_summary\":\"%s\",\"timestamp\":%s}}\n", req_id, tool, summary, ts
}}
' > "$REQ_TMP"

mv -f "$REQ_TMP" "$REQ_FILE"

TIMEOUT_MS="${{DESIGNOS_APPROVAL_TIMEOUT_MS:-300000}}"
ELAPSED_MS=0
while [ "$ELAPSED_MS" -lt "$TIMEOUT_MS" ]; do
    if [ -f "$ANS_FILE" ]; then
        DECISION="$(tr -d ' \t\r\n' < "$ANS_FILE" 2>/dev/null || true)"
        rm -f "$REQ_FILE" "$ANS_FILE"
        case "$DECISION" in
            allow)
                printf '{{"hookSpecificOutput":{{"hookEventName":"PermissionRequest","decision":{{"behavior":"allow"}}}}}}\n'
                exit 0
                ;;
            deny)
                printf '{{"hookSpecificOutput":{{"hookEventName":"PermissionRequest","decision":{{"behavior":"deny","message":"Denied in design:os Studio"}}}}}}\n'
                exit 0
                ;;
            *)
                exit 0
                ;;
        esac
    fi
    sleep 0.1
    ELAPSED_MS=$((ELAPSED_MS + 100))
done

exit 0
"#
    )
}

/// Writes `<session_dir>/approve-hook.sh` and marks it executable on Unix.
pub fn write_approval_hook_script(session_dir: &Path) -> PathBuf {
    let _ = std::fs::create_dir_all(session_dir);
    let script_path = session_dir.join("approve-hook.sh");
    let content = approval_hook_script(session_dir);
    let _ = designos_library::write_atomic(&script_path, content.as_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&script_path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&script_path, perms);
        }
    }
    script_path
}

fn status_hook_cmd(status_path: &Path, state: &str) -> String {
    let p = status_path.display().to_string().replace('\'', "'\\''");
    format!("printf '{{\"state\":\"{state}\"}}\\n' > '{p}'")
}

fn command_group(command: String) -> serde_json::Value {
    serde_json::json!([
        {
            "matcher": "*",
            "hooks": [
                {
                    "type": "command",
                    "command": command,
                }
            ]
        }
    ])
}

/// Builds Claude Code's `<session_dir>/hooks.json` for `mode`.
/// In `Ask` mode, `PermissionRequest` runs `approve-hook.sh` with `timeout: 300` and never sets `permissions.allow`.
pub fn claude_session_hooks_settings(
    status_path: &Path,
    context_reply: Option<&Path>,
    mode: AgentPermissionMode,
    approval_script_path: Option<&Path>,
) -> serde_json::Value {
    let prompt_cmd = match context_reply {
        Some(reply) => {
            let r = reply.display().to_string().replace('\'', "'\\''");
            format!("{}; cat '{r}'", status_hook_cmd(status_path, "working"))
        }
        None => status_hook_cmd(status_path, "working"),
    };
    let permission_request = match (mode, approval_script_path) {
        (AgentPermissionMode::Ask, Some(script)) => {
            let s = script.display().to_string().replace('\'', "'\\''");
            serde_json::json!([
                {
                    "matcher": "*",
                    "hooks": [
                        {
                            "type": "command",
                            "command": format!("sh '{s}'"),
                            "timeout": APPROVAL_HOOK_TIMEOUT_SECS,
                        }
                    ]
                }
            ])
        }
        _ => command_group(status_hook_cmd(status_path, "waiting")),
    };
    serde_json::json!({
        "hooks": {
            "UserPromptSubmit": command_group(prompt_cmd),
            "PreToolUse": command_group(status_hook_cmd(status_path, "working")),
            "PostToolUse": command_group(status_hook_cmd(status_path, "working")),
            "PermissionRequest": permission_request,
            "Stop": command_group(status_hook_cmd(status_path, "done")),
        }
    })
}

/// Builds Grok's `<session_dir>/.grok/hooks/designos-status.json` for `mode`.
/// In `Ask` mode, adds a passive `Notification` (`permission_prompt`) hook writing `waiting` to `agent-status.json`.
pub fn grok_session_hooks_settings(status_path: &Path, mode: AgentPermissionMode) -> serde_json::Value {
    let mut hooks = serde_json::Map::new();
    hooks.insert("UserPromptSubmit".into(), command_group(status_hook_cmd(status_path, "working")));
    hooks.insert("PreToolUse".into(), command_group(status_hook_cmd(status_path, "working")));
    hooks.insert("PostToolUse".into(), command_group(status_hook_cmd(status_path, "working")));
    hooks.insert("PermissionRequest".into(), command_group(status_hook_cmd(status_path, "waiting")));
    hooks.insert("Stop".into(), command_group(status_hook_cmd(status_path, "done")));
    if mode == AgentPermissionMode::Ask {
        hooks.insert(
            "Notification".into(),
            serde_json::json!([
                {
                    "matcher": "permission_prompt",
                    "hooks": [
                        {
                            "type": "command",
                            "command": status_hook_cmd(status_path, "waiting"),
                        }
                    ]
                }
            ]),
        );
    }
    serde_json::json!({ "hooks": hooks })
}

/// One pending tool-approval request in `<session_dir>/approvals/<id>.request.json`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub tool_name: String,
    #[serde(alias = "summary")]
    pub input_summary: String,
    pub timestamp: u64,
}

/// Returns `<session_dir>/approvals`.
pub fn approvals_dir(session_dir: &Path) -> PathBuf {
    session_dir.join("approvals")
}

/// Returns `<session_dir>/approve-hook.sh`.
pub fn approval_hook_script_path(session_dir: &Path) -> PathBuf {
    session_dir.join("approve-hook.sh")
}

/// Cleans up stale approval requests older than [`APPROVAL_HOOK_TIMEOUT_SECS`].
pub fn cleanup_stale_approvals(session_dir: &Path, now_secs: u64) {
    let _ = poll_approvals_dir(session_dir, now_secs, APPROVAL_HOOK_TIMEOUT_SECS);
}

/// Decision written by Studio to `<session_dir>/approvals/<id>.answer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalDecision {
    Allow,
    Deny,
    Withdrawn,
}

impl ApprovalDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Withdrawn => "withdrawn",
        }
    }
}

/// Summarizes a `PermissionRequest`'s `tool_input` to at most [`MAX_INPUT_SUMMARY_CHARS`] characters,
/// never including file bodies (`content`, `new_string`, `old_string`).
pub fn summarize_tool_input(tool_name: &str, tool_input: &serde_json::Value) -> String {
    let raw = tool_input
        .get("command")
        .or_else(|| tool_input.get("file_path"))
        .or_else(|| tool_input.get("path"))
        .or_else(|| tool_input.get("pattern"))
        .and_then(|v| v.as_str())
        .unwrap_or(tool_name);
    let cleaned: String = raw.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let trimmed = cleaned.trim();
    let fallback = if trimmed.is_empty() { tool_name } else { trimmed };
    fallback.chars().take(MAX_INPUT_SUMMARY_CHARS).collect()
}

/// Polls `<session_dir>/approvals/*.request.json`, cleaning up any stale request files older than
/// `timeout_secs` and returning active requests ordered oldest-first.
pub fn poll_approvals_dir(session_dir: &Path, now_secs: u64, timeout_secs: u64) -> Vec<ApprovalRequest> {
    let approvals_dir = session_dir.join("approvals");
    let Ok(entries) = std::fs::read_dir(&approvals_dir) else {
        return Vec::new();
    };
    let mut requests = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".request.json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut req) = serde_json::from_str::<ApprovalRequest>(&raw) else {
            continue;
        };
        let ans = approvals_dir.join(format!("{}.answer", req.id));
        if req.timestamp > 0 && now_secs.saturating_sub(req.timestamp) > timeout_secs {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(&ans);
            continue;
        }
        if ans.exists() {
            continue;
        }
        if req.input_summary.chars().count() > MAX_INPUT_SUMMARY_CHARS {
            req.input_summary = req.input_summary.chars().take(MAX_INPUT_SUMMARY_CHARS).collect();
        }
        requests.push(req);
    }
    requests.sort_by(|a, b| (a.timestamp, &a.id).cmp(&(b.timestamp, &b.id)));
    requests
}

/// Writes `<session_dir>/approvals/<id>.answer` (`allow`, `deny`, or `withdrawn`).
pub fn write_approval_answer(session_dir: &Path, id: &str, decision: ApprovalDecision) -> std::io::Result<()> {
    let approvals_dir = session_dir.join("approvals");
    std::fs::create_dir_all(&approvals_dir)?;
    let ans_path = approvals_dir.join(format!("{id}.answer"));
    designos_library::write_atomic(&ans_path, decision.as_str().as_bytes()).map_err(|e| std::io::Error::other(e.to_string()))
}

/// Writes `withdrawn` to `<id>.answer` for every pending `<id>.request.json` in `<session_dir>/approvals/`.
pub fn withdraw_pending_requests(session_dir: &Path) {
    let approvals_dir = session_dir.join("approvals");
    let Ok(entries) = std::fs::read_dir(&approvals_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(id) = name.strip_suffix(".request.json") {
            let _ = write_approval_answer(session_dir, id, ApprovalDecision::Withdrawn);
        }
    }
}

/// Abstraction over OS notifications when a tab transitions into `Needs approval` while Studio is unfocused.
pub trait ApprovalNotifier: Send + Sync {
    fn notify_needs_approval(&self, cli_label: &str, doc_name: &str);
}

/// Production notifier using `osascript` on macOS and `notify-send` on Linux, passing strings only via `argv`.
#[derive(Default)]
pub struct OsApprovalNotifier;

impl ApprovalNotifier for OsApprovalNotifier {
    fn notify_needs_approval(&self, cli_label: &str, doc_name: &str) {
        let title = "design:os Studio";
        let body = format!("{cli_label} needs approval — {doc_name}");
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("osascript")
                .args(["-e", "on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run", "--", title, &body])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("notify-send")
                .args([title, &body])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = (title, body);
        }
    }
}

/// Test double recording `(cli_label, doc_name)` notifications without spawning external commands.
#[derive(Clone, Default)]
pub struct RecordingNotifier {
    pub events: Arc<Mutex<Vec<String>>>,
}

impl RecordingNotifier {
    pub fn recorded_events(&self) -> Vec<String> {
        self.events.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

impl ApprovalNotifier for RecordingNotifier {
    fn notify_needs_approval(&self, cli_label: &str, doc_name: &str) {
        if let Ok(mut guard) = self.events.lock() {
            guard.push(format!("{cli_label} needs approval — {doc_name}"));
        }
    }
}

/// Latch ensuring at most one notification fires per pending request while the Studio window is unfocused.
#[derive(Default)]
pub struct NotificationLatch {
    fired_keys: BTreeSet<String>,
}

impl NotificationLatch {
    pub fn reset(&mut self) {
        self.fired_keys.clear();
    }

    pub fn on_window_focus(&mut self) {
        self.fired_keys.clear();
    }

    pub fn clear_key(&mut self, key: &str) {
        self.fired_keys.remove(key);
    }

    /// Fires `notifier` once per `request_key` when `focused == Some(false)`.
    /// Clears the latch when the window is focused (`focused.unwrap_or(true)`) or when no request is pending.
    pub fn update(&mut self, notifier: &dyn ApprovalNotifier, focused: Option<bool>, cli_label: &str, doc_name: &str, request_key: Option<&str>) {
        if focused.unwrap_or(true) {
            self.fired_keys.clear();
            return;
        }
        let Some(key) = request_key else {
            self.fired_keys.clear();
            return;
        };
        if self.fired_keys.insert(key.to_string()) {
            notifier.notify_needs_approval(cli_label, doc_name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::AgentCliPreset;
    use std::io::Write;
    use std::process::{Command, Stdio};

    #[test]
    fn default_mode_is_skip() {
        let tmp = std::env::temp_dir().join(format!("designos-656-default-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        assert_eq!(load_permission_mode(&tmp), AgentPermissionMode::Skip);
        save_permission_mode(&tmp, AgentPermissionMode::Ask).unwrap();
        assert_eq!(load_permission_mode(&tmp), AgentPermissionMode::Ask);
        save_permission_mode(&tmp, AgentPermissionMode::Skip).unwrap();
        assert_eq!(load_permission_mode(&tmp), AgentPermissionMode::Skip);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn skip_mode_launch_commands_unchanged() {
        for preset in [AgentCliPreset::Claude, AgentCliPreset::Codex, AgentCliPreset::Grok, AgentCliPreset::Antigravity] {
            assert_eq!(cli_launch_command_for_mode(preset, AgentPermissionMode::Skip), preset.cli_launch_command());
        }
    }

    #[test]
    fn ask_mode_drops_each_skip_flag() {
        for preset in [AgentCliPreset::Claude, AgentCliPreset::Codex, AgentCliPreset::Grok, AgentCliPreset::Antigravity] {
            let cmd = cli_launch_command_for_mode(preset, AgentPermissionMode::Ask);
            assert!(!cmd.contains("--dangerously-skip-permissions"), "{preset:?} ask command kept skip flag: {cmd}");
            assert!(!cmd.contains("--dangerously-bypass-approvals-and-sandbox"), "{preset:?} ask command kept bypass flag: {cmd}");
            assert!(!cmd.contains("--always-approve"), "{preset:?} ask command kept always-approve flag: {cmd}");
            assert!(!cmd.contains("--dangerously-bypass-hook-trust"), "{preset:?} ask command must never bypass hook trust: {cmd}");
            assert_eq!(cmd, strip_skip_permission_flags(preset.cli_launch_command()));
        }
        assert!(cli_launch_command_for_mode(AgentCliPreset::Claude, AgentPermissionMode::Ask).starts_with("claude --mcp-config"));
        assert!(cli_launch_command_for_mode(AgentCliPreset::Codex, AgentPermissionMode::Ask).starts_with("codex -c"));
        assert!(cli_launch_command_for_mode(AgentCliPreset::Grok, AgentPermissionMode::Ask).starts_with("grok --cwd"));
        assert!(cli_launch_command_for_mode(AgentCliPreset::Antigravity, AgentPermissionMode::Ask).starts_with("agy --add-dir"));
    }

    #[test]
    fn ask_mode_session_settings_have_the_permission_request_hook_with_a_timeout() {
        let status = PathBuf::from("/tmp/designos-656/agent-status.json");
        let reply = PathBuf::from("/tmp/designos-656/page-context-reply.json");
        let script = PathBuf::from("/tmp/designos-656/approve-hook.sh");
        let val = claude_session_hooks_settings(&status, Some(&reply), AgentPermissionMode::Ask, Some(&script));
        assert!(val.get("permissions").is_none(), "Ask mode hooks.json must NOT set permissions.allow: {val}");
        let pr = val["hooks"]["PermissionRequest"].as_array().expect("PermissionRequest array");
        assert_eq!(pr[0]["hooks"][0]["timeout"].as_u64(), Some(APPROVAL_HOOK_TIMEOUT_SECS));
        assert!(pr[0]["hooks"][0]["command"].as_str().unwrap_or_default().contains("approve-hook.sh"));
    }

    #[test]
    fn hook_script_allow_prints_the_documented_decision() {
        let tmp = std::env::temp_dir().join(format!("designos-656-hook-allow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let script = write_approval_hook_script(&tmp);

        let mut child = Command::new("sh").arg(&script).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("spawn sh approve-hook.sh");
        let payload = serde_json::json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test -p designos-app" }
        });
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let req_id = loop {
            let reqs = poll_approvals_dir(&tmp, crate::agent_sessions::unix_timestamp_secs(), APPROVAL_HOOK_TIMEOUT_SECS);
            if let Some(first) = reqs.first() {
                break first.id.clone();
            }
            assert!(std::time::Instant::now() < deadline, "Timed out waiting for request file");
            std::thread::sleep(std::time::Duration::from_millis(25));
        };
        write_approval_answer(&tmp, &req_id, ApprovalDecision::Allow).unwrap();

        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON decision");
        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PermissionRequest");
        assert_eq!(parsed["hookSpecificOutput"]["decision"]["behavior"], "allow");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn hook_script_deny_carries_a_message() {
        let tmp = std::env::temp_dir().join(format!("designos-656-hook-deny-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let script = write_approval_hook_script(&tmp);

        let mut child = Command::new("sh").arg(&script).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("spawn sh approve-hook.sh");
        let payload = serde_json::json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "Write",
            "tool_input": { "file_path": "/etc/hosts", "content": "secret" }
        });
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let req_id = loop {
            let reqs = poll_approvals_dir(&tmp, crate::agent_sessions::unix_timestamp_secs(), APPROVAL_HOOK_TIMEOUT_SECS);
            if let Some(first) = reqs.first() {
                break first.id.clone();
            }
            assert!(std::time::Instant::now() < deadline, "Timed out waiting for request file");
            std::thread::sleep(std::time::Duration::from_millis(25));
        };
        write_approval_answer(&tmp, &req_id, ApprovalDecision::Deny).unwrap();

        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON decision");
        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PermissionRequest");
        assert_eq!(parsed["hookSpecificOutput"]["decision"]["behavior"], "deny");
        assert!(!parsed["hookSpecificOutput"]["decision"]["message"].as_str().unwrap_or_default().is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn hook_script_timeout_prints_nothing() {
        let tmp = std::env::temp_dir().join(format!("designos-656-hook-timeout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let script = write_approval_hook_script(&tmp);

        let mut child = Command::new("sh")
            .arg(&script)
            .env("DESIGNOS_APPROVAL_TIMEOUT_MS", "300")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn sh approve-hook.sh");
        let payload = serde_json::json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "Bash",
            "tool_input": { "command": "echo hi" }
        });
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();

        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        assert!(out.stdout.is_empty(), "Timeout must print nothing to stdout: {:?}", String::from_utf8_lossy(&out.stdout));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn request_file_holds_only_a_summary() {
        let long_cmd = "x".repeat(5000);
        let input = serde_json::json!({
            "command": long_cmd,
            "content": "SUPER_SECRET_FILE_BODY",
            "new_string": "SUPER_SECRET_NEW_STRING"
        });
        let summary = summarize_tool_input("Bash", &input);
        assert_eq!(summary.chars().count(), MAX_INPUT_SUMMARY_CHARS);
        assert!(!summary.contains("SUPER_SECRET"));
    }
}
