//! CLI argument parsing.
//!
//! Separates proxy flags (`--stats`, `--raw`, `--head N`, `--tail N`, `-i`)
//! from the wrapped command and its arguments.
//! Everything after the first non-flag token is the child command.

use clap::Parser;
use clap_complete::Shell;

/// Lightweight CLI proxy that reduces LLM token consumption.
#[derive(Parser, Debug)]
#[command(
    name = "l0-compressor",
    version,
    long_version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("L0_COMPRESSOR_GIT_HASH"), ")"),
    about = "CLI proxy: filters & truncates command output to save LLM tokens",
    // `trailing_var_arg` is set on the `command` field below; everything after the
    // first non-flag token is the child command.
)]
pub struct Args {
    /// Show aggregated token savings statistics, then exit.
    #[arg(long)]
    pub stats: bool,

    /// Filter stats/discover to entries within this time window (e.g. "7d", "24h").
    #[arg(long)]
    pub since: Option<String>,

    /// Reset (delete) all telemetry statistics.
    #[arg(long)]
    pub reset_stats: bool,

    /// Show an optimization advisory derived from your metrics, then exit.
    #[arg(long)]
    pub discover: bool,

    /// Output `--stats` as JSON instead of the dashboard.
    #[arg(long)]
    pub json: bool,

    /// USD per million tokens; when > 0, show estimated cost saved in `--stats`/`--discover`.
    #[arg(long, default_value_t = 0.0)]
    pub cost_per_mtok: f64,

    /// Run command but print full output without truncation (still logs metrics).
    #[arg(long)]
    pub raw: bool,

    /// Force interactive/passthrough mode (no capture, no metrics).
    #[arg(short = 'i', long = "interactive")]
    pub interactive: bool,

    /// Number of head lines to keep.
    #[arg(long, default_value_t = crate::filter::DEFAULT_HEAD)]
    pub head: usize,

    /// Number of tail lines to keep (120 on error exit).
    #[arg(long, default_value_t = crate::filter::DEFAULT_TAIL)]
    pub tail: usize,

    /// Number of tail lines on error exit.
    #[arg(long, default_value_t = crate::filter::DEFAULT_TAIL_ERROR)]
    pub tail_error: usize,

    /// Line count threshold below which output is never truncated.
    #[arg(long, default_value_t = crate::filter::DEFAULT_THRESHOLD)]
    pub threshold: usize,

    /// Deprecated no-op: auto-tuning is on by default (disable with `--no-auto`).
    /// Kept (hidden) for backward compatibility.
    #[arg(long, hide = true)]
    pub auto: bool,

    /// Disable adaptive auto-tuning of parameters.
    #[arg(long)]
    pub no_auto: bool,

    /// Aggressively filter out lines that don't look like errors (e.g. ERROR, WARN, FAIL).
    #[arg(long)]
    pub only_errors: bool,

    /// Disable the clean-success squelch (the tighter tail used on a zero exit
    /// with no error/warning signal). Failures are never squelched regardless.
    #[arg(long)]
    pub no_squelch: bool,

    /// On a failing command whose output was truncated, save the full output to a
    /// temp file and point to it in the banner (so the agent can read the omitted
    /// lines without re-running). Off by default.
    #[arg(long)]
    pub recover: bool,

    /// Kill the command if it produces no output for this many seconds (prevents interactive deadlocks).
    #[arg(long, default_value_t = 0)]
    pub idle_timeout: u64,

    /// Suppress proxy's own stderr warnings (e.g., auto-tuning notifications).
    #[arg(short, long)]
    pub quiet: bool,

    /// Enable safety guard to block dangerous commands (defaults to auto-detect).
    #[arg(long)]
    pub guard: bool,

    /// Disable safety guard to block dangerous commands.
    #[arg(long)]
    pub no_guard: bool,

    /// Diagnose system installation, shell environment, and active LLM editors.
    #[arg(long)]
    pub doctor: bool,

    /// Run as a Claude Code PostToolUse hook: read the hook payload on stdin and
    /// print a filtered `updatedToolOutput` (or nothing). Installed by claude-hook.sh.
    #[arg(long)]
    pub claude_hook: bool,

    /// Floor for success optimization decay under --auto.
    #[arg(long, default_value_t = 10)]
    pub auto_floor: usize,

