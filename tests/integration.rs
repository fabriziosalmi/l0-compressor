// The E2E suite drives the binary through `sh -c` mock scripts and POSIX
// helpers throughout; on Windows the unit-test suite plus the CI smoke run
// cover the native path.
#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

fn get_t_bin() -> PathBuf {
    let mut exe = std::env::current_exe().expect("failed to get current exe");
    // target/debug/deps/integration-...
    exe.pop(); // pop filename
    if exe.file_name().and_then(|s| s.to_str()) == Some("deps") {
        exe.pop(); // pop deps
    }
    exe.push("l0-compressor");
    exe
}

fn wait_timeout(mut child: Child, timeout: Duration) -> Result<ExitStatus, String> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    return Err("Timeout exceeded".to_string());
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

#[test]
fn binary_output_does_not_hang() {
    let t_bin = get_t_bin();
    let xdg = temp_xdg("dd-hang");
    let child = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .args(["dd", "if=/dev/urandom", "bs=1M", "count=1"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn l0-compressor");

    // Wait with a 5 second timeout
    let _status = wait_timeout(child, Duration::from_secs(5)).expect("dd command hung!");
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn exit_code_propagation() {
    let t_bin = get_t_bin();
    let xdg = temp_xdg("exit-code");
    let child = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .args(["sh", "-c", "exit 42"])
        .spawn()
        .expect("failed to spawn l0-compressor");
    let status = wait_timeout(child, Duration::from_secs(5)).unwrap();
    assert_eq!(status.code(), Some(42));
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn pipe_to_head() {
    let t_bin = get_t_bin();
    let t_bin_str = t_bin.to_str().expect("valid path");
    let xdg = temp_xdg("pipe-head");
    let child = Command::new("sh")
        .arg("-c")
        .arg(format!("'{}' seq 1 10000 | head -5", t_bin_str))
        .env("XDG_DATA_HOME", &xdg)
        .spawn()
        .expect("failed to spawn shell");
    let status = wait_timeout(child, Duration::from_secs(5)).unwrap();
    assert!(status.success() || status.code() == Some(141));
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn integration_banner_appears_when_truncated() {
    let t_bin = get_t_bin();
    // Isolate metrics + disable auto-tuning so head/tail are exactly 5/5 regardless
    // of any shared history or parallel `seq` runs.
    let xdg = temp_xdg("banner-trunc");
    let output = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .args([
            "--no-auto",
            "--head",
            "5",
            "--tail",
            "5",
            "--threshold",
            "10",
            "seq",
            "1",
            "100",
        ])
        .output()
        .expect("failed to execute l0-compressor");
    let stdout_str = String::from_utf8_lossy(&output.stdout);
    assert!(stdout_str.contains("[Showing 5 head + 5 tail of 100 lines]"));
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn integration_banner_does_not_appear_when_not_truncated() {
    let t_bin = get_t_bin();
    let xdg = temp_xdg("banner-notrunc");
    let output = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .args([
            "--no-auto",
            "--head",
            "5",
            "--tail",
            "5",
            "--threshold",
            "10",
            "seq",
            "1",
            "5",
        ])
        .output()
        .expect("failed to execute l0-compressor");
    let stdout_str = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout_str.contains("truncated=true"));
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn integration_auto_flag_accepted() {
    let t_bin = get_t_bin();
    let xdg = temp_xdg("auto-flag");
    let output = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .args(["--auto", "echo", "hello"])
        .output()
        .expect("failed to execute l0-compressor with --auto");
    let stdout_str = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout_str.trim(), "hello");
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn test_auto_tuning_success_decay_e2e() {
    let t_bin = get_t_bin();
    let mut temp_dir = std::env::temp_dir();
    let unique_sub = format!(
        "l0-compressor-test-success-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    temp_dir.push(unique_sub);
    std::fs::create_dir_all(&temp_dir).unwrap();

    // `--no-squelch`: this test asserts the tuned tail CAP in the banner, so the
    // clean-success squelch must not trim the displayed tail out from under it.
    // Run 1, 2 & 3: Successes (no decay yet because history has 0, 1, 2 successes respectively)
    for _ in 0..3 {
        let output = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &temp_dir)
            .args(["--auto", "--no-squelch", "seq", "1", "200"])
            .output()
            .unwrap();
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        assert!(stdout_str.contains("[Showing 30 head + 30 tail of 200 lines]"));
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr_str.contains("auto-tuning"));
    }

    // Run 4: 20% decay against the SYSTEM defaults (30,30) → head=24, tail=24.
    // First firing in this bucket so the persistence sidecar is empty.
    {
        let output = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &temp_dir)
            .args(["--auto", "--no-squelch", "seq", "1", "200"])
            .output()
            .unwrap();
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        assert!(stdout_str.contains("[Showing 24 head + 24 tail of 200 lines]"));
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        assert!(stderr_str.contains("consecutive successful runs, optimizing head=24 tail=24"));
    }

    // Run 5: Step 5 — persistence compounds. The previous firing saved
    // (24, 24); now the moderate decay applies on TOP of that → 24*0.8=19.
    // Pre-Step-5 this run still showed 24/24 because the rule re-derived
    // from system defaults each time.
    {
        let output = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &temp_dir)
            .args(["--auto", "--no-squelch", "seq", "1", "200"])
            .output()
            .unwrap();
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout_str.contains("[Showing 19 head + 19 tail of 200 lines]"),
            "Step 5 compounding: expected 19/19; got: {stdout_str}"
        );
    }

    // Run 6: strong decay (5+ truncated) on cached (19, 19) → 19*0.6=11.
    // Pre-Step-5 this was 18/18 because the rule started from defaults.
    {
        let output = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &temp_dir)
            .args(["--auto", "--no-squelch", "seq", "1", "200"])
            .output()
            .unwrap();
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout_str.contains("[Showing 11 head + 11 tail of 200 lines"),
            "Step 5 compounding: expected 11/11; got: {stdout_str}"
        );
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        assert!(stderr_str.contains("consecutive successful runs, optimizing head=11 tail=11"));
    }

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn test_auto_tuning_failure_backoff_e2e() {
    let t_bin = get_t_bin();
    let mut temp_dir = std::env::temp_dir();
    let unique_sub = format!(
        "l0-compressor-test-fail-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    temp_dir.push(unique_sub);
    std::fs::create_dir_all(&temp_dir).unwrap();

    // 1st failure (no warning yet because history is empty)
    {
        let output = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &temp_dir)
            .args(["--auto", "sh", "-c", "seq 1 200; exit 1"])
            .output()
            .unwrap();
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr_str.contains("auto-tuning"));
    }

    // 2nd failure: F=1 -> tail_error scales to 240
    {
        let output = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &temp_dir)
            .args(["--auto", "sh", "-c", "seq 1 200; exit 1"])
            .output()
            .unwrap();
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        assert!(stderr_str.contains("1 consecutive failures detected, expanding tail_error to 240"));
    }

    // 3rd failure: Step 5 — persistence compounds. Cached tail_error from
    // the previous firing is 240, factor=3 → 720. Pre-Step-5 this was 360
    // because the rule started from the system default (120) each time.
    {
        let output = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &temp_dir)
            .args(["--auto", "sh", "-c", "seq 1 200; exit 1"])
            .output()
            .unwrap();
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr_str.contains("2 consecutive failures detected, expanding tail_error to 720"),
            "Step 5 compounding: expected 720; got: {stderr_str}"
        );
    }

    // Step 2 — bucket isolation. Switching args switches buckets, so a
    // different-args run is NOT influenced by the failure streak of the
    // previous bucket. (Pre-Step-2 this was a quirk: the prior 3 failures
    // would still drive `expand_tail_err` on this run even though the args
    // differ entirely.)
    {
        let output = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &temp_dir)
            .args(["--auto", "sh", "-c", "seq 1 2; exit 0"])
            .output()
            .unwrap();
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr_str.contains("auto-tuning"),
            "different-args run is in its own bucket (Step 2): {stderr_str}"
        );
    }

    // Running the original args once more is back in the failing bucket —
    // the 3 prior failures still dominate its history → expand still fires.
    // Step 5 — cached tail_error is 720 from the previous firing; factor=4
    // → 2880, capped at auto_ceiling (default 1000). Pre-Step-5 was 480.
    {
        let output = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &temp_dir)
            .args(["--auto", "sh", "-c", "seq 1 200; exit 1"])
            .output()
            .unwrap();
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr_str.contains("3 consecutive failures detected, expanding tail_error to 1000"),
            "Step 5 compounding hits ceiling: expected 1000; got: {stderr_str}"
        );
    }

    // And one more clean run in the OTHER bucket stays quiet — bucket B's
    // history is just one success, no streak to expand on.
    {
        let output = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &temp_dir)
            .args(["--auto", "sh", "-c", "seq 1 2; exit 0"])
            .output()
            .unwrap();
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr_str.contains("auto-tuning"));
    }

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn test_quiet_flag_suppresses_warnings() {
    let t_bin = get_t_bin();
    let mut temp_dir = std::env::temp_dir();
    let unique_sub = format!(
        "l0-compressor-test-quiet-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    temp_dir.push(unique_sub);
    std::fs::create_dir_all(&temp_dir).unwrap();

    // With --quiet, auto-tuning warnings should be suppressed
    let output = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &temp_dir)
        .args(["--auto", "--quiet", "sh", "-c", "seq 1 200; exit 1"])
        .output()
        .unwrap();
    let stderr_str = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr_str.contains("auto-tuning"));

    let _ = std::fs::remove_dir_all(temp_dir);
}

