//! `l0-compressor` — Lightweight CLI proxy that reduces LLM token consumption.
//!
//! Usage:
//!   l0-compressor cargo test          # filtered output
//!   l0-compressor --raw cargo test    # full output, still logs metrics
//!   l0-compressor --stats             # show savings report
//!   l0-compressor --stats --since 7d  # last 7 days only
//!   l0-compressor -i vim file.txt     # force passthrough (interactive)

mod args;
mod config;
mod filter;
mod hook;
mod recovery;
mod runner;
mod telemetry;
mod ui;

use args::Args;
use std::io::Write;

fn main() {
    // Parse via ArgMatches so we can tell an explicit CLI flag from a default —
    // this drives config precedence (explicit CLI > config file > built-in default).
    let matches = <Args as clap::CommandFactory>::command().get_matches();
    let args =
        <Args as clap::FromArgMatches>::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let from_cli =
        |id: &str| matches.value_source(id) == Some(clap::parser::ValueSource::CommandLine);

    // ── Shell completions ───────────────────────────────────────────────
    if let Some(shell) = args.completions {
        let mut cmd = <Args as clap::CommandFactory>::command();
        clap_complete::generate(shell, &mut cmd, "l0-compressor", &mut std::io::stdout());
        std::process::exit(0);
    }

    // ── Claude Code hook mode ───────────────────────────────────────────
    // Stdout is the hook response channel: nothing else may print before this.
    if args.claude_hook {
        telemetry::migrate_legacy_data_dir();
        std::process::exit(hook::run());
    }

    // ── One-time data-dir migration from the pre-rename `l0-cache/` location ──
    // Best-effort and non-destructive; keeps a user's metrics/tuning across the
    // rebrand. Runs before any telemetry read (--stats, --reset-stats, run mode).
    telemetry::migrate_legacy_data_dir();

    // ── Reset stats mode ────────────────────────────────────────────────
    if args.reset_stats {
        if let Err(e) = telemetry::reset_stats() {
            eprintln!("l0-compressor: failed to reset stats: {}", e);
            std::process::exit(1);
        }
        println!("l0-compressor: telemetry stats have been successfully reset.");
        std::process::exit(0);
    }

    // ── Validate --since up front (any mode) ────────────────────────────
    // An unparseable window must be an error, not silently-ignored: the old
    // behavior rendered all-time data under a header claiming the window —
    // and in run mode the bad value was swallowed without a sound. Trimmed
    // here so a stray trailing space can't leak into the dashboard header.
    let since = args.since.as_deref().map(str::trim);
    if let Some(s) = since {
        if !telemetry::since_is_valid(s) {
            eprintln!(
                "l0-compressor: error: invalid --since value '{}' (expected <num><unit> with unit one of d/h/m/s, e.g. 7d, 24h, 30m)",
                s
            );
            std::process::exit(2);
        }
        if !args.stats && !args.discover {
            eprintln!(
                "l0-compressor: warning: --since has no effect without --stats or --discover"
            );
        }
    }

    // ── Stats mode ──────────────────────────────────────────────────────
    if args.stats {
        telemetry::print_stats(since, args.json, args.cost_per_mtok);
        std::process::exit(0);
    }

    // ── Discover mode (optimization advisory) ───────────────────────────
    if args.discover {
        telemetry::run_discover(since, args.cost_per_mtok);
        std::process::exit(0);
    }

    // ── Doctor mode ─────────────────────────────────────────────────────
    if args.doctor {
        telemetry::run_doctor();
        std::process::exit(0);
    }

    // ── No command provided ─────────────────────────────────────────────
    if args.command.is_empty() {
        eprintln!("l0-compressor: no command specified. Usage: l0-compressor <command> [args...]");
        eprintln!("   l0-compressor --stats       show token savings report");
        eprintln!("   l0-compressor --help        show all options");
        std::process::exit(1);
    }

    // ── Unknown leading flag ────────────────────────────────────────────
    // `trailing_var_arg + allow_hyphen_values` means clap hands us any
    // unrecognized `--flag` as command[0]. Running it would silently fail
    // (`sh -c "--flag …"` exits 2 with the shell's error on its own, nulled,
    // stderr) and pollute telemetry with a flag-named command. Reject it
    // here, before the guard and before any metric is written.
    if let Some(first) = args.command.first() {
        if first.starts_with('-') {
            eprintln!(
                "l0-compressor: error: unrecognized option '{}' (commands cannot start with '-')",
                first
            );
            eprintln!("   l0-compressor --help        show all options");
            std::process::exit(2);
        }
    }

    // ── Safety Command Guard ────────────────────────────────────────────
    let should_guard = telemetry::guard_enabled(args.guard, args.no_guard);

    if should_guard && !args.command.is_empty() {
        if let Err(reason) = telemetry::check_dangerous_command(&args.cmd_name(), &args.command) {
            eprintln!("\x1b[31m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\x1b[0m");
            eprintln!(
                "\x1b[31;1m● l0-compressor: GUARD BLOCKED A POTENTIALLY DESTRUCTIVE COMMAND\x1b[0m"
            );
            eprintln!("\x1b[31m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\x1b[0m");
            eprintln!("Reason: {}", reason);
            eprintln!(
                "If this is intentional, bypass the guard using the \x1b[33m--no-guard\x1b[0m flag"
            );
            eprintln!("or set the environment variable \x1b[33mL0_COMPRESSOR_GUARD=0\x1b[0m.");
            eprintln!("\x1b[31m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\x1b[0m");
            std::process::exit(126);
        }
    }

    // ── Signal handling ─────────────────────────────────────────────────
    // SIGINT/SIGTERM: Ignored in parent so the child (same process group)
    // receives them from the terminal. We wait for child.wait() to complete
    // and propagate its exit code. This prevents zombie processes.
    //
    // SIGPIPE: Ignored so `l0-compressor cmd | head` doesn't kill us before we can
    // log metrics. We handle BrokenPipe on stdout manually.
    install_signal_handlers();

    // ── Passthrough mode (interactive) ──────────────────────────────────
    if args.should_passthrough() {
        match runner::run_passthrough(&args.command) {
            Ok(code) => std::process::exit(code),
            Err(e) => {
                eprintln!(
                    "l0-compressor: failed to execute '{}': {}",
                    args.cmd_name(),
                    e
                );
                std::process::exit(exec_error_code(&e));
            }
        }
    }

    // ── Capture mode (default) ──────────────────────────────────────────
    // Per-command config (if any) fills in values the user did not set explicitly
    // on the command line; auto-tuning then adjusts from that resolved base.
    let cfg = config::Config::load(args.quiet);
    let ov = cfg.for_command(&args.cmd_name());

    let resolve = |id: &str, cli_val: usize, cfg_val: Option<usize>| {
        if from_cli(id) {
            cli_val
        } else {
            cfg_val.unwrap_or(cli_val)
        }
    };
    let mut head = resolve("head", args.head, ov.head);
    let mut tail = resolve("tail", args.tail, ov.tail);
    let mut tail_error = resolve("tail_error", args.tail_error, ov.tail_error);
    let threshold = resolve("threshold", args.threshold, ov.threshold);
    // Booleans: an explicit CLI flag or the config can turn these on.
    let only_errors = args.only_errors || ov.only_errors.unwrap_or(false);
    let recover = args.recover || ov.recover.unwrap_or(false);
    // Clean-success squelch is on by default; `--no-squelch` or `squelch = false`
    // (CLI wins over config) turns it off for callers who want a fixed tail.
    let squelch = !args.no_squelch && ov.squelch.unwrap_or(true);

    // Tracks which auto-tuning rule branch fired this run (None if disabled or
    // no rule matched), so the metric written below records it for `--stats`.
    let mut adaptive_event: Option<&'static str> = None;
    // Per-bucket key for the learner: derived from the redacted args string
    // exactly once so the value we pass to the learner is the same value we
    // serialize into the metric record.
    let args_str = args.cmd_args_string();
    let bucket_key = telemetry::args_hash(&args_str);
    if !args.no_auto {
        // The pre-seed resolution is the recovery rule's restore target —
        // captured BEFORE the persisted tune overwrites head/tail/tail_error.
        let base = telemetry::BaseParams {
            head,
            tail,
            tail_error,
        };
        // Step 5 — seed from persisted tune. If this bucket has been tuned
        // before, the learner runs against THOSE values, not the CLI defaults.
        // This is what makes the decay/shrink rules COMPOUND across runs
        // ("apprendimento incrementale") instead of resetting every time.
        // Floored values are still bounded by auto_floor / auto_ceiling.
        let mut seeded_by: Option<String> = None;
        if let Some(prior) = telemetry::lookup_tuned(&args.cmd_name(), &bucket_key) {
            head = prior.head;
            tail = prior.tail;
            tail_error = prior.tail_error;
            seeded_by = Some(prior.event);
        }
        let tuned = telemetry::get_adaptive_params(
            &args.cmd_name(),
            &bucket_key,
            head,
            tail,
            tail_error,
            base,
            seeded_by.as_deref(),
            args.auto_floor,
            args.auto_ceiling,
        );
        adaptive_event = tuned.event;
        if tuned.modified {
            if let Some(reason) = &tuned.reason {
                if !args.quiet {
                    eprintln!("l0-compressor: auto-tuning: {}", reason);
                }
            }
            head = tuned.head;
            tail = tuned.tail;
            tail_error = tuned.tail_error;
            // Step 5 — persist this firing so the next invocation of the same
            // bucket starts from here. Only persist on actually-modified
            // params (the Step 0 honesty distinction holds: a no-op trigger
            // does not change state).
            if let Some(event_tag) = tuned.event {
                telemetry::save_tuned(
                    &telemetry::TunedParams {
                        ts: telemetry::rfc3339_now_for_pub(),
                        cmd: args.cmd_name(),
                        args_hash: bucket_key.clone(),
                        head,
                        tail,
                        tail_error,
                        event: event_tag.to_string(),
                    },
                    args.quiet,
                );
            }
        }
    }

    match runner::run_captured(
        &args.command,
        head,
        tail,
        tail_error,
        threshold,
        args.raw,
        only_errors,
        squelch,
        args.idle_timeout,
        recover,
    ) {
        Ok(result) => {
            let mut output_to_write = result.filter_result.output.clone();

            if result.filter_result.truncated && result.strategy == "head_tail" {
                // `display_tail` is the ACTUAL tail shown (success vs error tail,
                // minus any clean-success squelch) — reported by the runner so the
                // banner never overstates what survived. The recovery path is only
                // set on a failing, truncated run with --recover.
                let banner = runner::truncation_banner(
                    &output_to_write,
                    Some(result.exit_code),
                    result.duration_ms,
                    head,
                    result.display_tail,
                    result.filter_result.lines_raw,
                    result.recovery_path.as_deref(),
                );
                output_to_write.push_str(&banner);
            }

            let output_result = write_output(&output_to_write);

            // Log metrics BEFORE exiting (even on BrokenPipe)
            let metric = telemetry::ExecutionMetric::from_run_with_factor(
                telemetry::RunMetrics {
                    cmd: &args.cmd_name(),
                    args: &args_str,
                    bytes_raw: result.filter_result.bytes_raw,
                    bytes_final: result.filter_result.bytes_final,
                    lines_raw: result.filter_result.lines_raw,
                    lines_final: result.filter_result.lines_final,
                    truncated: result.filter_result.truncated,
                    strategy: result.strategy,
                    exit_code: result.exit_code,
                    duration_ms: result.duration_ms,
                    adaptive_event,
                    args_hash: Some(&bucket_key),
                },
                args.token_factor,
            );
            telemetry::append_metric(&metric, args.quiet);

            // If stdout was broken (pipe closed), exit cleanly
            if output_result.is_err() {
                std::process::exit(141); // 128 + SIGPIPE(13)
            }

            // Propagate exit code — CRITICAL
            std::process::exit(result.exit_code);
        }
        Err(e) => {
            eprintln!(
                "l0-compressor: failed to execute '{}': {}",
                args.cmd_name(),
                e
            );
            std::process::exit(exec_error_code(&e));
        }
    }
}

