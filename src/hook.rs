//! Claude Code `PostToolUse` hook: `l0-compressor --claude-hook`.
//!
//! Claude Code runs this after every **successful** Bash tool call and passes
//! the call — including the captured output — as JSON on stdin. When the output
//! is long enough to truncate, the hook prints a `PostToolUse` response whose
//! `updatedToolOutput` replaces what Claude reads with the filtered version.
//! Otherwise it prints nothing and Claude reads the original output.
//!
//! Why after the run and not before it: the previous integration was a
//! `PreToolUse` hook that rewrote `cmd` into `l0-compressor … cmd`. Claude Code
//! evaluates permission rules against the *rewritten* command, so every
//! `allow` rule stopped matching, and a `deny`/`ask` rule on the original
//! command (`Bash(git push *)`) no longer matched either — under
//! `bypassPermissions` a denied command ran. Here the command, its permission
//! check and its execution are exactly what they would be without
//! l0-compressor; only the text Claude reads afterwards changes.
//!
//! Limits, by Claude Code's design: a failing command fires
//! `PostToolUseFailure`, whose output cannot be replaced, so failures reach
//! Claude unfiltered; and auto-tuning does not learn from hook runs.
//!
//! Fail-safe: any unexpected input, I/O error or panic produces no output and
//! exit 0, which leaves Claude's view of the command untouched.