    /// Ceiling for failure backoff tail expansion under --auto.
    #[arg(long, default_value_t = 1000)]
    pub auto_ceiling: usize,

    /// Divisor for token estimation (default: 4).
    #[arg(long, default_value_t = 4)]
    pub token_factor: usize,

    /// Generate shell completions and print to stdout (bash, zsh, fish, elvish, powershell).
    #[arg(long, value_name = "SHELL")]
    pub completions: Option<Shell>,

    /// The command and arguments to execute.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

/// Commands that are inherently interactive and should passthrough
/// when stdin is a TTY.
const INTERACTIVE_ALLOWLIST: &[&str] = &[
    "vim", "vi", "nvim", "nano", "emacs", "less", "more", "man", "htop", "top", "ssh", "fzf",
];

impl Args {
    /// Returns true if the command should run in passthrough mode.
    pub fn should_passthrough(&self) -> bool {
        if self.interactive {
            return true;
        }

        // Only passthrough if stdin is a TTY (i.e., human at keyboard, not Claude Code)
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            return false;
        }

        // Check if command binary is in the interactive allowlist
        if let Some(cmd) = self.command.first() {
            let binary = cmd.rsplit('/').next().unwrap_or(cmd);
            INTERACTIVE_ALLOWLIST.contains(&binary)
        } else {
            false
        }
    }

    /// Returns the command binary name (for metrics). Leading `NAME=value`
    /// environment assignments are skipped: `API_KEY=x deploy` must record
    /// cmd "deploy" — recording the assignment leaked the secret VERBATIM
    /// into `metrics.jsonl` and the `--stats` table (the redaction pipeline
    /// only ran on the args field).
    pub fn cmd_name(&self) -> String {
        if let Some(real) = self.extract_real_cmd() {
            return real;
        }
        self.command
            .iter()
            .find(|t| !is_env_assignment(t))
            .map(|s| s.rsplit('/').next().unwrap_or(s).to_string())
            .unwrap_or_else(|| "(none)".to_string())
    }

    /// Try to extract the real command from shell wrappers like sh -c "cmd".
    fn extract_real_cmd(&self) -> Option<String> {
        self.extract_real_cmd_and_args().map(|(cmd, _)| cmd)
    }

    /// Extract the real (command, args) pair from shell wrappers like
    /// `sh -c "cmd args…"`. Both fields come from the SAME layer: metrics
    /// records used to mix them ({"cmd":"exit","args":"-c exit 42"} — inner
    /// cmd, outer args), which also keyed the learner's args_hash buckets on
    /// the wrong string.
    fn extract_real_cmd_and_args(&self) -> Option<(String, String)> {
        let raw_bin = self.command.first()?;
        let bin = raw_bin.rsplit('/').next().unwrap_or(raw_bin);
        let shells = ["sh", "bash", "zsh", "ksh", "csh", "tcsh", "fish", "dash"];
        if !shells.contains(&bin) {
            return None;
        }
        // Find the position of "-c"
        let c_idx = self.command.iter().position(|a| a == "-c")?;
        let cmd_str = self.command.get(c_idx + 1)?;
        let mut rest_str = cmd_str.trim_start();
        if rest_str.is_empty() {
            return None;
        }
        // Skip leading NAME=value assignments inside the script too
        // (`sh -c 'PASSWORD=x deploy'` must not record cmd "PASSWORD=x");
        // they are collected and re-join the args so secret redaction
        // applies to them.
        let mut assignments: Vec<String> = Vec::new();
        loop {
            let tok_end = rest_str.find(char::is_whitespace).unwrap_or(rest_str.len());
            let tok = &rest_str[..tok_end];
            if tok.is_empty() || !is_env_assignment(tok) || tok.contains([';', '&', '|']) {
                break;
            }
            assignments.push(tok.to_string());
            rest_str = rest_str[tok_end..].trim_start();
        }
        if rest_str.is_empty() {
            return None;
        }
        // Find the first word before spaces or operators
        let first_word = rest_str
            .split(|c: char| c.is_whitespace() || c == ';' || c == '&' || c == '|')
            .next()?;
        let parsed = first_word.trim_matches(|c| c == '\'' || c == '"');
        let real_bin = parsed.rsplit('/').next().unwrap_or(parsed);
        if real_bin.is_empty() {
            return None;
        }
        // The rest of the script string is the inner command's args; redact
        // secrets token-wise (assignments included), same as the non-wrapped
        // path.
        let rest = rest_str[first_word.len()..].trim_start();
        let mut tokens = assignments;
        tokens.extend(rest.split_whitespace().map(String::from));
        Some((real_bin.to_string(), redact_secret_args(&tokens)))
    }

