# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.1] - 2026-10-03

### Security
- **The Claude Code hook no longer rewrites commands.** Up to 0.3.0 it was a
  `PreToolUse` hook turning `cmd` into `l0-compressor --quiet --recover cmd`.
  Claude Code evaluates permission rules against the rewritten command, so
  `allow` rules stopped matching (commands that never prompted started
  prompting) and so did `deny`/`ask` rules: under `bypassPermissions`, a command
  matching a `deny` rule such as `Bash(seq *)` ran (reproduced on Claude Code
  2.1.285). Re-run `claude-hook.sh install` to migrate.
- Metrics now redact compound credential variable names in command prefixes
  (`GITHUB_TOKEN=…`, `AWS_SECRET_ACCESS_KEY=…`, `PGPASSWORD=…`); only bare names
  such as `PASSWORD=` were redacted before.

### Added
- **`l0-compressor --claude-hook`**: a Claude Code `PostToolUse` hook that
  filters the output of successful Bash calls through the CLI's pipeline and
  returns it as `updatedToolOutput`. The command, its permission check and its
  execution are untouched. Outputs Claude Code persisted (over ~30 KB) are
  filtered from the full file and the footer points at it; smaller truncated
  outputs get a private recovery file. Unknown payload shapes, failures,
  interrupted/image calls and short outputs are left alone. Hook runs are
  recorded with strategy `claude_hook`.

### Changed
- `claude-hook.sh install` registers `"<abs path>/l0-compressor" --claude-hook`
  under `PostToolUse` (absolute path, so it works in editors launched without
  the shell `PATH`), removes the old `PreToolUse` wrapper, and refuses a binary
  without the hook mode. `status` reports the registered binary's version and
  flags a leftover old hook. `agent-hook.sh install claude` hands off to it;
  the Gemini CLI integration is unchanged.
- Failures are not filtered by the hook: Claude Code's `PostToolUseFailure`
  cannot replace output. Auto-tuning does not learn from hook runs.

## [0.3.0] - 2026-07-07

### Added
- **Native Windows support (experimental).** The command is spawned directly
  via `CreateProcess` (no shell); stdout/stderr are merged by two drain
  threads feeding one channel (best-effort interleaving vs the kernel-exact
  `sh -c '… 2>&1'` merge on unix). Data dir resolves via `%LOCALAPPDATA%` /
  `%USERPROFILE%`. New CI job builds, unit-tests, and smoke-tests the native
  path (spawn, truncation, stderr merge, exit-code propagation); the release
  workflow now ships `l0-compressor-x86_64-pc-windows-msvc.zip`. E2E suite
  and process-spawning unit tests are unix-gated.
- **Tokenizer-measured benchmark.** `cargo run --release --example
  token_benchmark` replays captured real command outputs
  (`benchmarks/fixtures/`) through the binary and measures savings with
  tiktoken `o200k_base` / `cl100k_base`: 88–97% per output, 96.6% weighted
  (see `benchmarks/RESULTS.md`). tiktoken-rs is a dev-dependency only.

### Changed
- Docs toolchain upgraded to VitePress 2 (vite 8), clearing the Dependabot
  alerts on the docs dev server (GHSA-4w7w-66w2-5vf9 and related); stale
  esbuild override removed.

## [0.2.0] - 2026-07-07

### Changed — project renamed `l0-cache` → `l0-compressor`

The name never matched the tool: there is no cache. `l0-compressor` is what it
actually does — a universal command-output *compressor* (head/tail, collapse,
diff-aware filtering) — and matches the `l0_compressor` name the downstream
llmproxy plugin already uses. The rename is mechanical and preserves history;
old release tags (`v0.1.0`..`v0.1.15`) remain the frozen `l0-cache` artifacts.

- **Binary is now `l0-compressor`**, with `l0-comp` as the short alias (the
  legacy `t` alias is unchanged). The Homebrew formula, `install.sh`,
  `install-binary.sh`, and `make install` all create both aliases.
- **Environment variables are now `L0_COMPRESSOR_*`** (`_GUARD`,
  `_NO_TELEMETRY`, `_BIN_DIR`, `_VERSION`, `_GIT_HASH`). The two runtime knobs
  keep reading their **pre-rename `L0_CACHE_GUARD` / `L0_CACHE_NO_TELEMETRY`
  names as a deprecated fallback**, so existing hooks/integrations don't break;
  these fallbacks will be removed at the next major.