use crate::args::Args;
use crate::config::Config;
use crate::filter;
use crate::recovery::Recovery;
use crate::runner;
use crate::telemetry;
use clap::Parser;
use serde_json::{json, Map, Value};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Hook payloads carry at most ~30 KB of output inline (Claude Code persists
/// larger outputs to a file), so anything far beyond that is not a payload
/// this hook understands.
const MAX_HOOK_INPUT_BYTES: u64 = 16 * 1024 * 1024;

/// Largest persisted output the hook will read back from disk.
const MAX_PERSISTED_BYTES: u64 = 64 * 1024 * 1024;

/// The Bash `tool_response` fields this hook knows how to reproduce. A response
/// carrying any other field (a background task id, a return-code
/// interpretation, …) is left alone: rebuilding it without that field would
/// silently drop information Claude relies on.
const KNOWN_RESPONSE_KEYS: &[&str] = &[
    "stdout",
    "stderr",
    "interrupted",
    "isImage",
    "noOutputExpected",
    "persistedOutputPath",
    "persistedOutputSize",
];

/// Marker the CLI writes into everything it truncates; seeing it means the
/// command was already an explicit `l0-compressor …` call.
const ALREADY_FILTERED_MARKER: &str = "... [l0-compressor: ";

/// Paths the hook depends on, resolved from the environment by [`HookEnv::from_process`]
/// and injected so tests can point them at a sandbox.
pub struct HookEnv {
    /// Claude Code's config dir (`$CLAUDE_CONFIG_DIR`, else `~/.claude`). Only
    /// persisted outputs under its `projects/` dir are read back.
    pub claude_config_dir: Option<PathBuf>,
}

impl HookEnv {
    pub fn from_process() -> Self {
        let claude_config_dir = non_empty_env("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| non_empty_env("HOME").map(|h| PathBuf::from(h).join(".claude")));
        HookEnv { claude_config_dir }
    }
}

/// What a successful filtering pass produced: the hook response and the
/// metric describing it.
pub struct HookOutcome {
    pub response: Value,
    pub cmd: String,
    pub args: String,
    pub bytes_raw: usize,
    pub bytes_final: usize,
    pub lines_raw: usize,
    pub lines_final: usize,
    pub duration_ms: u64,
}

/// Entry point for `--claude-hook`. Always returns exit code 0.
pub fn run() -> i32 {
    // A panic must not surface as a hook error in Claude Code's UI: silence the
    // default report, swallow the unwind, and fall through to "no output", i.e.
    // the original output stands.
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(|| {
        if !toggle_enabled() {
            return None;
        }
        let mut buf = Vec::new();
        std::io::stdin()
            .take(MAX_HOOK_INPUT_BYTES + 1)
            .read_to_end(&mut buf)
            .ok()?;
        if buf.len() as u64 > MAX_HOOK_INPUT_BYTES {
            return None;
        }
        let input: Value = serde_json::from_slice(&buf).ok()?;
        respond(&input, &HookEnv::from_process())
    })
    .ok()
    .flatten();

    if let Some(o) = outcome {
        println!("{}", o.response);
        let hash = telemetry::args_hash(&o.args);
        let metric = telemetry::ExecutionMetric::from_run_with_factor(
            telemetry::RunMetrics {
                cmd: &o.cmd,
                args: &o.args,
                bytes_raw: o.bytes_raw,
                bytes_final: o.bytes_final,
                lines_raw: o.lines_raw,
                lines_final: o.lines_final,
                truncated: true,
                strategy: "claude_hook",
                // PostToolUse only fires for calls Claude Code classified as successful.
                exit_code: 0,
                duration_ms: o.duration_ms,
                adaptive_event: None,
                args_hash: Some(&hash),
            },
            4,
        );
        telemetry::append_metric(&metric, true);
    }
    0
}

/// Decide the hook response for one payload. `None` means "print nothing":
/// the payload is not one this hook handles, or filtering would not shorten it.
pub fn respond(input: &Value, env: &HookEnv) -> Option<HookOutcome> {
    if input.get("hook_event_name")?.as_str()? != "PostToolUse"
        || input.get("tool_name")?.as_str()? != "Bash"
    {
        return None;
    }
    let command = input.get("tool_input")?.get("command")?.as_str()?;
    let resp = input.get("tool_response")?.as_object()?;
    if resp
        .keys()
        .any(|k| !KNOWN_RESPONSE_KEYS.contains(&k.as_str()))
    {
        return None;
    }
    if resp.get("interrupted")?.as_bool()? || resp.get("isImage")?.as_bool()? {
        return None;
    }
    let stdout = resp.get("stdout")?.as_str()?;
    let stderr = resp.get("stderr")?.as_str()?;
    // Claude Code merges stderr into stdout; a separate stderr would force us to
    // pick an interleaving, so leave such output alone.
    if !stderr.is_empty() || stdout.contains(ALREADY_FILTERED_MARKER) {
        return None;
    }

    // Outputs over ~30 KB arrive truncated inline, with the full text persisted
    // to a file. Filter the full text, never the inline excerpt: the excerpt is
    // only the head, and its tail is where errors and summaries live.
    let persisted = match resp.get("persistedOutputPath") {
        Some(p) => {
            let path = PathBuf::from(p.as_str()?);
            let size = resp.get("persistedOutputSize")?.as_u64()?;
            let data = read_persisted(&path, size, env.claude_config_dir.as_deref()?)?;
            Some((path, data))
        }
        None => None,
    };
    let text: &[u8] = match &persisted {
        Some((_, data)) => data,
        None => stdout.as_bytes(),
    };

    // Name the command the same way the CLI does for `sh -c '<command>'`
    // (env assignments skipped, secrets redacted), so stats and per-command
    // config line up with explicit invocations.
    let args = Args::try_parse_from(["l0-compressor", "sh", "-c", command]).ok()?;
    let cmd = args.cmd_name();

    let ov = Config::load(true).for_command(&cmd);
    let head = ov.head.unwrap_or(filter::DEFAULT_HEAD);
    let tail = ov.tail.unwrap_or(filter::DEFAULT_TAIL);
    let threshold = ov.threshold.unwrap_or(filter::DEFAULT_THRESHOLD);
    let only_errors = ov.only_errors.unwrap_or(false);
    let squelch = ov.squelch.unwrap_or(true);

    // A persisted output already has a full copy on disk; otherwise keep our own
    // so the omitted lines stay reachable without re-running the command.
    let mut recovery = Recovery::new(persisted.is_none(), &cmd, threshold);
    let Some((result, display_tail)) = runner::filter_captured_success(
        text,
        head,
        tail,
        threshold,
        only_errors,
        squelch,
        &mut recovery,
    ) else {
        let _ = recovery.finalize(false);
        return None;
    };
    if !result.truncated || result.bytes_final >= text.len() {
        let _ = recovery.finalize(false);
        return None;
    }
    let bytes_raw = text.len();
    let full_output = match persisted {
        Some((path, _)) => Some(path),
        None => recovery.finalize(true),
    };

    let duration_ms = input
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut new_stdout = result.output;
    let banner = runner::truncation_banner(
        &new_stdout,
        None,
        duration_ms,
        head,
        display_tail,
        result.lines_raw,
        full_output.as_deref(),
    );
    new_stdout.push_str(&banner);

    let mut updated = Map::new();
    updated.insert("stdout".into(), Value::String(new_stdout.clone()));
    updated.insert("stderr".into(), Value::String(String::new()));
    updated.insert("interrupted".into(), Value::Bool(false));
    updated.insert("isImage".into(), Value::Bool(false));
    if let Some(v) = resp.get("noOutputExpected") {
        updated.insert("noOutputExpected".into(), v.clone());
    }

    Some(HookOutcome {
        response: json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "updatedToolOutput": Value::Object(updated),
            }
        }),
        args: args.cmd_args_string(),
        cmd,
        bytes_raw,
        bytes_final: new_stdout.len(),
        lines_raw: result.lines_raw,
        lines_final: result.lines_final,
        duration_ms,
    })
}