/// A unique, isolated `XDG_DATA_HOME` temp dir for metrics-touching tests.
fn temp_xdg(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "l0-compressor-it-{}-{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn tail_error_shows_more_tail_than_success() {
    // The headline behavior: on a non-zero exit the tail window is the larger
    // error tail; on success it's the small tail. This must hold on STDOUT, not
    // just in a stderr notice. (Regression guard for the old expand_tail no-op.)
    let t = get_t_bin();
    let xdg = temp_xdg("tailerr");

    let fail = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--no-auto", "sh", "-c", "seq 1 500; exit 1"])
        .output()
        .unwrap();
    let fail_out = String::from_utf8_lossy(&fail.stdout);
    // Error tail is 120 lines → line 400 (well past the 30-line success tail) is shown.
    assert!(
        fail_out.lines().any(|l| l == "400"),
        "error tail should reach ~120 deep (line 400 missing)"
    );
    assert!(fail_out.lines().any(|l| l == "500"));

    let ok = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--no-auto", "sh", "-c", "seq 1 500; exit 0"])
        .output()
        .unwrap();
    let ok_out = String::from_utf8_lossy(&ok.stdout);
    // Success tail is 30 lines → line 400 must NOT appear, but 500 (last) does.
    assert!(
        !ok_out.lines().any(|l| l == "400"),
        "success tail should be small (line 400 should not appear)"
    );
    assert!(ok_out.lines().any(|l| l == "500"));

    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn clean_success_squelch_trims_tail_and_no_squelch_opts_out() {
    // Default: a clean exit-0 run squelches the success tail (15 lines) so the
    // mid-tail (line 480) is dropped; `--no-squelch` restores the full 30-line
    // tail where line 480 reappears. The last line (500) survives either way.
    let t = get_t_bin();
    let xdg = temp_xdg("squelch");

    let default_run = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--no-auto", "sh", "-c", "seq 1 500; exit 0"])
        .output()
        .unwrap();
    let default_out = String::from_utf8_lossy(&default_run.stdout);
    assert!(
        !default_out.lines().any(|l| l == "480"),
        "squelched success tail (15) should not reach line 480"
    );
    assert!(default_out.lines().any(|l| l == "500"), "summary survives");
    // The banner must report the ACTUAL squelched tail, not the configured cap.
    assert!(
        default_out.contains("30 head + 15 tail"),
        "banner must honestly report the 15-line squelched tail: {default_out}"
    );

    let no_squelch = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--no-auto", "--no-squelch", "sh", "-c", "seq 1 500; exit 0"])
        .output()
        .unwrap();
    let no_squelch_out = String::from_utf8_lossy(&no_squelch.stdout);
    assert!(
        no_squelch_out.lines().any(|l| l == "480"),
        "--no-squelch restores the full 30-line success tail (line 480 present)"
    );
    assert!(
        no_squelch_out.contains("30 head + 30 tail"),
        "--no-squelch banner reports the full 30-line tail: {no_squelch_out}"
    );

    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn raw_mode_keeps_all_lines_and_no_json_truncation() {
    let t = get_t_bin();
    let xdg = temp_xdg("raw");
    let out = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--raw", "seq", "1", "5000"])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.lines().any(|l| l == "1"));
    assert!(s.lines().any(|l| l == "2500"));
    assert!(s.lines().any(|l| l == "5000"));
    assert!(!s.contains("omitted for LLM"), "raw must not truncate");

    // A big single-line JSON payload is kept verbatim in --raw.
    let json = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args([
            "--raw",
            "sh",
            "-c",
            "printf '{'; for i in $(seq 1 3000); do printf a; done; printf '}\\n'",
        ])
        .output()
        .unwrap();
    let js = String::from_utf8_lossy(&json.stdout);
    assert!(!js.contains("Large JSON Payload Truncated"));
    assert!(js.trim().len() >= 3000);
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn binary_output_carries_explicit_banner() {
    let t = get_t_bin();
    let xdg = temp_xdg("bin");
    let child = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["sh", "-c", "head -c 200000 /dev/urandom"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn");
    let out = child.wait_with_output().expect("wait");
    let s = String::from_utf8_lossy(&out.stdout);
    // Never silently forward the whole 200 KB blob.
    assert!(
        out.stdout.len() < 200_000,
        "binary should not be passed through whole"
    );
    assert!(
        s.contains("binary output detected"),
        "binary output should carry an explicit banner"
    );
    let _ = std::fs::remove_dir_all(&xdg);
}

#[cfg(unix)]
#[test]
fn idle_timeout_kills_hung_command() {
    let t = get_t_bin();
    let xdg = temp_xdg("idle");
    let start = Instant::now();
    let child = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--idle-timeout", "1", "sh", "-c", "sleep 8 | cat"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn");
    let _ = wait_timeout(child, Duration::from_secs(5)).expect("idle-timeout did not fire");
    assert!(
        start.elapsed() < Duration::from_secs(4),
        "watchdog should kill the command within ~1-2s, took {:?}",
        start.elapsed()
    );
    let _ = std::fs::remove_dir_all(&xdg);
}

#[cfg(unix)]
#[test]
fn sigterm_is_forwarded_and_propagated() {
    let t = get_t_bin();
    let xdg = temp_xdg("term");
    let child = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["sh", "-c", "sleep 30"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn");
    // Give it a moment to spawn the child group, then SIGTERM the proxy.
    thread::sleep(Duration::from_millis(500));
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status();
    let status =
        wait_timeout(child, Duration::from_secs(5)).expect("proxy did not exit after SIGTERM");
    // 128 + SIGTERM(15) = 143
    assert_eq!(
        status.code(),
        Some(143),
        "proxy should propagate SIGTERM as 143"
    );
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn concurrent_metric_writers_do_not_corrupt_the_log() {
    let t = get_t_bin();
    let xdg = temp_xdg("conc");
    let n = 16;
    let mut kids = Vec::new();
    for i in 0..n {
        kids.push(
            Command::new(&t)
                .env("XDG_DATA_HOME", &xdg)
                .env("L0_COMPRESSOR_GUARD", "0")
                .args(["--no-auto", "echo", &format!("run{}", i)])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("spawn"),
        );
    }
    for k in kids {
        let _ = wait_timeout(k, Duration::from_secs(10));
    }
    let metrics = xdg.join("l0-compressor").join("metrics.jsonl");
    let content = std::fs::read_to_string(&metrics).expect("metrics file");
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        n,
        "every concurrent run should append exactly one line"
    );
    for l in &lines {
        assert!(
            l.starts_with('{') && l.ends_with('}'),
            "interleaved/corrupted JSONL line: {l}"
        );
    }
    let _ = std::fs::remove_dir_all(&xdg);
}

/// Resident memory (KB) of a process via `ps`, or None if it can't be read.
#[cfg(unix)]
fn rss_kb(pid: u32) -> Option<u64> {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .lines()
        .next()?
        .trim()
        .parse::<u64>()
        .ok()
}

#[cfg(unix)]
#[test]
fn memory_stays_bounded_on_a_giant_newline_free_stream() {
    // Push ~100 MB as a SINGLE line with no newline (the minified-bundle OOM case).
    // With the bounded reader, l0-compressor must keep only ~1 MB regardless of input
    // size: we sample its RSS while it runs and assert the peak stays far below the
    // input volume. (Before the fix, RSS tracked the input — the line is buffered
    // whole and again as the stored head line → ~200 MB.)
    let t = get_t_bin();
    let xdg = temp_xdg("rss");
    let mut child = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args([
            "--no-auto",
            "sh",
            "-c",
            "head -c 104857600 /dev/zero | tr '\\0' a",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn");

    let pid = child.id();
    let start = Instant::now();
    let mut peak_kb = 0u64;
    loop {
        match child.try_wait().expect("try_wait") {
            Some(_) => break,
            None => {
                if let Some(kb) = rss_kb(pid) {
                    peak_kb = peak_kb.max(kb);
                }
                if start.elapsed() > Duration::from_secs(30) {
                    let _ = child.kill();
                    panic!(
                        "l0-compressor hung on a 200 MB single line (possible unbounded buffering)"
                    );
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    assert!(peak_kb > 0, "could not sample RSS via ps");
    // 200 MB pushed in; peak must be a small fraction of that. Generous ceiling
    // (120 MB) to avoid flakiness, while the broken behavior would be ~200 MB.
    assert!(
        peak_kb < 120 * 1024,
        "peak RSS {peak_kb} KB is too high for a bounded buffer — the giant line was buffered whole"
    );
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn stats_renders_with_seeded_metrics() {
    let t = get_t_bin();
    let xdg = temp_xdg("stats");
    let dir = xdg.join("l0-compressor");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("metrics.jsonl"),
        "{\"ts\":\"2026-06-04T10:00:00Z\",\"cmd\":\"cargo\",\"tokens_saved\":900,\"tokens_raw\":1000}\n\
         {\"ts\":\"2026-06-04T10:01:00Z\",\"cmd\":\"git\",\"tokens_saved\":100,\"tokens_raw\":400}\n",
    )
    .unwrap();
    let out = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .arg("--stats")
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("cargo"));
    assert!(s.contains("TELEMETRY"));
    assert!(s.contains("Runs"));
    // Piped (non-TTY) output must be free of raw ANSI escapes.
    assert!(
        !s.contains('\x1b'),
        "stats output should be plain when not a TTY"
    );
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn stats_json_is_machine_readable() {
    let t = get_t_bin();
    let xdg = temp_xdg("statsjson");
    let dir = xdg.join("l0-compressor");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("metrics.jsonl"),
        "{\"ts\":\"2026-06-05T10:00:00Z\",\"cmd\":\"cargo\",\"tokens_saved\":900,\"tokens_raw\":1000}\n",
    )
    .unwrap();
    let out = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--stats", "--json", "--cost-per-mtok", "3"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(!s.contains('\x1b'), "json output must be plain");
    assert!(s.contains("\"total_runs\""));
    assert!(s.contains("\"usd_saved\""));
    assert!(s.contains("\"command\": \"cargo\""));
    // Fields added with the dashboard refresh.
    assert!(s.contains("\"median_run_efficiency_pct\""));
    assert!(s.contains("\"recover_defaults\""));
    assert!(s.contains("\"noisy_last_seen\""));
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn discover_lists_commands() {
    let t = get_t_bin();
    let xdg = temp_xdg("discover");
    let dir = xdg.join("l0-compressor");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("metrics.jsonl"),
        "{\"ts\":\"2026-06-05T10:00:00Z\",\"cmd\":\"cargo\",\"tokens_saved\":900,\"tokens_raw\":1000}\n",
    )
    .unwrap();
    let out = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .arg("--discover")
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("optimization advisor"));
    assert!(s.contains("cargo"));
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn stats_skips_malformed_and_empty_lines() {
    let t = get_t_bin();
    let xdg = temp_xdg("statsmalformed");
    let dir = xdg.join("l0-compressor");
    std::fs::create_dir_all(&dir).unwrap();
    // valid, blank, garbage, valid → exactly 2 rows aggregated, no crash.
    let body = "{\"ts\":\"2026-06-05T10:00:00Z\",\"cmd\":\"cargo\",\"tokens_saved\":900,\"tokens_raw\":1000}\n\
                \n\
                this is not json {{{\n\
                {\"ts\":\"2026-06-05T10:01:00Z\",\"cmd\":\"git\",\"tokens_saved\":100,\"tokens_raw\":400}\n";
    std::fs::write(dir.join("metrics.jsonl"), body).unwrap();
    let out = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--stats", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("\"command\": \"cargo\"") && s.contains("\"command\": \"git\""));
    assert!(s.contains("\"total_runs\": 2"), "exactly 2 valid rows: {s}");
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn config_overrides_apply_and_explicit_cli_wins() {
    // Regression guard for the config-precedence wiring (explicit CLI > config >
    // default), including the subtle "user passes a value equal to the default"
    // case which must still override a differing config value.
    let t = get_t_bin();
    let xdg = temp_xdg("cfgprec-data");
    let cfg = temp_xdg("cfgprec-conf");
    std::fs::create_dir_all(cfg.join("l0-compressor")).unwrap();
    std::fs::write(
        cfg.join("l0-compressor").join("config.json"),
        "{\"commands\":{\"seq\":{\"head\":7}}}",
    )
    .unwrap();
    let run = |extra: &[&str]| -> String {
        let mut c = Command::new(&t);
        c.env("XDG_DATA_HOME", &xdg)
            .env("XDG_CONFIG_HOME", &cfg)
            .env("L0_COMPRESSOR_GUARD", "0")
            .arg("--no-auto")
            .args(extra)
            .args(["seq", "1", "500"]);
        String::from_utf8_lossy(&c.output().unwrap().stdout).into_owned()
    };
    // 500 lines > threshold → truncated → banner reports the head budget used.
    assert!(
        run(&[]).contains("7 head"),
        "config head=7 should apply with no CLI flag"
    );
    assert!(
        run(&["--head", "30"]).contains("30 head"),
        "explicit --head 30 (== default) must override config head=7"
    );
    let _ = std::fs::remove_dir_all(&xdg);
    let _ = std::fs::remove_dir_all(&cfg);
}

#[test]
fn config_toml_is_picked_up_transparently() {
    // A flat TOML config (no JSON) must be auto-detected and applied.
    let t = get_t_bin();
    let xdg = temp_xdg("cfgtoml-data");
    let cfg = temp_xdg("cfgtoml-conf");
    std::fs::create_dir_all(cfg.join("l0-compressor")).unwrap();
    std::fs::write(
        cfg.join("l0-compressor").join("config.toml"),
        "[seq]\nhead = 5\n",
    )
    .unwrap();
    let out = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("XDG_CONFIG_HOME", &cfg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--no-auto", "seq", "1", "500"])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("5 head"), "config.toml head=5 should apply: {s}");
    let _ = std::fs::remove_dir_all(&xdg);
    let _ = std::fs::remove_dir_all(&cfg);
}

// ── Auto-tuning telemetry — e2e ──────────────────────────────────────────────
//
// These tests drive the real binary against an isolated XDG dir so the smoke
// behavior the user sees on their box (the new AUTO-TUNING section in --stats
// + the auto_tuning JSON block) is anchored against real metric records, not
// internal aggregation helpers.

#[test]
fn auto_tuning_section_renders_when_rule_fires_for_real() {
    // Drive enough consecutive failures to fire `expand_tail_err`, then run
    // --stats and assert the AUTO-TUNING section appears with the firings the
    // rule generated. This is the load-bearing demo: if this test fails, the
    // user's --stats no longer reports what the auto-tuner is doing.
    let t_bin = get_t_bin();
    let xdg = temp_xdg("autotune-stats");

    // Run a failing command 3 times — from run #2 onward the rule fires.
    for _ in 0..3 {
        let _ = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &xdg)
            .env("L0_COMPRESSOR_GUARD", "0")
            .args(["--auto", "sh", "-c", "seq 1 200; exit 1"])
            .output()
            .unwrap();
    }

    // metrics.jsonl must now carry at least one adaptive_event=expand_tail_err.
    let metrics = xdg.join("l0-compressor").join("metrics.jsonl");
    let body = std::fs::read_to_string(&metrics).expect("metrics file should exist");
    assert!(
        body.contains("\"adaptive_event\":\"expand_tail_err\""),
        "metrics should carry the event tag: {body}"
    );

    // --stats must render the new section with non-zero firings.
    let out = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .arg("--stats")
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("AUTO-TUNING"), "section header missing: {s}");
    assert!(s.contains("Firings"), "firings line missing: {s}");
    assert!(s.contains("expand_tail_err"), "event label missing: {s}");
    // The exact count depends on how many runs the rule observed in history,
    // but it must be at least 1 (the second run triggered it).
    assert!(
        !s.contains("Firings     0"),
        "expected non-zero Firings: {s}"
    );
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn auto_tuning_section_says_quiet_when_no_firings() {
    // Seed metrics.jsonl with only clean (non-firing) records. The section
    // must still appear, with firings=0 and the "auto-tuning quiet" hint —
    // honesty: we explicitly say the rule didn't fire instead of hiding it.
    let t_bin = get_t_bin();
    let xdg = temp_xdg("autotune-quiet");
    let dir = xdg.join("l0-compressor");
    std::fs::create_dir_all(&dir).unwrap();
    let body = "{\"ts\":\"2026-06-05T10:00:00Z\",\"cmd\":\"cargo\",\"args\":\"\",\"tokens_saved\":900,\"tokens_raw\":1000,\"lines_raw\":20,\"lines_final\":20,\"truncated\":false,\"strategy\":\"passthrough\",\"exit_code\":0,\"duration_ms\":5,\"version\":\"0.1.10\"}\n";
    std::fs::write(dir.join("metrics.jsonl"), body).unwrap();

    let out = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .arg("--stats")
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("AUTO-TUNING"), "section header missing: {s}");
    assert!(s.contains("Firings"), "firings line missing: {s}");
    assert!(
        s.contains("quiet"),
        "should explicitly mark zero-firings as quiet: {s}"
    );
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn auto_tuning_json_block_is_well_formed() {
    // Seed mixed records (one of each event type + one without). Assert that
    // --stats --json produces a valid auto_tuning object with the correct
    // per-event counters AND that per-command entries carry auto_tuning too.
    let t_bin = get_t_bin();
    let xdg = temp_xdg("autotune-json");
    let dir = xdg.join("l0-compressor");
    std::fs::create_dir_all(&dir).unwrap();
    let body = concat!(
        "{\"ts\":\"2026-06-05T10:00:00Z\",\"cmd\":\"grep\",\"args\":\"\",\"tokens_saved\":0,\"tokens_raw\":0,\"lines_raw\":0,\"lines_final\":0,\"truncated\":false,\"strategy\":\"passthrough\",\"exit_code\":1,\"duration_ms\":5,\"version\":\"0.1.10\",\"adaptive_event\":\"expand_tail_err\"}\n",
        "{\"ts\":\"2026-06-05T10:01:00Z\",\"cmd\":\"seq\",\"args\":\"\",\"tokens_saved\":900,\"tokens_raw\":1000,\"lines_raw\":500,\"lines_final\":60,\"truncated\":true,\"strategy\":\"head_tail\",\"exit_code\":0,\"duration_ms\":5,\"version\":\"0.1.10\",\"adaptive_event\":\"decay_moderate\"}\n",
        "{\"ts\":\"2026-06-05T10:02:00Z\",\"cmd\":\"seq\",\"args\":\"\",\"tokens_saved\":900,\"tokens_raw\":1000,\"lines_raw\":500,\"lines_final\":60,\"truncated\":true,\"strategy\":\"head_tail\",\"exit_code\":0,\"duration_ms\":5,\"version\":\"0.1.10\",\"adaptive_event\":\"decay_strong\"}\n",
        "{\"ts\":\"2026-06-05T10:03:00Z\",\"cmd\":\"cat\",\"args\":\"\",\"tokens_saved\":0,\"tokens_raw\":100,\"lines_raw\":10,\"lines_final\":10,\"truncated\":false,\"strategy\":\"passthrough\",\"exit_code\":0,\"duration_ms\":5,\"version\":\"0.1.10\"}\n",
    );
    std::fs::write(dir.join("metrics.jsonl"), body).unwrap();

    let out = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--stats", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");

    // Top-level auto_tuning object.
    let at = v.get("auto_tuning").expect("auto_tuning key present");
    assert_eq!(at["firings"].as_u64(), Some(3));
    assert_eq!(at["expand_tail_err"].as_u64(), Some(1));
    assert_eq!(at["decay_moderate"].as_u64(), Some(1));
    assert_eq!(at["decay_strong"].as_u64(), Some(1));
    // grep record had exit=1 + lines_raw=0 → 1 noisy.
    assert_eq!(at["noisy"].as_u64(), Some(1));

    // Per-command auto_tuning blocks.
    let cmds = v["commands"].as_array().expect("commands array");
    let grep = cmds
        .iter()
        .find(|c| c["command"] == "grep")
        .expect("grep entry");
    assert_eq!(grep["auto_tuning"]["firings"].as_u64(), Some(1));
    assert_eq!(grep["auto_tuning"]["noisy"].as_u64(), Some(1));
    let cat = cmds
        .iter()
        .find(|c| c["command"] == "cat")
        .expect("cat entry");
    assert_eq!(cat["auto_tuning"]["firings"].as_u64(), Some(0));

    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn auto_tuning_handles_backcompat_mixed_old_and_new_records() {
    // The metrics file in the wild is a long-lived JSONL: it contains many
    // pre-0.1.10 records without an `adaptive_event` field plus new records
    // that carry it. --stats must (a) count all rows in total_runs, (b)
    // count only the new tagged rows in firings, (c) never panic.
    let t_bin = get_t_bin();
    let xdg = temp_xdg("autotune-backcompat");
    let dir = xdg.join("l0-compressor");
    std::fs::create_dir_all(&dir).unwrap();
    let body = concat!(
        // OLD records — no adaptive_event field at all.
        "{\"ts\":\"2026-06-01T10:00:00Z\",\"cmd\":\"cargo\",\"args\":\"\",\"tokens_saved\":900,\"tokens_raw\":1000,\"lines_raw\":50,\"lines_final\":10,\"truncated\":true,\"strategy\":\"head_tail\",\"exit_code\":0,\"duration_ms\":5,\"version\":\"0.1.9\"}\n",
        "{\"ts\":\"2026-06-01T10:01:00Z\",\"cmd\":\"cargo\",\"args\":\"\",\"tokens_saved\":800,\"tokens_raw\":1000,\"lines_raw\":50,\"lines_final\":10,\"truncated\":true,\"strategy\":\"head_tail\",\"exit_code\":1,\"duration_ms\":5,\"version\":\"0.1.9\"}\n",
        // NEW records — with adaptive_event.
        "{\"ts\":\"2026-06-05T10:00:00Z\",\"cmd\":\"cargo\",\"args\":\"\",\"tokens_saved\":900,\"tokens_raw\":1000,\"lines_raw\":50,\"lines_final\":10,\"truncated\":true,\"strategy\":\"head_tail\",\"exit_code\":0,\"duration_ms\":5,\"version\":\"0.1.10\",\"adaptive_event\":\"decay_moderate\"}\n",
        "{\"ts\":\"2026-06-05T10:01:00Z\",\"cmd\":\"cargo\",\"args\":\"\",\"tokens_saved\":900,\"tokens_raw\":1000,\"lines_raw\":50,\"lines_final\":10,\"truncated\":true,\"strategy\":\"head_tail\",\"exit_code\":0,\"duration_ms\":5,\"version\":\"0.1.10\",\"adaptive_event\":\"decay_strong\"}\n",
    );
    std::fs::write(dir.join("metrics.jsonl"), body).unwrap();

    let out = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--stats", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "stats must not panic on mixed file");
    let v: serde_json::Value =
        serde_json::from_str(&out.stdout.iter().map(|&b| b as char).collect::<String>())
            .expect("valid JSON");
    assert_eq!(
        v["total_runs"].as_u64(),
        Some(4),
        "all 4 rows count toward runs"
    );
    assert_eq!(
        v["auto_tuning"]["firings"].as_u64(),
        Some(2),
        "only the 2 tagged rows count as firings"
    );
    assert_eq!(v["auto_tuning"]["decay_moderate"].as_u64(), Some(1));
    assert_eq!(v["auto_tuning"]["decay_strong"].as_u64(), Some(1));
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn step5_persistence_compounds_decay_across_runs() {
    // E2E proof of compounding: run the same command enough times that decay
    // fires 3 times in a row. Without persistence the rule shrinks from
    // defaults each time (30→24, 30→24, 30→24). With persistence it
    // compounds (30→24→19→11), and the head/tail in the truncation banner
    // shows the progression.
    let t_bin = get_t_bin();
    let xdg = temp_xdg("step5-compound");

    // `--no-squelch` throughout: the banner assertions check the tuned tail CAP,
    // which the clean-success squelch would otherwise trim at render time.
    // Run 1-3: build up history (3 truncated successes — below trigger).
    for _ in 0..3 {
        let _ = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &xdg)
            .env("L0_COMPRESSOR_GUARD", "0")
            .args(["--auto", "--no-squelch", "seq", "1", "200"])
            .output()
            .unwrap();
    }
    // Run 4: 3 prior truncated → decay_moderate fires, 30→24, saved.
    let r4 = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--auto", "--no-squelch", "seq", "1", "200"])
        .output()
        .unwrap();
    let s4 = String::from_utf8_lossy(&r4.stdout);
    assert!(
        s4.contains("[Showing 24 head + 24 tail of 200 lines]"),
        "run 4 should land at 24/24: {s4}"
    );

    // Run 5: cached (24,24) → decay_moderate (still 3-4 priors) → 24*0.8=19.
    let r5 = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--auto", "--no-squelch", "seq", "1", "200"])
        .output()
        .unwrap();
    let s5 = String::from_utf8_lossy(&r5.stdout);
    assert!(
        s5.contains("[Showing 19 head + 19 tail of 200 lines]"),
        "run 5 compounds from 24 to 19: {s5}"
    );

    // Run 6: cached (19,19), 5 priors → decay_strong → 19*0.6=11.
    let r6 = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--auto", "--no-squelch", "seq", "1", "200"])
        .output()
        .unwrap();
    let s6 = String::from_utf8_lossy(&r6.stdout);
    assert!(
        s6.contains("[Showing 11 head + 11 tail of 200 lines"),
        "run 6 compounds from 19 to 11: {s6}"
    );

    // The tuned.jsonl sidecar must carry the bucket's tune.
    let tuned = xdg.join("l0-compressor").join("tuned.jsonl");
    assert!(tuned.exists(), "tuned.jsonl should exist at {tuned:?}");
    let body = std::fs::read_to_string(&tuned).unwrap();
    // At least one record must show head=11 (the most-recent compound).
    assert!(
        body.contains("\"head\":11"),
        "tuned.jsonl should record head=11: {body}"
    );
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn step5_persistence_bucket_isolation_via_args_hash() {
    // Two distinct (cmd, args_hash) buckets must have INDEPENDENT tunes —
    // shrinking bucket A doesn't bleed onto bucket B's defaults.
    let t_bin = get_t_bin();
    let xdg = temp_xdg("step5-iso");

    // `--no-squelch`: assert the tuned tail CAP in the banner without the
    // clean-success squelch trimming the displayed tail.
    // Drive bucket A through 4 truncated successes → decay_moderate fires
    // once and persists (24, 24).
    for _ in 0..4 {
        let _ = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &xdg)
            .env("L0_COMPRESSOR_GUARD", "0")
            .args(["--auto", "--no-squelch", "seq", "1", "200"])
            .output()
            .unwrap();
    }

    // Bucket B (different args, same cmd) — fresh history, no cached tune
    // means it must start from system defaults (30, 30).
    let out = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--auto", "--no-squelch", "seq", "1", "300"])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("[Showing 30 head + 30 tail of 300 lines]"),
        "bucket B is fresh — should use defaults 30/30, not bucket A's tune: {s}"
    );

    // tuned.jsonl should carry bucket A's tune; bucket B isn't persisted
    // because it didn't fire any rule.
    let tuned = std::fs::read_to_string(xdg.join("l0-compressor").join("tuned.jsonl")).unwrap();
    // Bucket B's args_hash should NOT appear (no firing → no persistence).
    let mut a_count = 0;
    let mut b_count = 0;
    let h_a = {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in "1 200".as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{:08x}", h as u32)
    };
    let h_b = {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in "1 300".as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{:08x}", h as u32)
    };
    for line in tuned.lines() {
        if line.contains(&h_a) {
            a_count += 1;
        }
        if line.contains(&h_b) {
            b_count += 1;
        }
    }
    assert!(a_count > 0, "bucket A should be persisted");
    assert_eq!(b_count, 0, "bucket B should NOT be persisted (no firing)");
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn step4_decay_steady_fires_on_seeded_mixed_window() {
    // 16/20 truncated + 4 non-truncated at the most-recent end → the
    // consecutive-streak decay rule sees streak=0 and skips, but
    // decay_steady catches the steady-state pattern.
    let t_bin = get_t_bin();
    let xdg = temp_xdg("step4-steady");
    let dir = xdg.join("l0-compressor");
    std::fs::create_dir_all(&dir).unwrap();

    let args_hash = {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in "hi".as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{:08x}", h as u32)
    };

    let mut body = String::new();
    for _ in 0..16 {
        body.push_str(&format!(
            "{{\"ts\":\"2026-06-05T10:00:00Z\",\"cmd\":\"cat\",\"args\":\"hi\",\"bytes_raw\":500,\"bytes_final\":300,\"lines_raw\":100,\"lines_final\":80,\"tokens_raw\":125,\"tokens_final\":75,\"tokens_saved\":50,\"truncated\":true,\"strategy\":\"head_tail\",\"exit_code\":0,\"duration_ms\":5,\"version\":\"0.1.10\",\"args_hash\":\"{args_hash}\"}}\n"
        ));
    }
    for _ in 0..4 {
        body.push_str(&format!(
            "{{\"ts\":\"2026-06-05T10:01:00Z\",\"cmd\":\"cat\",\"args\":\"hi\",\"bytes_raw\":250,\"bytes_final\":250,\"lines_raw\":50,\"lines_final\":50,\"tokens_raw\":62,\"tokens_final\":62,\"tokens_saved\":0,\"truncated\":false,\"strategy\":\"passthrough\",\"exit_code\":0,\"duration_ms\":5,\"version\":\"0.1.10\",\"args_hash\":\"{args_hash}\"}}\n"
        ));
    }
    std::fs::write(dir.join("metrics.jsonl"), body).unwrap();

    // Run cat with non-existent file that produces "hi" path → use `echo hi`
    // instead so the args_hash actually matches what main computes.
    let _ = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--auto", "cat", "hi"])
        .output()
        .unwrap();

    let final_body = std::fs::read_to_string(dir.join("metrics.jsonl")).unwrap();
    let last_line = final_body.lines().last().expect("at least one line");
    assert!(
        last_line.contains("\"adaptive_event\":\"decay_steady\""),
        "last line should carry decay_steady: {last_line}"
    );

    // --stats counts the firing under the right event.
    let out = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--stats", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("valid JSON");
    assert_eq!(v["auto_tuning"]["decay_steady"].as_u64(), Some(1));
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn step3_proactive_shrink_fires_on_seeded_clean_history() {
    // Seed metrics.jsonl with 25 clean records for a single bucket; the next
    // real run must (a) write adaptive_event=proactive_shrink, and (b) be
    // run with a smaller head/tail than the default. Verify the JSONL record
    // carries the new event.
    let t_bin = get_t_bin();
    let xdg = temp_xdg("step3-shrink");
    let dir = xdg.join("l0-compressor");
    std::fs::create_dir_all(&dir).unwrap();

    // Compute the args_hash for the args we will use below so the seeded
    // history matches the next run's bucket. The args string is built by
    // cmd_args_string, which for `l0-compressor echo hi` is `"hi"`.
    let args_hash = {
        // FNV-1a 64-bit, low 32 bits as 8 hex chars — must match production.
        let mut h: u64 = 0xcbf29ce484222325;
        for b in "hi".as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{:08x}", h as u32)
    };

    let mut body = String::new();
    for _ in 0..25 {
        body.push_str(&format!(
            "{{\"ts\":\"2026-06-05T10:00:00Z\",\"cmd\":\"echo\",\"args\":\"hi\",\"bytes_raw\":3,\"bytes_final\":3,\"lines_raw\":1,\"lines_final\":1,\"tokens_raw\":1,\"tokens_final\":1,\"tokens_saved\":0,\"truncated\":false,\"strategy\":\"passthrough\",\"exit_code\":0,\"duration_ms\":1,\"version\":\"0.1.10\",\"args_hash\":\"{args_hash}\"}}\n"
        ));
    }
    std::fs::write(dir.join("metrics.jsonl"), body).unwrap();

    // Real run with the matching bucket.
    let _ = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--auto", "echo", "hi"])
        .output()
        .unwrap();

    // The 26th JSONL line — appended by the run we just made — must carry
    // adaptive_event=proactive_shrink.
    let final_body = std::fs::read_to_string(dir.join("metrics.jsonl")).unwrap();
    let last_line = final_body.lines().last().expect("at least one line");
    assert!(
        last_line.contains("\"adaptive_event\":\"proactive_shrink\""),
        "last line should carry proactive_shrink: {last_line}"
    );

    // --stats surfaces it under the new event row.
    let out = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--stats", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("valid JSON");
    assert_eq!(v["auto_tuning"]["proactive_shrink"].as_u64(), Some(1));
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn step3_does_not_fire_with_short_history() {
    // 5 clean records — below MIN=20 → no event.
    let t_bin = get_t_bin();
    let xdg = temp_xdg("step3-short");
    let dir = xdg.join("l0-compressor");
    std::fs::create_dir_all(&dir).unwrap();
    let args_hash = {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in "hi".as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{:08x}", h as u32)
    };
    let mut body = String::new();
    for _ in 0..5 {
        body.push_str(&format!(
            "{{\"ts\":\"2026-06-05T10:00:00Z\",\"cmd\":\"echo\",\"args\":\"hi\",\"bytes_raw\":3,\"bytes_final\":3,\"lines_raw\":1,\"lines_final\":1,\"tokens_raw\":1,\"tokens_final\":1,\"tokens_saved\":0,\"truncated\":false,\"strategy\":\"passthrough\",\"exit_code\":0,\"duration_ms\":1,\"version\":\"0.1.10\",\"args_hash\":\"{args_hash}\"}}\n"
        ));
    }
    std::fs::write(dir.join("metrics.jsonl"), body).unwrap();
    let _ = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--auto", "echo", "hi"])
        .output()
        .unwrap();
    let final_body = std::fs::read_to_string(dir.join("metrics.jsonl")).unwrap();
    let last_line = final_body.lines().last().expect("at least one line");
    assert!(
        !last_line.contains("\"adaptive_event\""),
        "should not fire: {last_line}"
    );
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn step2_args_hash_lands_in_jsonl_per_run() {
    // Each run writes an args_hash field; different args produce different
    // hashes. This is the load-bearing serialization test.
    let t_bin = get_t_bin();
    let xdg = temp_xdg("step2-jsonl");

    let _ = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--auto", "sh", "-c", "echo one"])
        .output()
        .unwrap();
    let _ = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--auto", "sh", "-c", "echo two"])
        .output()
        .unwrap();

    let metrics = xdg.join("l0-compressor").join("metrics.jsonl");
    let body = std::fs::read_to_string(&metrics).expect("metrics exist");
    let mut hashes: Vec<String> = Vec::new();
    for line in body.lines() {
        let v: serde_json::Value = serde_json::from_str(line).expect("valid JSON");
        let h = v["args_hash"]
            .as_str()
            .expect("args_hash present")
            .to_string();
        assert_eq!(h.len(), 8, "8-char hex: {h}");
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()), "hex: {h}");
        hashes.push(h);
    }
    assert_eq!(hashes.len(), 2);
    assert_ne!(hashes[0], hashes[1], "different args → different hashes");
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn step2_bucket_isolates_failure_streak_across_distinct_args() {
    // Counter-test of bucket isolation, end-to-end through the binary.
    // Bucket A accumulates 3 real failures; bucket B (same cmd, different
    // args) is then introduced. Bucket B's first run must NOT inherit
    // bucket A's expand streak.
    let t_bin = get_t_bin();
    let xdg = temp_xdg("step2-isolation");

    // 3 real failures in bucket A (same args every time).
    for _ in 0..3 {
        let _ = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &xdg)
            .env("L0_COMPRESSOR_GUARD", "0")
            .args(["--auto", "sh", "-c", "seq 1 200; exit 1"])
            .output()
            .unwrap();
    }

    // Now run a DIFFERENT-args command (bucket B). Expect: NO auto-tuning
    // warning on stderr, because bucket B's history is empty.
    let out = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--auto", "sh", "-c", "seq 1 50; exit 1"])
        .output()
        .unwrap();
    let stderr_str = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr_str.contains("auto-tuning"),
        "bucket B should not inherit bucket A's streak: {stderr_str}"
    );

    // JSONL records show two DISTINCT args_hash values. (cmd_name extracts
    // the first word from `sh -c "<cmd> …"`, so both bucket A and B end up
    // with cmd="seq" — that's fine; bucketing is by args_hash, not cmd.)
    let metrics = xdg.join("l0-compressor").join("metrics.jsonl");
    let body = std::fs::read_to_string(&metrics).expect("metrics exist");
    let mut hashes = std::collections::HashSet::new();
    for line in body.lines() {
        let v: serde_json::Value = serde_json::from_str(line).expect("JSON");
        if let Some(h) = v["args_hash"].as_str() {
            hashes.insert(h.to_string());
        }
    }
    assert!(
        hashes.len() >= 2,
        "expected ≥2 distinct args_hash values across runs: {hashes:?}"
    );

    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn step2_same_args_keeps_same_bucket_and_still_fires() {
    // Regression guard: bucketing must not break the within-bucket learning.
    // Same args 3 times in a row → bucket sees streak=2 on run #3, expand
    // fires (factor=3) → adaptive_event=expand_tail_err in the JSONL.
    let t_bin = get_t_bin();
    let xdg = temp_xdg("step2-same-bucket");

    for _ in 0..3 {
        let _ = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &xdg)
            .env("L0_COMPRESSOR_GUARD", "0")
            .args(["--auto", "sh", "-c", "seq 1 200; exit 1"])
            .output()
            .unwrap();
    }
    let metrics = xdg.join("l0-compressor").join("metrics.jsonl");
    let body = std::fs::read_to_string(&metrics).expect("metrics exist");
    assert!(
        body.contains("\"adaptive_event\":\"expand_tail_err\""),
        "within-bucket learning must still fire: {body}"
    );
    // All records share one args_hash (same args every time).
    let mut hashes = std::collections::HashSet::new();
    for line in body.lines() {
        let v: serde_json::Value = serde_json::from_str(line).expect("JSON");
        if let Some(h) = v["args_hash"].as_str() {
            hashes.insert(h.to_string());
        }
    }
    assert_eq!(hashes.len(), 1, "single bucket: {hashes:?}");
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn step2_backcompat_pre_step2_records_dont_break_aggregation() {
    // Mixed file: pre-Step-2 records without args_hash + new records with
    // args_hash. --stats must aggregate all of them under their `cmd` column
    // and never panic — the args_hash bucketing is invisible at this layer.
    let t_bin = get_t_bin();
    let xdg = temp_xdg("step2-backcompat");
    let dir = xdg.join("l0-compressor");
    std::fs::create_dir_all(&dir).unwrap();
    let body = concat!(
        // OLD: no args_hash, no adaptive_event.
        "{\"ts\":\"2026-06-01T10:00:00Z\",\"cmd\":\"cargo\",\"args\":\"test\",\"tokens_saved\":900,\"tokens_raw\":1000,\"lines_raw\":50,\"lines_final\":10,\"truncated\":true,\"strategy\":\"head_tail\",\"exit_code\":0,\"duration_ms\":5,\"version\":\"0.1.9\"}\n",
        // NEW: with args_hash, with adaptive_event.
        "{\"ts\":\"2026-06-05T10:00:00Z\",\"cmd\":\"cargo\",\"args\":\"test\",\"tokens_saved\":900,\"tokens_raw\":1000,\"lines_raw\":50,\"lines_final\":10,\"truncated\":true,\"strategy\":\"head_tail\",\"exit_code\":0,\"duration_ms\":5,\"version\":\"0.1.10\",\"adaptive_event\":\"decay_moderate\",\"args_hash\":\"a1b2c3d4\"}\n",
        "{\"ts\":\"2026-06-05T10:01:00Z\",\"cmd\":\"cargo\",\"args\":\"build\",\"tokens_saved\":100,\"tokens_raw\":200,\"lines_raw\":30,\"lines_final\":10,\"truncated\":false,\"strategy\":\"passthrough\",\"exit_code\":0,\"duration_ms\":5,\"version\":\"0.1.10\",\"args_hash\":\"e5f60718\"}\n",
    );
    std::fs::write(dir.join("metrics.jsonl"), body).unwrap();

    let out = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--stats", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "stats must not panic on mixed file");
    let v: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("valid JSON");
    // All 3 rows counted in total_runs.
    assert_eq!(v["total_runs"].as_u64(), Some(3));
    // Only the new record carries adaptive_event=decay_moderate.
    assert_eq!(v["auto_tuning"]["firings"].as_u64(), Some(1));
    assert_eq!(v["auto_tuning"]["decay_moderate"].as_u64(), Some(1));
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn auto_tuning_step1_skips_expand_on_empty_failures() {
    // Real binary, real command: run something that fails with zero output
    // (the canonical no-match case) several times. Before Step 1, the second
    // run would trigger `expand_tail_err`; after Step 1, the rule sees only
    // noisy history → never fires → adaptive_event remains absent on every
    // record, --stats shows 0 firings for that command.
    let t_bin = get_t_bin();
    let xdg = temp_xdg("autotune-step1-skip");

    // `false` is the canonical exit=1 with zero stdout/stderr command.
    for _ in 0..4 {
        let _ = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &xdg)
            .env("L0_COMPRESSOR_GUARD", "0")
            .args(["--auto", "false"])
            .output()
            .unwrap();
    }

    // No metric in the JSONL must carry an adaptive_event tag — the rule
    // never fired because every history entry is noisy.
    let metrics = xdg.join("l0-compressor").join("metrics.jsonl");
    let body = std::fs::read_to_string(&metrics).expect("metrics exist");
    assert!(
        !body.contains("\"adaptive_event\""),
        "Step 1 should suppress the rule; got: {body}"
    );

    // --stats --json: auto_tuning.firings == 0, noisy == 0.
    let out = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--stats", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("valid JSON");
    assert_eq!(v["auto_tuning"]["firings"].as_u64(), Some(0));
    assert_eq!(v["auto_tuning"]["noisy"].as_u64(), Some(0));
    // ...but the runs still count toward total_runs (we didn't drop telemetry).
    assert!(v["total_runs"].as_u64().unwrap() >= 4);
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn auto_tuning_step1_still_fires_on_real_failures_with_output() {
    // Counter-test: a failing command that DOES produce output must still
    // trigger expand. This is the regression guard — Step 1 only filters
    // noisy entries, it doesn't disable the rule.
    let t_bin = get_t_bin();
    let xdg = temp_xdg("autotune-step1-keep");

    // Produces 200 lines then fails — real failure, lines_raw > 0.
    for _ in 0..3 {
        let _ = Command::new(&t_bin)
            .env("XDG_DATA_HOME", &xdg)
            .env("L0_COMPRESSOR_GUARD", "0")
            .args(["--auto", "sh", "-c", "seq 1 200; exit 1"])
            .output()
            .unwrap();
    }

    let metrics = xdg.join("l0-compressor").join("metrics.jsonl");
    let body = std::fs::read_to_string(&metrics).expect("metrics exist");
    assert!(
        body.contains("\"adaptive_event\":\"expand_tail_err\""),
        "real failure with output should still fire: {body}"
    );

    let out = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--stats", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("valid JSON");
    assert!(
        v["auto_tuning"]["firings"].as_u64().unwrap() > 0,
        "expected real failures to fire expand; got {}",
        v["auto_tuning"]
    );
    // And the "real failure" runs produced output, so they aren't noisy.
    assert_eq!(v["auto_tuning"]["noisy"].as_u64(), Some(0));
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn auto_tuning_noisy_counter_only_marks_empty_failure_expansions() {
    // The noisy counter exists to flag false-positive expansions: the
    // `expand_tail_err` rule fired on a failing run with zero output (think
    // grep "no match"). Decay events are never noisy; expansions on failing
    // runs that DID produce output are not noisy either.
    let t_bin = get_t_bin();
    let xdg = temp_xdg("autotune-noisy");
    let dir = xdg.join("l0-compressor");
    std::fs::create_dir_all(&dir).unwrap();
    let body = concat!(
        // NOISY: expand_tail_err + exit!=0 + lines_raw==0.
        "{\"ts\":\"2026-06-05T10:00:00Z\",\"cmd\":\"grep\",\"args\":\"\",\"tokens_saved\":0,\"tokens_raw\":0,\"lines_raw\":0,\"lines_final\":0,\"truncated\":false,\"strategy\":\"passthrough\",\"exit_code\":1,\"duration_ms\":5,\"version\":\"0.1.10\",\"adaptive_event\":\"expand_tail_err\"}\n",
        // NOT noisy: expand_tail_err but produced output.
        "{\"ts\":\"2026-06-05T10:01:00Z\",\"cmd\":\"make\",\"args\":\"\",\"tokens_saved\":0,\"tokens_raw\":500,\"lines_raw\":120,\"lines_final\":80,\"truncated\":true,\"strategy\":\"head_tail\",\"exit_code\":2,\"duration_ms\":5,\"version\":\"0.1.10\",\"adaptive_event\":\"expand_tail_err\"}\n",
        // NOT noisy: decay_moderate (decay is never noisy by definition).
        "{\"ts\":\"2026-06-05T10:02:00Z\",\"cmd\":\"seq\",\"args\":\"\",\"tokens_saved\":900,\"tokens_raw\":1000,\"lines_raw\":500,\"lines_final\":60,\"truncated\":true,\"strategy\":\"head_tail\",\"exit_code\":0,\"duration_ms\":5,\"version\":\"0.1.10\",\"adaptive_event\":\"decay_moderate\"}\n",
        // NOT noisy: expand_tail_err with exit_code=0 (impossible in prod —
        // the rule only fires on failures — but defensive: we don't want a
        // forged record to be classified as noisy).
        "{\"ts\":\"2026-06-05T10:03:00Z\",\"cmd\":\"ok\",\"args\":\"\",\"tokens_saved\":0,\"tokens_raw\":0,\"lines_raw\":0,\"lines_final\":0,\"truncated\":false,\"strategy\":\"passthrough\",\"exit_code\":0,\"duration_ms\":5,\"version\":\"0.1.10\",\"adaptive_event\":\"expand_tail_err\"}\n",
    );
    std::fs::write(dir.join("metrics.jsonl"), body).unwrap();

    let out = Command::new(&t_bin)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--stats", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("valid JSON");
    assert_eq!(
        v["auto_tuning"]["firings"].as_u64(),
        Some(4),
        "all four rule-firing rows count"
    );
    assert_eq!(
        v["auto_tuning"]["noisy"].as_u64(),
        Some(1),
        "only the grep row is noisy"
    );
    let _ = std::fs::remove_dir_all(&xdg);
}

// ── Review-driven coverage: behaviors changed in the audit remediation ──────

/// L0_COMPRESSOR_NO_TELEMETRY=1 must suppress every telemetry write; a falsy value
/// must not.
#[test]
fn no_telemetry_env_suppresses_all_writes() {
    let t = get_t_bin();
    let xdg = temp_xdg("no-telemetry");
    let metrics = xdg.join("l0-compressor").join("metrics.jsonl");

    let out = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .env("L0_COMPRESSOR_NO_TELEMETRY", "1")
        .args(["echo", "silent"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        !metrics.exists(),
        "metric written despite L0_COMPRESSOR_NO_TELEMETRY=1"
    );

    let out = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .env("L0_COMPRESSOR_NO_TELEMETRY", "0")
        .args(["echo", "recorded"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(metrics.exists(), "falsy value must not disable telemetry");
    let _ = std::fs::remove_dir_all(&xdg);
}

/// An unknown leading flag is rejected (exit 2) BEFORE any metric is written —
/// it used to run via `sh -c`, fail silently, and pollute the stats forever.
#[test]
fn unknown_leading_flag_rejected_without_metric() {
    let t = get_t_bin();
    let xdg = temp_xdg("bogus-flag");
    let out = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--bogus-flag", "echo", "hi"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unrecognized option '--bogus-flag'"),
        "stderr: {stderr}"
    );
    assert!(
        !xdg.join("l0-compressor").join("metrics.jsonl").exists(),
        "no metric may be written for a rejected invocation"
    );
    let _ = std::fs::remove_dir_all(&xdg);
}

/// Invalid --since is an error in stats mode (it used to silently render
/// all-time data under a header claiming the window); a valid one succeeds.
#[test]
fn invalid_since_is_an_error_in_stats_mode() {
    let t = get_t_bin();
    let xdg = temp_xdg("since-invalid");
    let out = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .args(["--stats", "--since", "7days"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid --since value '7days'"), "{stderr}");

    let out = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .args(["--stats", "--since", "7d"])
        .output()
        .unwrap();
    assert!(out.status.success(), "valid window must not error");

    // Run mode: invalid value errors too; valid-but-ineffective warns.
    let out = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--since", "7D", "echo", "hi"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "invalid --since in run mode");
    let out = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["--since", "7d", "echo", "hi"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--since has no effect"),
        "expected ineffective-flag warning, got: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&xdg);
}

/// --reset-stats deletes BOTH metrics.jsonl and tuned.jsonl (stale adaptive
/// state used to survive and keep seeding runs).
#[test]
fn reset_stats_deletes_metrics_and_tuned() {
    let t = get_t_bin();
    let xdg = temp_xdg("reset-both");
    let dir = xdg.join("l0-compressor");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("metrics.jsonl"), "{}\n").unwrap();
    std::fs::write(dir.join("tuned.jsonl"), "{}\n").unwrap();
    let out = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .args(["--reset-stats"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        !dir.join("metrics.jsonl").exists(),
        "metrics must be deleted"
    );
    assert!(
        !dir.join("tuned.jsonl").exists(),
        "tuned must be deleted too"
    );
    let _ = std::fs::remove_dir_all(&xdg);
}

/// End-to-end recovery through the REAL main.rs wiring (BaseParams captured
/// pre-seed, seeded_by plumbed): decay a bucket on a 200-line workload, shrink
/// the workload (same args bucket!), and watch recover_defaults restore base.
#[test]
fn recovery_restores_base_through_main_wiring() {
    let t = get_t_bin();
    let xdg = temp_xdg("recovery-e2e");
    let file = xdg.join("data.txt");
    let write_lines = |n: usize| {
        let body: String = (1..=n).map(|i| format!("{}\n", i)).collect();
        std::fs::write(&file, body).unwrap();
    };
    let run = || {
        Command::new(&t)
            .env("XDG_DATA_HOME", &xdg)
            .env("L0_COMPRESSOR_GUARD", "0")
            .args(["cat", file.to_str().unwrap()])
            .output()
            .unwrap()
    };

    // 7 runs over 200 lines: truncated successes → decay persists a tune.
    write_lines(200);
    for _ in 0..7 {
        assert!(run().status.success());
    }
    let tuned_path = xdg.join("l0-compressor").join("tuned.jsonl");
    let tuned = std::fs::read_to_string(&tuned_path).unwrap();
    assert!(
        tuned.contains("decay"),
        "expected a decay tune, got: {tuned}"
    );

    // Workload shrinks below the threshold (same args → same bucket): after
    // 5 clean runs the 6th fires recover_defaults and restores 30/30.
    write_lines(80);
    for _ in 0..6 {
        assert!(run().status.success());
    }
    let tuned = std::fs::read_to_string(&tuned_path).unwrap();
    assert!(
        tuned.contains("recover_defaults"),
        "recovery did not fire through main wiring: {tuned}"
    );
    assert!(
        tuned.contains("\"head\":30") && tuned.contains("\"tail\":30"),
        "base not restored: {tuned}"
    );
    let _ = std::fs::remove_dir_all(&xdg);
}

/// The isolated tests write to THEIR xdg (pins both the metric write and the
/// isolation — dropping the .env() would silently resume polluting the real
/// metrics file with every cargo test run).
#[test]
fn isolated_run_writes_metric_into_its_own_xdg() {
    let t = get_t_bin();
    let xdg = temp_xdg("isolation-pin");
    let out = Command::new(&t)
        .env("XDG_DATA_HOME", &xdg)
        .env("L0_COMPRESSOR_GUARD", "0")
        .args(["sh", "-c", "exit 42"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(42));
    let metrics = xdg.join("l0-compressor").join("metrics.jsonl");
    let content = std::fs::read_to_string(&metrics).expect("metric must land in the temp xdg");
    assert!(content.contains("\"cmd\":\"exit\""));
    assert!(content.contains("\"args\":\"42\""), "inner args: {content}");
    let _ = std::fs::remove_dir_all(&xdg);
}

/// The command-rewriting wrapper's skip-list (Gemini CLI): zero-output builtins
/// are not wrapped, real commands are. Exercises the heredoc-generated wrapper
/// extracted from agent-hook.sh (the front line against zero-output telemetry
/// pollution).
#[test]
fn hook_wrapper_skips_builtins_and_wraps_commands() {
    // jq is a hard dependency of the wrapper; skip the test where absent.
    if Command::new("jq").arg("--version").output().is_err() {
        eprintln!("skipping: jq not installed");
        return;
    }
    let t = get_t_bin();
    let hook_src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("agent-hook.sh"),
    )
    .unwrap();
    let start = hook_src.find("<<'WRAP'").expect("WRAP heredoc start") + "<<'WRAP'".len();
    let end = hook_src[start..]
        .find("\nWRAP\n")
        .map(|i| start + i)
        .expect("WRAP heredoc end");
    let wrapper_body = &hook_src[start..end];

    let dir = temp_xdg("hook-wrapper");
    let wrapper = dir.join("wrapper.sh");
    // agent-hook.sh writes the agent's output program above the heredoc body.
    let gemini_output = "OUTPUT_JQ='{hookSpecificOutput: {tool_input: {command: $new}}}'\n";
    std::fs::write(
        &wrapper,
        format!("{gemini_output}{}", wrapper_body.trim_start()),
    )
    .unwrap();
    // Toggle file + PATH with the freshly built binary visible as `l0-compressor`.
    let cfg = dir.join("config");
    std::fs::create_dir_all(cfg.join("l0-compressor")).unwrap();
    std::fs::write(cfg.join("l0-compressor").join("hook.enabled"), "").unwrap();
    let bin_dir = dir.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::copy(&t, bin_dir.join("l0-compressor")).unwrap();
    let path_env = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let run_hook = |cmd: &str| -> String {
        use std::io::Write as _;
        let mut child = Command::new("bash")
            .arg(&wrapper)
            .env("XDG_CONFIG_HOME", &cfg)
            .env("PATH", &path_env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let payload = format!("{{\"tool_input\":{{\"command\":\"{}\"}}}}", cmd);
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    for skip in ["exit 42", "true", "wait", "trap - INT"] {
        let out = run_hook(skip);
        assert!(
            out.trim().is_empty(),
            "builtin '{skip}' must be skipped, got: {out}"
        );
    }
    let out = run_hook("seq 1 100");
    assert!(
        out.contains("l0-compressor --quiet --recover seq 1 100"),
        "real command must be wrapped, got: {out}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Writes the PostToolUse payload Claude Code sends after a successful Bash call.
fn claude_post_payload(stdout: &str) -> String {
    serde_json::json!({
        "session_id": "s",
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": "seq 1 500", "description": "numbers"},
        "tool_response": {
            "stdout": stdout,
            "stderr": "",
            "interrupted": false,
            "isImage": false,
            "noOutputExpected": false
        },
        "duration_ms": 7
    })
    .to_string()
}

fn run_claude_hook(
    xdg_config: &std::path::Path,
    xdg_data: &std::path::Path,
    stdin: &str,
) -> (i32, String, String) {
    use std::io::Write as _;
    let mut child = Command::new(get_t_bin())
        .arg("--claude-hook")
        .env("XDG_CONFIG_HOME", xdg_config)
        .env("XDG_DATA_HOME", xdg_data)
        .env("CLAUDE_CONFIG_DIR", xdg_config.join("claude"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `--claude-hook` end to end through the binary: silent and exit 0 unless the
/// toggle is on; with it on, a long output becomes a PostToolUse response in
/// the shape Claude Code accepts, and the run is recorded in the metrics.
#[test]
fn claude_hook_mode_end_to_end() {
    let dir = temp_xdg("claude-hook");
    let cfg = dir.join("config");
    let data = dir.join("data");
    let long: String = (1..=500).map(|i| format!("{i}\n")).collect();

    // Toggle off: no output whatever the payload.
    let (code, out, err) = run_claude_hook(&cfg, &data, &claude_post_payload(&long));
    assert_eq!((code, out.as_str(), err.as_str()), (0, "", ""));

    std::fs::create_dir_all(cfg.join("l0-compressor")).unwrap();
    std::fs::write(cfg.join("l0-compressor").join("hook.enabled"), "").unwrap();

    // Garbage, empty and short payloads: still silent, still exit 0.
    for stdin in ["", "not json", "{}", &claude_post_payload("1\n2\n3\n")] {
        let (code, out, _) = run_claude_hook(&cfg, &data, stdin);
        assert_eq!((code, out.as_str()), (0, ""), "stdin: {stdin:?}");
    }

    let (code, out, err) = run_claude_hook(&cfg, &data, &claude_post_payload(&long));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(err.is_empty(), "the hook must not write to stderr: {err}");
    let v: serde_json::Value = serde_json::from_str(out.trim()).expect("one JSON object");
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    let updated = &v["hookSpecificOutput"]["updatedToolOutput"];
    let stdout = updated["stdout"].as_str().unwrap();
    assert!(stdout.contains("lines omitted"), "{stdout}");
    assert!(stdout.contains("\n500\n"), "{stdout}");
    assert!(stdout.len() < long.len());
    assert_eq!(updated["stderr"], "");
    assert_eq!(updated["interrupted"], false);
    assert_eq!(updated["isImage"], false);

    let metrics =
        std::fs::read_to_string(data.join("l0-compressor").join("metrics.jsonl")).unwrap();
    let rows: Vec<serde_json::Value> = metrics
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "only the filtered call is recorded: {metrics}"
    );
    assert_eq!(rows[0]["cmd"], "seq");
    assert_eq!(rows[0]["strategy"], "claude_hook");
    let _ = std::fs::remove_dir_all(&dir);
}

/// claude-hook.sh install: registers `<abs bin> --claude-hook` under
/// PostToolUse, migrates away the old PreToolUse command-rewriting wrapper,
/// keeps unrelated hooks, and is idempotent; uninstall removes only ours.
#[test]
fn claude_hook_script_installs_posttooluse_and_migrates() {
    if Command::new("jq").arg("--version").output().is_err() {
        eprintln!("skipping: jq not installed");
        return;
    }
    let dir = temp_xdg("claude-hook-sh");
    let claude = dir.join("claude");
    std::fs::create_dir_all(claude.join("hooks")).unwrap();
    let legacy = claude.join("hooks").join("l0-compressor-wrapper.sh");
    std::fs::write(&legacy, "#!/bin/sh\n").unwrap();
    let settings = claude.join("settings.json");
    std::fs::write(
        &settings,
        serde_json::json!({
            "permissions": {"deny": ["Bash(git push *)"]},
            "hooks": {
                "PreToolUse": [
                    {"matcher": "Bash", "hooks": [{"type": "command", "command": legacy.to_str().unwrap()}]},
                    {"matcher": "Edit", "hooks": [{"type": "command", "command": "/x/other.sh"}]}
                ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("claude-hook.sh");
    let run = |action: &str| {
        let out = Command::new("bash")
            .arg(&script)
            .arg(action)
            .env("CLAUDE_CONFIG_DIR", &claude)
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("L0_COMPRESSOR_BIN", get_t_bin())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{action}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run("install");
    run("install");
    let s: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let post = s["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 1, "idempotent: {s}");
    assert_eq!(post[0]["matcher"], "Bash");
    let cmd = post[0]["hooks"][0]["command"].as_str().unwrap();
    assert_eq!(cmd, format!("\"{}\" --claude-hook", get_t_bin().display()));
    let pre = s["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre.len(), 1, "legacy rewrite removed, other hook kept: {s}");
    assert_eq!(pre[0]["matcher"], "Edit");
    assert!(!legacy.exists(), "legacy wrapper file removed");
    assert_eq!(s["permissions"]["deny"][0], "Bash(git push *)");

    run("uninstall");
    let s: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert!(s["hooks"].get("PostToolUse").is_none(), "{s}");
    assert_eq!(s["hooks"]["PreToolUse"][0]["matcher"], "Edit");
    let _ = std::fs::remove_dir_all(&dir);
}