- **Data/config directories moved** to `…/l0-compressor/` (from
  `…/l0-cache/`). On first run the tool performs a one-time, **non-destructive**
  migration: it renames the legacy directory into place only when the new one
  does not yet exist, so accumulated `metrics.jsonl` / `tuned.jsonl` / `config.*`
  carry over. Nothing is ever deleted; if a new dir already exists the legacy
  one is left untouched.
- Homebrew tap/formula, repository URLs, docs, and completions updated to the
  new name (`brew tap fabriziosalmi/l0-compressor …`).

## [0.1.15] - 2026-07-01

Hardening / security / performance pass (three parallel audits, findings
verified against the code before fixing).

### Fixed
- **Crash on multibyte output (core-invariant violation).** `skip_timestamp`
  sliced arbitrary child output at fixed byte offsets (`&s[0..3]`, `&s[15..]`);
  a `€`/emoji/CJK char straddling offset 3 or 15 panicked, **aborting the
  wrapper and swallowing the child's real exit code** (verified: `exit 7`
  surfaced as `101`). Now char-boundary-safe via `get(..)`, so ordinary
  accented/emoji log lines can never crash the wrapper. Regression test added.
- **Recovery temp file hardened against a shared-`/tmp` symlink attack.** The
  `--recover` file lived at a predictable `$TMPDIR/l0-cache/recovery-<cmd>-<pid>.log`
  created with `File::create` (follows symlinks, default perms) — on a multi-user
  host an attacker could pre-plant a symlink to clobber a victim's file, and the
  un-redacted output (possibly secrets) was world-readable. Now the dir is a
  private `0700` (rejected if it's a symlink or another user's), and the file is
  opened `0600` with `O_NOFOLLOW`. Unix-only hardening; Windows temp is per-user.

### Performance (hot path — runs per output line on every filtered run)
- **`has_error_signal` computed once per line, not up to 14×.** `feed()` scanned
  each line against the 7-keyword set for the sticky-signal flag, again for
  `--only-errors`, and ran 2–3 more full-line phrase scans for auto-tune on
  *every clean line*. All consumers now share a single computed boolean (the
  auto-tune phrases are a subset of the keyword set, so gating them on it is
  exact).
- **Fuzzy line-collapse no longer allocates two `String`s per line.** The
  adjacent-line "fuzzy signature" comparison (the common non-matching case) now
  iterates both char streams in lockstep with zero allocation.
- **Binary detection no longer re-validates the growing 8 KB prefix per line**
  (was O(n²)); it validates only the new line, which is equivalent since lines
  split on ASCII `\n`.

### Notes
- Audited and confirmed correct (no change needed): `shell_escape`, secret
  redaction, ANSI/terminal sanitization (`safe_label`/`strip_ansi`), the `libc`
  `unsafe` blocks, file-lock/TOCTOU in the user's own data dir, and integer/
  memory bounds (`saturating_sub`, line/byte caps).

## [0.1.14] - 2026-07-01

### Added
- **Adversarial guard sandbox (161-case regression net).** A decision-level test
  (`sandbox_rm_rf_in_100_ways_is_always_blocked`) throws 161 destructive
  `rm -rf` spellings — every flag form × system root, path decorations, all HOME
  spellings, and `bash -c`/`sh -c` wrappers with chaining and quote-insertion —
  at the guard and asserts each is refused, plus a benign control set that must
  NOT be blocked. It calls only the guard's decision function, so it can never
  run a real `rm`. A companion test pins the known, documented lint limits.

### Fixed
- **Guard hardened against five obfuscation classes** the sandbox surfaced, each
  of which previously let a real `rm -rf` through:
  - **`..` traversal** — `rm -rf /etc/../etc`, `/etc/..`, `~/../alice` are now
    lexically resolved before matching (`/etc/../etc` → `/etc`).
  - **Home parents** — `rm -rf /home` and `rm -rf /Users` (which wipe *every*
    account at once) are now protected, incl. the Windows `C:\Users` / `/c/Users`
    spellings.
  - **Nested `sh -c`** — `bash -c "bash -c 'rm -rf /etc'"` is now unwrapped
    recursively (bounded depth).
  - **Wrapper / env prefixes** — `env rm -rf /etc`, `sudo rm -rf /root`, and
    `FOO=1 rm -rf /etc` are classified by their effective command.
  - **`~user`** — another account's home (`rm -rf ~root`) is now blocked.