/// Map a spawn/execution I/O error to a POSIX-flavored exit code:
/// 127 when the command (or `/bin/sh`) was not found, 126 for any other failure
/// to execute it. Reserves 127 for its conventional "not found" meaning.
fn exec_error_code(e: &std::io::Error) -> i32 {
    if e.kind() == std::io::ErrorKind::NotFound {
        127
    } else {
        126
    }
}

/// Write output to stdout, handling BrokenPipe gracefully.
fn write_output(output: &str) -> std::io::Result<()> {
    if output.is_empty() {
        return Ok(());
    }

    let stdout = std::io::stdout();
    let mut handle = stdout.lock();

    handle.write_all(output.as_bytes())?;

    // Ensure trailing newline
    if !output.ends_with('\n') {
        handle.write_all(b"\n")?;
    }

    handle.flush()
}

/// Async-signal-safe handler: forward the received signal to the captured
/// child's process group, so the whole subtree terminates. Only loads an atomic
/// and calls `kill`, both of which are async-signal-safe.
///
/// Because the captured child runs in its OWN process group, the controlling
/// terminal no longer delivers SIGINT directly to it — the parent must forward.
/// This also fixes a directed `kill <l0-compressor-pid>` (SIGTERM), which the old
/// `SIG_IGN` swallowed while the child kept running and `child.wait()` blocked.
#[cfg(unix)]
extern "C" fn forward_signal(sig: libc::c_int) {
    let pgid = runner::CHILD_PGID.load(std::sync::atomic::Ordering::SeqCst);
    if pgid > 0 {
        // Negative pid → signal the process group (killpg).
        unsafe {
            libc::kill(-pgid, sig);
        }
    }
    // If no child is running (pgid == 0) we deliberately no-op: l0-compressor itself
    // is mid-spawn or finishing up and should not be torn down here.
}

/// Install signal handlers for clean proxy behavior.
///
/// - SIGINT (Ctrl-C) / SIGTERM: forwarded to the child's process group (above).
/// - SIGPIPE: Ignored so we can handle BrokenPipe in code and still log metrics.
#[cfg(unix)]
fn install_signal_handlers() {
    let handler = forward_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    unsafe {
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

#[cfg(not(unix))]
fn install_signal_handlers() {
    // On non-Unix platforms, signals are handled differently.
    // The child process will still receive Ctrl-C via the console.
}