    /// Returns the command arguments as a single string (for metrics), with
    /// obvious secrets redacted so they are not persisted to `metrics.jsonl`.
    /// For `sh -c` wrappers this is the INNER command's args, matching what
    /// `cmd_name()` reports as the command. Leading env assignments are part
    /// of the args (not the cmd), so they pass through secret redaction.
    pub fn cmd_args_string(&self) -> String {
        if let Some((_, args)) = self.extract_real_cmd_and_args() {
            return args;
        }
        // Everything except the binary token (assignments + args after it).
        let bin_idx = self.command.iter().position(|t| !is_env_assignment(t));
        let tokens: Vec<String> = self
            .command
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != bin_idx)
            .map(|(_, t)| t.clone())
            .collect();
        if tokens.is_empty() {
            String::new()
        } else {
            redact_secret_args(&tokens)
        }
    }
}

/// Whether a token is a shell `NAME=value` environment assignment that can
/// prefix a command (`API_KEY=x deploy-tool`): a valid identifier before the
/// first `=` (alphanumeric/underscore, not starting with a digit).
fn is_env_assignment(tok: &str) -> bool {
    match tok.find('=') {
        Some(eq) if eq > 0 => {
            let name = &tok[..eq];
            !name.starts_with(|c: char| c.is_ascii_digit())
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    }
}

/// Flag names (with leading dashes stripped) whose value is a credential.
fn is_secret_flag_name(flag: &str) -> bool {
    let f = flag.trim_start_matches('-').to_lowercase();
    matches!(
        f.as_str(),
        "password"
            | "passwd"
            | "token"
            | "secret"
            | "api-key"
            | "apikey"
            | "api_key"
            | "access-token"
            | "auth"
            | "credential"
            | "credentials"
            | "private-key"
            | "secret-key"
            | "bearer"
    )
}

/// Whether an environment variable name holds a credential. Unlike flags,
/// variable names compose freely (`GITHUB_TOKEN`, `AWS_SECRET_ACCESS_KEY`,
/// `DB_PASSWORD`, `PGPASSWORD`), so this matches on name parts rather than the
/// whole name: distinctive words anywhere, short ambiguous ones (`KEY`, `AUTH`,
/// `PASS`) only at the start of an `_`-separated segment.
fn is_secret_env_name(name: &str) -> bool {
    const ANYWHERE: &[&str] = &[
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "BEARER",
    ];
    const SEGMENT_START: &[&str] = &["KEY", "AUTH", "PASS"];
    let n = name.to_ascii_uppercase();
    ANYWHERE.iter().any(|w| n.contains(w))
        || n.split('_')
            .any(|seg| SEGMENT_START.iter().any(|p| seg.starts_with(p)))
}

/// Redact `user:pass` userinfo from a URL argument: `scheme://u:p@host` → `scheme://***@host`.
fn redact_url_userinfo(arg: &str) -> String {
    if let Some(scheme_end) = arg.find("://") {
        let after = &arg[scheme_end + 3..];
        if let Some(at) = after.find('@') {
            if after[..at].contains(':') {
                return format!("{}://***@{}", &arg[..scheme_end], &after[at + 1..]);
            }
        }
    }
    arg.to_string()
}

/// Join command arguments into a metrics string, redacting credential-bearing
/// values: `--password X` / `--password=X`, `-H/--header` values, and URL
/// userinfo. Benign flags (`mkdir -p`, `sort -u`, …) are left untouched.
fn redact_secret_args(args: &[String]) -> String {
    let mut out: Vec<String> = Vec::with_capacity(args.len());
    let mut redact_next = false;
    for arg in args {
        if redact_next {
            out.push("***".to_string());
            redact_next = false;
            continue;
        }
        // `--flag=value` form, and `NAME=value` environment assignments.
        if let Some(eq) = arg.find('=') {
            let name = &arg[..eq];
            if is_secret_flag_name(name) || (is_env_assignment(arg) && is_secret_env_name(name)) {
                out.push(format!("{}=***", name));
                continue;
            }
        }
        // Bare secret flag, or a header flag: redact the following token.
        if is_secret_flag_name(arg) || arg == "-H" || arg == "--header" {
            out.push(arg.clone());
            redact_next = true;
            continue;
        }
        out.push(redact_url_userinfo(arg));
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// sh -c records keep cmd and args on the SAME layer: inner command,
    /// inner args (the old behavior mixed inner cmd with outer args).
    #[test]
    fn sh_c_inner_args_extracted() {
        let args = Args::parse_from(["t", "sh", "-c", "cargo test --all"]);
        assert_eq!(args.cmd_name(), "cargo");
        assert_eq!(args.cmd_args_string(), "test --all");

        let args = Args::parse_from(["t", "sh", "-c", "exit 42"]);
        assert_eq!(args.cmd_name(), "exit");
        assert_eq!(args.cmd_args_string(), "42");
    }

    /// Secret redaction applies to the INNER tokens of sh -c scripts.
    #[test]
    fn sh_c_inner_args_are_redacted() {
        let args = Args::parse_from(["t", "sh", "-c", "curl --token abc123 https://x.test"]);
        assert_eq!(args.cmd_name(), "curl");
        assert_eq!(args.cmd_args_string(), "--token *** https://x.test");
    }

    /// Leading NAME=value assignments are NOT the command — recording them as
    /// cmd leaked secrets verbatim into metrics.jsonl and the --stats table.
    #[test]
    fn env_assignment_prefix_not_recorded_as_cmd() {
        let args = Args::parse_from(["t", "API_KEY=sk-direct789", "deploy-tool", "--go"]);
        assert_eq!(args.cmd_name(), "deploy-tool");
        let rendered = args.cmd_args_string();
        assert!(
            rendered.contains("API_KEY=***"),
            "assignment must be redacted: {rendered}"
        );
        assert!(
            !rendered.contains("sk-direct789"),
            "secret leaked: {rendered}"
        );

        // Same inside an sh -c script.
        let args = Args::parse_from(["t", "sh", "-c", "PASSWORD=topsecret deploy-tool"]);
        assert_eq!(args.cmd_name(), "deploy-tool");
        let rendered = args.cmd_args_string();
        assert!(
            rendered.contains("PASSWORD=***"),
            "inner assignment: {rendered}"
        );
        assert!(!rendered.contains("topsecret"), "secret leaked: {rendered}");

        // Non-secret assignments survive un-redacted; pure-assignment command
        // degrades to "(none)" rather than recording the assignment.
        let args = Args::parse_from(["t", "FOO=bar", "env"]);
        assert_eq!(args.cmd_name(), "env");
        assert_eq!(args.cmd_args_string(), "FOO=bar");
        let args = Args::parse_from(["t", "FOO=bar"]);
        assert_eq!(args.cmd_name(), "(none)");
    }

    /// Credential-bearing variable names are compound (`GITHUB_TOKEN`), not
    /// the bare flag names `is_secret_flag_name` knows; the Claude Code hook
    /// records every command the agent runs, so these must not leak either.
    #[test]
    fn compound_secret_env_names_are_redacted() {
        for (assign, secret) in [
            ("API_TOKEN=hunter2", "hunter2"),
            ("GITHUB_TOKEN=ghp_abc", "ghp_abc"),
            ("AWS_SECRET_ACCESS_KEY=wJalr", "wJalr"),
            ("DB_PASSWORD=pw1", "pw1"),
            ("OPENAI_API_KEY=sk-1", "sk-1"),
            ("PGPASSWORD=pw2", "pw2"),
            ("HTTP_AUTH_HEADER=b64", "b64"),
        ] {
            let a = Args::parse_from(["t", "sh", "-c", &format!("{assign} deploy --go")]);
            let rendered = a.cmd_args_string();
            assert!(!rendered.contains(secret), "{assign} leaked: {rendered}");
            assert_eq!(a.cmd_name(), "deploy");
        }
        // Ordinary assignments stay readable.
        let a = Args::parse_from(["t", "sh", "-c", "RUST_LOG=debug NODE_ENV=test cargo test"]);
        assert_eq!(a.cmd_args_string(), "RUST_LOG=debug NODE_ENV=test test");
    }

    #[test]
    fn is_env_assignment_shape() {
        assert!(is_env_assignment("FOO=bar"));
        assert!(is_env_assignment("API_KEY=x=y"));
        assert!(!is_env_assignment("=bar"));
        assert!(!is_env_assignment("1FOO=bar"));
        assert!(!is_env_assignment("foo-bar=x"));
        assert!(!is_env_assignment("plain"));
        assert!(!is_env_assignment("--flag=value")); // dashes → not an identifier
    }

    #[test]
    fn parse_basic_command() {
        let args = Args::parse_from(["t", "cargo", "test", "--all"]);
        assert_eq!(args.command, vec!["cargo", "test", "--all"]);
        assert!(!args.stats);
        assert!(!args.raw);
        assert!(!args.auto);
    }

    #[test]
    fn parse_stats_flag() {
        let args = Args::parse_from(["t", "--stats"]);
        assert!(args.stats);
        assert!(args.command.is_empty());
    }

    #[test]
    fn parse_raw_with_command() {
        let args = Args::parse_from(["t", "--raw", "git", "log"]);
        assert!(args.raw);
        assert_eq!(args.command, vec!["git", "log"]);
    }

    #[test]
    fn parse_custom_head_tail() {
        let args = Args::parse_from(["t", "--head", "50", "--tail", "50", "ls", "-la"]);
        assert_eq!(args.head, 50);
        assert_eq!(args.tail, 50);
        assert_eq!(args.command, vec!["ls", "-la"]);
    }

    #[test]
    fn cmd_name_extracts_binary() {
        let args = Args::parse_from(["t", "/usr/bin/git", "status"]);
        assert_eq!(args.cmd_name(), "git");
    }

    #[test]
    fn cmd_args_string_joins() {
        let args = Args::parse_from(["t", "cargo", "test", "--all", "--release"]);
        assert_eq!(args.cmd_args_string(), "test --all --release");
    }

    #[test]
    fn parse_interactive_flag() {
        let args = Args::parse_from(["t", "-i", "vim", "file.txt"]);
        assert!(args.interactive);
        assert_eq!(args.command, vec!["vim", "file.txt"]);
    }

    #[test]
    fn parse_interactive_long() {
        let args = Args::parse_from(["t", "--interactive", "htop"]);
        assert!(args.interactive);
        assert_eq!(args.command, vec!["htop"]);
    }

    #[test]
    fn parse_no_command() {
        let args = Args::parse_from(["t"]);
        assert!(args.command.is_empty());
        assert!(!args.stats);
        assert!(!args.raw);
        assert!(!args.interactive);
        assert!(!args.auto);
    }

    #[test]
    fn parse_stats_with_since() {
        let args = Args::parse_from(["t", "--stats", "--since", "7d"]);
        assert!(args.stats);
        assert_eq!(args.since, Some("7d".to_string()));
    }

    #[test]
    fn parse_threshold_override() {
        let args = Args::parse_from(["t", "--threshold", "200", "ls"]);
        assert_eq!(args.threshold, 200);
        assert_eq!(args.command, vec!["ls"]);
    }

    #[test]
    fn parse_tail_error_override() {
        let args = Args::parse_from(["t", "--tail-error", "200", "cargo", "build"]);
        assert_eq!(args.tail_error, 200);
        assert_eq!(args.command, vec!["cargo", "build"]);
    }

    #[test]
    fn parse_defaults() {
        let args = Args::parse_from(["t", "echo"]);
        assert_eq!(args.head, 30);
        assert_eq!(args.tail, 30);
        assert_eq!(args.tail_error, 120);
        assert_eq!(args.threshold, 100);
        assert!(!args.raw);
        assert!(!args.stats);
        assert!(!args.interactive);
        assert!(!args.auto);
        assert_eq!(args.command, vec!["echo"]);
    }

    #[test]
    fn parse_all_flags_combined() {
        let args = Args::parse_from([
            "t",
            "--raw",
            "--head",
            "10",
            "--tail",
            "20",
            "--threshold",
            "50",
            "cargo",
            "test",
        ]);
        assert!(args.raw);
        assert_eq!(args.head, 10);
        assert_eq!(args.tail, 20);
        assert_eq!(args.threshold, 50);
        assert_eq!(args.command, vec!["cargo", "test"]);
    }

    #[test]
    fn cmd_name_no_command() {
        let args = Args::parse_from(["t"]);
        assert_eq!(args.cmd_name(), "(none)");
    }

    #[test]
    fn cmd_name_simple() {
        let args = Args::parse_from(["t", "ls"]);
        assert_eq!(args.cmd_name(), "ls");
    }

    #[test]
    fn cmd_args_string_no_args() {
        let args = Args::parse_from(["t", "ls"]);
        assert_eq!(args.cmd_args_string(), "");
    }

    #[test]
    fn cmd_args_string_redacts_secrets() {
        // --flag=value and --flag value forms of credential flags.
        let a = Args::parse_from(["t", "mysql", "--password=hunter2", "db"]);
        assert_eq!(a.cmd_args_string(), "--password=*** db");
        let b = Args::parse_from(["t", "tool", "--token", "abc123", "go"]);
        assert_eq!(b.cmd_args_string(), "--token *** go");
        // Authorization headers and URL userinfo.
        let c = Args::parse_from(["t", "curl", "-H", "Authorization: Bearer XYZ", "u"]);
        assert_eq!(c.cmd_args_string(), "-H *** u");
        let d = Args::parse_from(["t", "git", "clone", "https://user:pw@example.com/r.git"]);
        assert_eq!(d.cmd_args_string(), "clone https://***@example.com/r.git");
        // Benign overloaded short flags are NOT redacted.
        let e = Args::parse_from(["t", "mkdir", "-p", "a/b/c"]);
        assert_eq!(e.cmd_args_string(), "-p a/b/c");
        let f = Args::parse_from(["t", "sort", "-u", "file.txt"]);
        assert_eq!(f.cmd_args_string(), "-u file.txt");
    }

    #[test]
    fn cmd_args_string_single_arg() {
        let args = Args::parse_from(["t", "git", "status"]);
        assert_eq!(args.cmd_args_string(), "status");
    }

    #[test]
    fn should_passthrough_interactive_flag() {
        // When -i is set, should_passthrough returns true regardless of TTY state
        let args = Args::parse_from(["t", "-i", "echo", "hello"]);
        assert!(args.should_passthrough());
    }

    #[test]
    fn should_passthrough_no_command() {
        // Empty command, not interactive → false
        let args = Args::parse_from(["t"]);
        assert!(!args.should_passthrough());
    }

    #[test]
    fn parse_command_with_dashes() {
        let args = Args::parse_from(["t", "cargo", "test", "--", "--nocapture"]);
        assert_eq!(args.command, vec!["cargo", "test", "--", "--nocapture"]);
    }

    #[test]
    fn parse_raw_flag_alone_no_cmd() {
        let args = Args::parse_from(["t", "--raw"]);
        assert!(args.raw);
        assert!(args.command.is_empty());
    }

    #[test]
    fn parse_auto_flag() {
        let args = Args::parse_from(["t", "--auto", "cargo", "test"]);
        assert!(args.auto);
        assert_eq!(args.command, vec!["cargo", "test"]);
    }

    #[test]
    fn cmd_name_extracts_from_shell_wrapper() {
        let args = Args::parse_from(["t", "sh", "-c", "cargo test --all"]);
        assert_eq!(args.cmd_name(), "cargo");

        let args2 = Args::parse_from(["t", "/bin/bash", "-c", "  git diff && echo 1"]);
        assert_eq!(args2.cmd_name(), "git");
    }

    #[test]
    fn parse_custom_auto_limits_and_token_factor() {
        let args = Args::parse_from([
            "t",
            "--auto",
            "--auto-floor",
            "5",
            "--auto-ceiling",
            "500",
            "--token-factor",
            "8",
            "cargo",
            "test",
        ]);
        assert!(args.auto);
        assert_eq!(args.auto_floor, 5);
        assert_eq!(args.auto_ceiling, 500);
        assert_eq!(args.token_factor, 8);
        assert_eq!(args.command, vec!["cargo", "test"]);
    }

    #[test]
    fn parse_no_auto_and_doctor() {
        let args = Args::parse_from(["t", "--no-auto", "--doctor"]);
        assert!(args.no_auto);
        assert!(args.doctor);
        assert!(args.command.is_empty());
    }
}