### Notes
- Residual limits are unchanged in spirit and now documented explicitly: a
  static lint still cannot resolve command substitution / `eval`, glob
  expansion, targets arriving via stdin (`… | xargs rm -rf`), other destructive
  tools (`find -delete`, `dd`, `shred`), or symlinked aliases. The guard is a
  seatbelt against accidents, **not** a security boundary; `--no-guard` /
  `L0_CACHE_GUARD=0` still opt out.

## [0.1.13] - 2026-07-01

### Fixed
- **The destructive-command guard now protects the user's HOME, not just system
  roots.** Previously `rm -rf ~`, `rm -rf $HOME`, and `rm -rf ~/Documents`
  passed the guard and executed (a real incident): only `/`, `/etc`, `/usr`, …
  were covered. The recursive force-remove check now also blocks:
  - the HOME directory resolved at runtime from the environment
    (`$HOME` / `%USERPROFILE%` / `HOMEDRIVE`+`HOMEPATH`), both the exact path
    and its `…/*` glob;
  - its first-level data folders (Documents, Desktop, Downloads, Pictures,
    Movies, Music, and the Linux XDG equivalents Videos/Public/Templates);
  - literal, un-expanded `~`, `$HOME`, `${HOME}`, `%USERPROFILE%` references
    (in case they reach the `bash -c` re-scan without a shell expanding them).

  Coverage is cross-OS: Linux (`/home/<user>`, XDG dirs), macOS
  (`/Users/<user>`), and Windows (`C:\Users\<user>`, backslash separators,
  drive letters, and the `/c/Users` spelling used by Git Bash / MSYS) all
  normalize to a common comparison form.