/// Read back an output Claude Code persisted. Accepted only when the path is a
/// regular file (not a symlink) under `<claude_config_dir>/projects/` and its
/// size matches what the payload declares, so the hook never puts an arbitrary
/// file into Claude's context.
fn read_persisted(path: &Path, expected: u64, claude_config_dir: &Path) -> Option<Vec<u8>> {
    if expected > MAX_PERSISTED_BYTES || !path.is_absolute() {
        return None;
    }
    if !std::fs::symlink_metadata(path).ok()?.file_type().is_file() {
        return None;
    }
    let canonical = path.canonicalize().ok()?;
    let root = claude_config_dir.join("projects").canonicalize().ok()?;
    if !canonical.starts_with(&root) {
        return None;
    }
    let mut data = Vec::new();
    std::fs::File::open(&canonical)
        .ok()?
        .take(expected + 1)
        .read_to_end(&mut data)
        .ok()?;
    (data.len() as u64 == expected).then_some(data)
}

/// The runtime on/off switch shared with `claude-hook.sh enable|disable`:
/// `$XDG_CONFIG_HOME/l0-compressor/hook.enabled` (else `~/.config/…`).
fn toggle_enabled() -> bool {
    let base = non_empty_env("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| non_empty_env("HOME").map(|h| PathBuf::from(h).join(".config")));
    base.is_some_and(|b| b.join("l0-compressor").join("hook.enabled").is_file())
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(stdout: &str) -> Value {
        json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "seq 1 500", "description": "x"},
            "tool_response": {
                "stdout": stdout,
                "stderr": "",
                "interrupted": false,
                "isImage": false,
                "noOutputExpected": false
            },
            "duration_ms": 12
        })
    }

    fn lines(n: usize) -> String {
        (1..=n)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn env_none() -> HookEnv {
        HookEnv {
            claude_config_dir: None,
        }
    }

    fn updated(o: &HookOutcome) -> &Map<String, Value> {
        o.response["hookSpecificOutput"]["updatedToolOutput"]
            .as_object()
            .unwrap()
    }

    #[test]
    fn long_output_is_replaced_in_the_documented_shape() {
        let o = respond(&payload(&lines(500)), &env_none()).expect("should filter");
        let hso = &o.response["hookSpecificOutput"];
        assert_eq!(hso["hookEventName"], "PostToolUse");
        let u = updated(&o);
        let mut keys: Vec<_> = u.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "interrupted",
                "isImage",
                "noOutputExpected",
                "stderr",
                "stdout"
            ]
        );
        let out = u["stdout"].as_str().unwrap();
        assert!(out.starts_with("1\n2\n"), "head kept: {out}");
        assert!(out.contains("\n500\n"), "last line kept: {out}");
        assert!(out.contains("lines omitted"), "gap marked: {out}");
        assert!(out.contains("truncated=true"), "banner present: {out}");
        assert!(
            !out.contains("exit_code="),
            "hook must not claim an exit code"
        );
        assert!(out.len() < lines(500).len());
        assert_eq!(o.cmd, "seq");
        assert_eq!(o.lines_raw, 500);
    }

    #[test]
    fn short_output_is_left_alone() {
        assert!(respond(&payload(&lines(20)), &env_none()).is_none());
    }

    #[test]
    fn other_events_and_tools_are_ignored() {
        let mut p = payload(&lines(500));
        p["hook_event_name"] = json!("PreToolUse");
        assert!(respond(&p, &env_none()).is_none());
        let mut p = payload(&lines(500));
        p["hook_event_name"] = json!("PostToolUseFailure");
        assert!(respond(&p, &env_none()).is_none());
        let mut p = payload(&lines(500));
        p["tool_name"] = json!("Read");
        assert!(respond(&p, &env_none()).is_none());
    }

    #[test]
    fn unknown_response_fields_are_left_alone() {
        let mut p = payload(&lines(500));
        p["tool_response"]["backgroundTaskId"] = json!("abc");
        assert!(respond(&p, &env_none()).is_none());
        let mut p = payload(&lines(500));
        p["tool_response"]["returnCodeInterpretation"] = json!("No matches found");
        assert!(respond(&p, &env_none()).is_none());
    }

    #[test]
    fn interrupted_image_or_separate_stderr_are_left_alone() {
        let mut p = payload(&lines(500));
        p["tool_response"]["interrupted"] = json!(true);
        assert!(respond(&p, &env_none()).is_none());
        let mut p = payload(&lines(500));
        p["tool_response"]["isImage"] = json!(true);
        assert!(respond(&p, &env_none()).is_none());
        let mut p = payload(&lines(500));
        p["tool_response"]["stderr"] = json!("warning: x");
        assert!(respond(&p, &env_none()).is_none());
    }

    #[test]
    fn missing_or_mistyped_fields_are_left_alone() {
        let mut p = payload(&lines(500));
        p["tool_response"]
            .as_object_mut()
            .unwrap()
            .remove("interrupted");
        assert!(respond(&p, &env_none()).is_none());
        let mut p = payload(&lines(500));
        p["tool_response"]["stdout"] = json!(42);
        assert!(respond(&p, &env_none()).is_none());
        let mut p = payload(&lines(500));
        p["tool_input"] = json!({});
        assert!(respond(&p, &env_none()).is_none());
        assert!(respond(&json!([]), &env_none()).is_none());
    }

    #[test]
    fn already_filtered_output_is_left_alone() {
        let mut out = lines(500);
        out.push_str("\n... [l0-compressor: exit_code=0, duration=1ms, truncated=true] ...\n");
        assert!(respond(&payload(&out), &env_none()).is_none());
    }

    #[test]
    fn binary_output_is_left_alone() {
        let mut out = lines(500);
        out.insert(3, '\0');
        assert!(respond(&payload(&out), &env_none()).is_none());
    }

    #[test]
    fn secrets_in_the_command_do_not_reach_metrics() {
        let mut p = payload(&lines(500));
        p["tool_input"]["command"] = json!("API_TOKEN=hunter2 seq 1 500");
        let o = respond(&p, &env_none()).expect("should filter");
        assert_eq!(o.cmd, "seq");
        assert!(!o.args.contains("hunter2"), "args: {}", o.args);
    }

    /// A sandboxed `<config>/projects/…/tool-results/x.txt` like the one Claude
    /// Code writes for outputs over ~30 KB.
    fn persisted_fixture(tag: &str, content: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("l0-hook-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base
            .join("projects")
            .join("proj")
            .join("sess")
            .join("tool-results");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("out.txt");
        std::fs::write(&file, content).unwrap();
        (base, file)
    }

    fn persisted_payload(file: &Path, size: u64) -> Value {
        let mut p = payload(&lines(50)); // the inline excerpt: head only
        p["tool_response"]["persistedOutputPath"] = json!(file.to_str().unwrap());
        p["tool_response"]["persistedOutputSize"] = json!(size);
        p
    }

    #[test]
    fn persisted_output_is_filtered_from_the_full_file() {
        let full = lines(20_000);
        let (base, file) = persisted_fixture("full", &full);
        let env = HookEnv {
            claude_config_dir: Some(base.clone()),
        };
        let o = respond(&persisted_payload(&file, full.len() as u64), &env).expect("filter");
        let out = updated(&o)["stdout"].as_str().unwrap();
        // The tail comes from the file, not from the 50-line inline excerpt.
        assert!(out.contains("\n20000\n"), "{out}");
        assert!(
            out.contains(file.to_str().unwrap()),
            "points at full output"
        );
        assert_eq!(o.lines_raw, 20_000);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn persisted_output_with_wrong_size_is_left_alone() {
        let full = lines(20_000);
        let (base, file) = persisted_fixture("size", &full);
        let env = HookEnv {
            claude_config_dir: Some(base.clone()),
        };
        assert!(respond(&persisted_payload(&file, full.len() as u64 - 1), &env).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn persisted_output_outside_claude_projects_is_left_alone() {
        let full = lines(20_000);
        let (base, file) = persisted_fixture("outside", &full);
        // Config dir that does not contain the file.
        let other = std::env::temp_dir().join(format!("l0-hook-other-{}", std::process::id()));
        std::fs::create_dir_all(other.join("projects")).unwrap();
        let env = HookEnv {
            claude_config_dir: Some(other.clone()),
        };
        assert!(respond(&persisted_payload(&file, full.len() as u64), &env).is_none());
        assert!(respond(&persisted_payload(&file, full.len() as u64), &env_none()).is_none());
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[cfg(unix)]
    #[test]
    fn persisted_output_symlink_is_refused() {
        let secret = lines(20_000);
        let (base, target) = persisted_fixture("link", &secret);
        let link = target.with_file_name("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let env = HookEnv {
            claude_config_dir: Some(base.clone()),
        };
        assert!(respond(&persisted_payload(&link, secret.len() as u64), &env).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }
}