- **Quote-insertion no longer bypasses the `bash -c` re-scan.** The `-c`
  payload is now tokenized with a minimal quote-aware de-obfuscator (removing
  `'…'`/`"…"` quotes and honoring `\` escapes) before matching, so
  `bash -c 'r"m" -"r"f /etc'` recomposes to `rm -rf /etc` and is blocked. The
  previous `split_whitespace` re-scan matched only the direct-argv form.

### Notes
- The guard remains a **best-effort lint, not a sandbox**: it does not perform
  variable expansion, command substitution, path canonicalization, or symlink
  resolution, and a determined caller can still evade it. Opt out as before with
  `--no-guard` / `L0_CACHE_GUARD=0`.

## [0.1.12] - 2026-06-23

### Added
- **Clean-success squelch.** On a zero exit with no error/warning signal
  anywhere in the stream, the displayed tail is trimmed to half (floored at 5
  lines) — a clean build/test/install's middle is progress noise. The head
  (command echo) is always kept and the final summary line always survives.
  Any `error`/`warn`/`fail`/`exception`/`panic`/`traceback`/`fatal` signal, or
  a non-zero exit, restores the full tail so failures stay completely visible.
  On by default; disable with `--no-squelch` or `squelch = false` (per-command
  config). The error-signal keyword set is now shared with the `--only-errors`
  filter so the two can never disagree.

### Fixed
- **The truncation banner now reports the *actual* tail shown**, not the
  configured cap: with the squelch active it reads `30 head + 15 tail`, not a
  fabricated `30 tail`. The runner reports the rendered `display_tail` so the
  banner can never overstate what survived.

### Changed
- **`src/telemetry/mod.rs` split into focused submodules** (`lock`, `paths`,
  `tuned`, `metric`, `adaptive`, `stats/{agg,render}`, `doctor`, `tests`),
  reducing a 5313-line file to a 71-line facade. No behaviour change.

## [0.1.11] - 2026-06-11

Full-findings remediation of the `--stats`/telemetry audit: every confirmed
bug fixed one at a time with a verification step, plus an honest-by-design
dashboard refresh.

### Fixed
- **Integration tests no longer pollute the real telemetry.** Four tests
  (`binary_output_does_not_hang`, `exit_code_propagation`, `pipe_to_head`,
  `integration_auto_flag_accepted`) spawned the real binary without
  `XDG_DATA_HOME` isolation — one `dd`/`exit`/`seq`/`echo hello` record per
  `cargo test` run landed in the user's `metrics.jsonl` (90%+ of the
  headline "Saved" was test artifacts) and even trained `tuned.jsonl` on
  benchmark workloads. All four are now isolated like the rest of the suite.
- **Unknown leading flags are rejected** (exit 2, no metric written). A typo
  like `l0-cache --nonexistant grep …` used to be swallowed as the command,
  fail completely silently (`sh` rejected it with the error going to a nulled
  stderr), and pollute the stats table with a flag-named row forever.
- **No-op auto-tuning triggers are no longer recorded as firings.** A
  floor-pinned decay (or ceiling-pinned expand) emitted one event per run,
  inflating the `Firings` counter ~13× for buckets sitting at the floor.
- **Invalid `--since` values are an error (exit 2) — BREAKING** for scripts
  that piped `--stats --json` with a bad window and relied on the silent
  all-time fallback (`--since 7days` showed all-time totals labeled
  "last 7days"). Validation is unconditional (run mode included), values
  are trimmed, a leading `+` is rejected, and a valid `--since` without
  `--stats`/`--discover` warns that it has no effect.
- **The low-value predicate is one function** shared by the `⚠ low` row
  marker, the footer hints, and `--discover` (they used to disagree), and
  zero-output commands (≥5 runs, 0 raw tokens — e.g. a hook wrapping `exit`)
  are finally flagged: "no output to compress — wrapping is pure overhead".
- **`--reset-stats` also deletes `tuned.jsonl`** — stale adaptive state used
  to keep seeding runs after a reset while `--stats` said "No metrics found".
- **`100.0%` efficiency is reserved for truly fully-elided output**; 99.97%
  used to round up to a fabricated perfect score (real dd ratio in the wild).
  Tampered records with `tokens_saved > tokens_raw` are clamped at ingestion,
  so the headline, the median, and `--json` agree on corrupt input instead
  of the text view clamping while the JSON leaked >100%.
- **Deterministic table order on ties** — the 0-saved rows reshuffled on
  every invocation (HashMap iteration order leaking through a single-key sort).
- **`format_tokens`/`format_number` unit boundaries** — 999,950+ rendered as
  `1000.0k` (7 chars, breaking the 6-wide cells); units now promote at the
  rounding boundary, with a `G` tier keeping the cell within 7 chars up to
  ~999.9G tokens.
- **`sh -c` records keep cmd and args on the same layer** — `sh -c "exit 42"`
  used to record `{"cmd":"exit","args":"-c exit 42"}` (inner cmd, outer
  args), also keying the learner's bucket on the wrong string. Inner args are
  now extracted (and secret-redacted) alongside the inner command.
  *Migration note*: existing `tuned.jsonl` entries and learner history for
  `sh/bash/zsh -c`-wrapped buckets are keyed on the old string and are
  abandoned (the TTL prunes them); tuning re-converges in 3-5 runs.
- **Secrets no longer leak through the `cmd` field.** `l0-cache API_KEY=x
  deploy` (or `sh -c 'PASSWORD=x deploy'`) recorded the assignment VERBATIM
  as the command name, bypassing redaction and rendering the secret in the
  --stats table. Leading `NAME=value` assignments are now skipped when
  resolving the command and pass through the args-side secret redaction.
- **`safe_label` no longer underflows on width 0** (debug-build panic guard).
- **The file lock is `flock(2)` on unix** instead of a mkdir sentinel whose
  10s mtime-break could steal a live lock and whose rotation window could
  drop a concurrent append; kernel-released on crash. The flock protocol
  uses its own `<file>.flock` path so pre-flock binaries keep their mkdir
  lock working during a mixed-version window (no per-run stall), and
  `--reset-stats` cleans up lock artifacts. Non-unix keeps the mkdir
  fallback; `--doctor` probes the protocol actually in use.
- **`tuned.jsonl` is compacted on write** (latest entry per bucket, atomic
  temp+rename with a unique temp name) — it was append-only, one line per
  firing, contradicting its own "one line per bucket" doc comment. When the
  lock is unavailable or the file is unreadable, the writer falls back to a
  plain append of its own entry instead of rewriting the whole file from a
  bad snapshot (which could silently drop other buckets' tunes). Compaction
  is destructive for unparseable lines and expired/garbage-timestamp
  entries: they are pruned, not carried forward.

### Added
- **`recover_defaults` rule — the un-ratchet.** Every other rule only moves
  head/tail down and `tail_error` up, compounding forever. After 5
  consecutive clean (success, non-truncated) runs, a bucket seeded by a
  truncation-driven decay is restored to its configured base, and an
  expanded `tail_error` returns to base once failures stop.
  `proactive_shrink` tunes are deliberately not recovered — a clean streak
  is exactly their supporting evidence (no flip-flop); every other seed
  (including a tag overwritten by a later expand or a partial recovery)
  remains recoverable, so a bucket can never get stuck below base until the
  TTL. Rendered in the breakdown, the mix (`R:`), and `--json`
  (`recover_defaults`).
- **30-day TTL on persisted tunes** — `lookup_tuned` ignores (and compaction
  prunes) entries older than the metrics housekeeping window, so a tune from
  a long-gone workload can't seed today's runs. Timestamps more than a day
  in the future count as expired too (a corrupted far-future entry would
  otherwise be immortal), and the TTL is applied during the scan so a
  garbage later line can't mask a fresh entry.
- **`L0_CACHE_NO_TELEMETRY=1`** skips all `metrics.jsonl`/`tuned.jsonl`
  writes — belt-and-braces for test harnesses and benchmark scripts.
- **`Median/run` headline row** (unweighted median per-run efficiency) next
  to the token-weighted gauge, and a **dominance disclosure** ("`cmd`
  accounts for N% of savings") whenever one command holds >50% — one dd
  benchmark made a 77%-real-world day read as 98.3%.
- **The IMPACT bar is a real second axis**: filled by the command's share of
  total tokens saved (sqrt-scaled so small rows stay visible), colored by
  efficiency. It used to re-plot the same percentage as the EFFIC. column.
- **Legend for the auto-tuning mix** (`E=expand Dm/Ds/Dsy=decay P=shrink
  R=recover`), a `last <date>` annotation on the `noisy ⚠` counter (stale
  pre-fix history vs. live problem), and a `noisy_last_seen` JSON field.
- **`est. tokens` unit label** on the Saved row, **`↑ most saved`** replaces
  the ambiguous `↑ best` (and `⚠ low` now wins over it on row 0), and a dim
  **`(n<5)`** qualifier explains red rows that escape the ⚠ only because of
  the 5-run sample gate.
- **Terminal-aware layout**: the box grows with the terminal (COMMAND column
  absorbs the width, 10→24 cols) and visible-width math accounts for
  double-width (CJK) characters end-to-end — name cells pad and truncate by
  display columns, not chars. Red now starts below the 10% hint threshold so
  row color and `⚠` agree (one shared constant).
- **Rendering is unit-testable**: `--stats` builds a string
  (`render_stats_text`) instead of printing line-by-line; 10 new rendering
  tests cover the 100% boundary, markers, footers, dominance, legend, and
  wide-terminal behavior; hook skip-lists gain the zero-output builtins
  (`exit`, `true`, `false`, `:`, `wait`, `trap`).

## [0.1.10] - 2026-06-10

Self-learning audit + iterative fixes. `--stats` gains an `AUTO-TUNING` section
that reports, honestly, when the adaptive rules fire and when they don't —
prior versions had the mechanism but no visibility into it. Five surgical
changes, each measurable against the same metrics file before/after:

### Added
- **`AUTO-TUNING` section in `--stats`** and `auto_tuning` object in
  `--stats --json` — per-event firing counts (`expand_tail_err`,
  `decay_moderate`, `decay_strong`, `proactive_shrink`, `decay_steady`),
  a `noisy` counter (failure-expansions that fired on zero-output runs —
  the false-positive surface), top-cmds breakdown. No behavior change here;
  this is the baseline visibility every subsequent fix is measured against.
- **`adaptive_event` field in `metrics.jsonl`** records which rule branch
  fired per run (or absent when none fired). Old records without the field
  parse cleanly.
- **`args_hash` field in `metrics.jsonl`** — 8-char FNV-1a 64-bit hash of the
  redacted args string (no new crate). The adaptive learner now keys on
  `(cmd, args_hash)` instead of `cmd` alone, so `curl https://api.openai.com`
  and `curl https://example.com` no longer pollute each other's streak.
- **`proactive_shrink` rule** — fires when a bucket has ≥20 records, all
  clean (success + not truncated), and `max(lines_raw) + 5` ≤ half the
  current head+tail budget. Apply head=max+5, tail=`default_tail/4`,
  bounded by `auto_floor`. Max-based on purpose: never introduces a new
  truncation vs. observed history.
- **`decay_steady` rule** — window-adaptive complement to the consecutive
  decay rules. Looks at the last 20 records of the bucket: if ≥80% are
  truncated successes (regardless of streak), shrink head/tail by 30%.
  Catches the steady-state truncation pattern that "5 consecutive"
  misses when an occasional non-truncated run breaks the streak.
- **`tuned.jsonl` persistence sidecar** at
  `$XDG_DATA_HOME/l0-cache/tuned.jsonl`. After each rule firing, the
  resulting `(head, tail, tail_error)` is saved per `(cmd, args_hash)`
  bucket. The next run of the same bucket starts from that tune instead
  of the CLI defaults — so the decay rules **compound**: one bucket's
  head can shrink 30 → 24 → 19 → 11 → 10 (`auto_floor`) over four
  firings, instead of resetting to 24 each time. Best-effort I/O,
  fail-safe; corrupt or missing file degrades to the previous "no
  persistence" behavior.

### Changed
- **Adaptive failure-expand no longer triggers on no-output runs.** A
  `exit_code != 0 && lines_raw == 0` record (e.g. `grep` "no match",
  `find` "not found") used to grow `consecutive_failures` and fire
  `expand_tail_err`, wasting tokens to expand a tail that was zero
  bytes to begin with. Such entries now break the streak instead of
  contributing to it — `--stats`' `noisy` counter trends toward zero
  on this kind of usage.
- Pre-0.1.10 metric records (no `adaptive_event` / `args_hash` fields)
  remain countable by `--stats` (totals and per-cmd savings) but are
  excluded from the adaptive learner so legacy data can't re-introduce
  the pre-Step-2 noise. Graceful degradation, both directions.

## [0.1.9] - 2026-06-08

### Added
- **Homebrew now ships the integration scripts**, not just the binary. The formula
  installs the standalone setup tools as namespaced commands —
  `l0-cache-claude-hook`, `l0-cache-agent-hook`, `l0-cache-agent-rules` — so the
  transparent Claude Code / Gemini CLI hook can be set up without cloning the repo
  (`l0-cache-claude-hook install && l0-cache-claude-hook enable`, or
  `l0-cache-agent-hook install gemini` for Gemini CLI). Claude Code and Gemini CLI
  are now managed the same way out of the box.
- `jq` is now a Homebrew runtime dependency (the hook managers use it to edit the
  agent's `settings.json`), so the hook works out of the box after `brew install`.

### Changed
- `claude-hook.sh` is now behavior-identical to `agent-hook.sh` for Claude Code:
  the generated wrapper enables `--recover` (full output saved to a temp file when
  a failing command is truncated) and extracts the command with the same robust
  `.tool_input.command // .toolInput.command` path. Existing installs pick this up
  on the next `install`; the public CLI is unchanged.

## [0.1.8] - 2026-06-05

### Added
- **Prebuilt release binaries — install without a Rust toolchain.** A CI workflow
  builds and attaches macOS (arm64 + x64) and Linux (x64, static musl) binaries —
  each with a SHA-256 checksum — to every tag's GitHub Release. `install-binary.sh`
  downloads and verifies the right one for your platform:
  `curl -fsSL …/install-binary.sh | sh`.
- **Homebrew** (`Formula/l0-cache.rb`):
  `brew tap fabriziosalmi/l0-cache https://github.com/fabriziosalmi/l0-cache` then
  `brew install l0-cache`.
- Cargo.toml `repository`/`homepage` metadata.

## [0.1.7] - 2026-06-05

### Added
- **Transparent multi-format config — still zero new dependencies.** The optional
  per-command config file is now auto-detected as
  `config.{json,toml,yaml,yml,conf,ini}` in `$XDG_CONFIG_HOME/l0-cache/` (or
  `~/.config/l0-cache/`). JSON is parsed strictly via serde; TOML/YAML/INI share a
  small built-in flat parser (the schema is flat, so all three styles reduce to the
  same shape). `[*]` is an alias for `[defaults]`; unknown keys and unparseable
  lines are skipped. No new crates were added.

## [0.1.6] - 2026-06-05

Hardening pass from a full file-by-file/test-by-test audit.

### Fixed
- **Terminal-injection hardening.** `--stats` and `--discover` now strip control
  characters from the (user-writable) metrics file's command names, so a crafted
  `metrics.jsonl` can no longer drive the terminal with raw ANSI escapes. `--json`
  was already safe (serde escapes them).
- **Recovery file is now PID-scoped** (`recovery-<cmd>-<pid>.log`), so concurrent
  runs of the same command (multi-agent, `make -j`, parallel CI) no longer collide
  on and corrupt one shared file. A mid-stream I/O error no longer leaves a partial
  recovery file behind.
- **Diff-context collapsing is byte-bounded**, not just line-bounded — restoring the
  strict bounded-memory guarantee (1 MB-capped lines could otherwise buffer GBs). The
  hunk-header detector now requires the space real `@@ … @@` headers have (no false
  activation on `@@@@`).
- **Stats robustness:** `--cost-per-mtok inf`/`nan` no longer emit `null` cost fields
  in `--json`; displayed efficiency is clamped to 100% (a corrupt file can't show
  500%); `--discover` truncates long command names so columns stay aligned.
- **Shell installers:** `help` no longer leaks script internals (`set`/vars/dividers);
  `agent-rules.sh` no longer accumulates a blank line per install→remove cycle; the
  hook installers remove their `jq` scratch file even on an early-exit failure.

### Added
- Tests for all of the above, plus the previously-uncovered stats path (`pct`/`usd`/
  `cost_shown`, control-char sanitization), the DiffCollapse↔pipeline integration,
  and config precedence (explicit CLI flag > config > default). 205 unit + 21
  integration tests.

## [0.1.5] - 2026-06-05

### Added
- **Format-aware diff compression.** A new streaming filter stage collapses long
  runs of *unchanged context* in unified diffs (keeping file/hunk headers and every
  `+`/`-` line) to `… (N unchanged diff lines) …`. It activates only after a real
  `@@ … @@` hunk header, so non-diff output — even indented text — is never touched.
- **`--discover`** — an optimization advisory from your metrics: which prefixed
  commands are paying off (keep), which to consider dropping (low savings over
  enough runs), and the biggest raw-token footprint.
- **`--json`** — emit `--stats` as a single JSON object (totals + per-command array)
  for tooling.
- **`--cost-per-mtok <N>`** — when > 0, show estimated USD cost saved in `--stats`
  and `--discover` (and `usd_saved` in `--json`).
- **`agent-rules.sh`** — installs a project rule ("prefix noisy commands with
  `l0-cache`") for agents whose hook cannot rewrite a command (Cursor, Cline,
  Copilot, Codex). Best-effort prompt-injection, complementing the transparent
  `agent-hook.sh` (Claude Code, Gemini CLI).
- README "How it compares" section positioning l0-cache against rtk / snip / Lean Ctx.

## [0.1.4] - 2026-06-05

### Added
- **`--recover`**: on a failing command whose output was truncated, the full
  un-truncated output is saved to a temp file and the banner points the agent at
  it, so it can read the omitted middle without re-running. Lazy (no disk for
  small/under-threshold output), memory- and size-bounded, and fail-safe (any I/O
  error is ignored). Off by default.
- **Per-command config file** at `$XDG_CONFIG_HOME/l0-cache/config.json` (or
  `~/.config/l0-cache/config.json`): optional per-command overrides for `head`,
  `tail`, `tail_error`, `threshold`, `only_errors`, and `recover`, with a
  `defaults` block. Precedence is explicit CLI flag > config > built-in default;
  commands match by resolved name; malformed/unknown keys are ignored gracefully.
- **`agent-hook.sh`**: a generalized transparent-hook installer covering the
  agents whose hook API can rewrite a command — **Claude Code** (`PreToolUse`) and
  **Gemini CLI** (`BeforeTool`/`run_shell_command`) — with the same conservative,
  fail-safe wrapper (and `--recover` enabled). `claude-hook.sh` stays as a
  Claude-only convenience. Cursor's hook can only allow/deny (no rewrite), so it
  cannot be wrapped transparently.

## [0.1.3] - 2026-06-05

### Added
- **Color/UI overhaul.** `--stats` is now a dense, boxed dashboard (summary card +
  per-command table with proportional efficiency bars and `↑ best` / `⚠ low`
  markers), and `--doctor` is re-skinned to share the same boxed visual language.
- **`NO_COLOR` / TTY color guard.** `--stats` and `--doctor` emit ANSI only on an
  interactive terminal; piping or redirecting now yields clean, escape-free text.
  `NO_COLOR` (any value) and `TERM=dumb` force color off; `FORCE_COLOR` /
  `CLICOLOR_FORCE` force it on for CI captures.
- README status badges (CI, latest release, license, MSRV) and a declared
  `rust-version = "1.85"` so the MSRV is enforced by cargo.

### Changed
- `benchmark.sh` rewritten with a boxed header and a comparison table (lines, size,
  estimated tokens, reduction %); `install.sh` and `claude-hook.sh` now honor the
  `NO_COLOR` / TTY color guard.

## [0.1.2] - 2026-06-04

### Added
- `claude-hook.sh`: an optional, off-by-default transparent integration for
  Claude Code. It installs a `PreToolUse` hook that routes the *simple* Bash
  commands Claude Code runs through `l0-cache`, so the model never has to prefix
  anything. Conservative (compound/piped/redirected/interactive/stateful commands
  pass through untouched), fail-safe (any error or a missing `l0-cache`/`jq`
  leaves the command unchanged, and it never sets a `permissionDecision`), and
  toggleable at runtime with no restart (`install`/`enable`/`disable`/`status`/
  `uninstall`). See the Claude Code Integration guide.

### Fixed
- Memory is now bounded on a giant newline-free stream (e.g. a minified bundle).
  Line reading no longer uses `read_until`, which buffered the entire line before
  the 1 MB cap could apply; a chunked reader keeps at most ~1 MB and drains the
  rest (still counted for accurate `bytes_raw`). A new RSS stress test pushes
  100 MB as a single line and asserts the resident set stays far below it.

## [0.1.1] - 2026-06-04

### Fixed
- **`--tail-error` / failure backoff now actually works**: the tail buffer retains
  `max(--tail, --tail-error)` lines while streaming and trims at render based on
  exit code, instead of an after-the-fact expansion that did nothing. Also fixed a
  latent silent gap when the tail wrapped below the threshold.
- **`--raw` is now verbatim**: progress-bar/backspace squashing and big-JSON
  truncation are applied in filtered mode only; raw keeps content unchanged.
- **Binary output** is no longer silently truncated to ~8 KB with `truncated:false`;
  it now carries an explicit "binary output detected — showing first N of M bytes"
  banner and `truncated:true`.
- **Metrics `bytes_raw`** is measured from true pre-transform bytes, so reported
  token savings are no longer understated.
- **Metrics rotation** preserves recent history (it was rewriting the pruned log
  into a never-read `.old` file) and is size-capped to avoid re-rotating every run.
- **Safety Guard** is no longer bypassed by `sh -c "rm -rf /etc"` (it now scans the
  shell `-c` payload) or by trailing-slash/glob path variants.
- **`print_stats`** no longer panics on a multibyte command name in the metrics file.
- **Signals**: the captured child runs in its own process group; SIGINT/SIGTERM are
  forwarded to it (a directed `kill`/`timeout`/`docker stop` now terminates the
  child instead of being swallowed), and the `--idle-timeout` watchdog kills the
  whole group rather than only the `sh` wrapper.
- **Installer**: the `curl | bash` one-liner installs correctly (it was a silent
  no-op on the empty-argument path) and pins to the latest release tag.

### Added
- Safety Guard documentation, `L0_CACHE_GUARD` truthy/falsy parsing, and a unified
  enable/disable decision shared by enforcement and `--doctor`.
- Credential redaction for the metrics `args` field (passwords, tokens, auth
  headers, URL userinfo).
- Documentation for the previously undocumented flags (`--guard`/`--no-guard`,
  `--only-errors`, `--idle-timeout`, `--quiet`, `--reset-stats`) and an installer
  CI job (shellcheck + regression checks).

## [0.1.0] - 2026-06-04

### Added
- Rebranded CLI proxy under the name `l0-cache`.
- Single-threaded, synchronous Rust engine for process execution.
- Universal output filtering pipeline:
  - ANSI escape stripping.
  - Squeezing of consecutive blank lines.
  - Identical line collapsing.
  - Prefix-based line collapsing (e.g. for compiler progress).
- Head and tail buffer rendering with a safety floor of 10 lines.
- Failure backoff tail expansion (up to 1000 lines) to prevent AI loops.
- Consecutive success token savings decay (20% and 40% reductions).
- Restricted permission checks on local database metrics log file (`chmod 0600`).
- Local metrics reporting system with `XDG_DATA_HOME` overrides and automated rotation at 10 MB.
- Isolated E2E integration tests and LCG pseudo-random fuzzer testing.
- Static musl cross-compilation pipeline for Alpine and dynamic builds for Linux/macOS.
- Shell completions script generator for Bash, Zsh, Fish, Elvish, and PowerShell.
- VitePress documentation site.
