# l0-compressor

[![CI](https://github.com/fabriziosalmi/l0-compressor/actions/workflows/ci.yml/badge.svg)](https://github.com/fabriziosalmi/l0-compressor/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/fabriziosalmi/l0-compressor?sort=semver&color=blue)](https://github.com/fabriziosalmi/l0-compressor/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-green)](LICENSE)
[![MSRV](https://img.shields.io/badge/rust-1.85%2B-orange?logo=rust)](Cargo.toml)

A CLI proxy written in Rust that reduces LLM token consumption by filtering,
truncating, and compressing command output. Prefix any command with
`l0-compressor` (or enable the transparent Claude Code / Gemini CLI hook) and
the output an AI coding agent reads shrinks by 50–80% on typical mixed
sessions (telemetry estimate) and 88–97% on noisy outputs (tokenizer-measured
— see [Benchmarks](#benchmarks)), while exit codes, error tails, and output
ordering are preserved. Runs on macOS and Linux; native Windows support is
experimental (prebuilt zip from v0.3.0).

Documentation: [fabriziosalmi.github.io/l0-compressor](https://fabriziosalmi.github.io/l0-compressor/)

<p align="center">
  <img src="docs/public/screenshot.png" alt="l0-compressor --stats telemetry dashboard" width="560">
</p>

## Comparison

The category — compress command output before an AI coding agent reads it —
has several active implementations with different mechanisms. The tables below
cover the tools that operate at the same layer as l0-compressor (a wrapper or
hook around shell commands). Facts verified against each project's repository
on 2026-07-07; star counts are approximate and will drift. Savings figures are
each project's own claim, measured with different methodologies — they are not
directly comparable.

### Scope and mechanism

| Tool | ★ | Lang | License | Mechanism | Coverage | Unknown commands | Claimed savings |
|---|---|---|---|---|---|---|---|
| **l0-compressor** | — | Rust | MIT | generic streaming filters (head/tail, collapse, diff/JSON-aware) | any command | ✅ same behavior | 88–97% on noisy outputs (o200k-measured, [reproducible](benchmarks/RESULTS.md)); 50–80% session-wide (estimate) |
| [rtk](https://github.com/rtk-ai/rtk) | ~69k | Rust | Apache-2.0 | per-command parsers, 4 strategies | 100+ commands | partial (passthrough) | 60–90% |
| [snip](https://github.com/edouard-claude/snip) | ~360 | Go | MIT | declarative YAML filters, 19 pipeline actions | 127 filters | partial | 60–90% |
| [lean-ctx](https://github.com/yvgude/lean-ctx) | ~3.1k | Rust | Apache-2.0 | shell hook + MCP server (81 tools) + wire proxy | 95+ patterns + file reads | partial | 60–90% |
| [squeez](https://github.com/claudioemmanuel/squeez) | ~160 | Rust | Apache-2.0 | hook pipeline + cross-call dedup (MinHash) + MCP | 100+ handlers | partial | 89% (own benchmark suite) |
| [sqz](https://github.com/ojuschugh1/sqz) | ~390 | Rust | ELv2¹ | per-command formatters + repeated-read dedup | 40+ formatters | partial | 24.7% avg (3k real runs) |
| [lowfat](https://github.com/zdk/lowfat) | ~550 | Rust | Apache-2.0 | pipe filter, 3 intensity levels + file-content compression | 6 curated filters + plugins | partial | 38–96% per command |
| [chop](https://github.com/AgusRdz/chop) | ~40 | Go | MIT | per-command filters | 52+ commands | partial | 50–90% |
| [omni](https://github.com/fajarhide/omni) | ~300 | Rust | MIT | noise filter + local session memory (SQLite) | generic + memory | ✅ | 70–90% |

¹ Elastic License 2.0 is not an OSI-approved open-source license.

### Operational features

| Tool | Adaptive tuning | Blocks destructive cmds | Secret redaction | Full-output recovery | Local stats | Agent integrations | Native Windows |
|---|---|---|---|---|---|---|---|
| **l0-compressor** | ✅ 6 rules, persisted per `(cmd, args)` | ✅ guard, 161-case adversarial suite | ✅ telemetry args | ✅ `--recover` temp file | dashboard / JSON / `--discover` / `--doctor` | Claude Code + Gemini CLI hooks; rules files for Cursor/Cline/Copilot/Codex | experimental¹ |
| rtk | ❌ (static `discover`) | ❌ (exclusion lists) | partial (AWS) | ✅ tee on failure | ✅ + opt-in remote telemetry | 10+ agents | ✅ |
| snip | ❌ | ❌ | ❌ | ❌ | ✅ SQLite | 2 hooks + prompt injection for others | ❌ |
| lean-ctx | ✅ per-file read modes | guard layer | ✅ | ✅ 5 retrieval paths | ✅ signed ledger | 30+ agents | ✅ |
| squeez | ✅ context-pressure scaling | bypass only² | ❌ | ✅ blob store + retrieve | ✅ | 5 hosts | ✅ |
| sqz | ❌ | ❌ | ✅ entropy-based | ✅ `sqz expand` | ✅ | 7+ agents | ✅ |
| lowfat | ❌ | ❌ | ❌ | ❌ | ✅ | Claude Code, OpenCode | unverified |
| chop | ❌ | ❌ | ❌ | ❌ | ✅ `chop gain` | 4 agents | ✅ |
| omni | memory only | ❌ | ❌ | ✅ hash → store | ✅ | 6 agents | ✅ |

¹ Native Windows support is experimental: shipped as a prebuilt zip since v0.3.0, built and smoke-tested in CI, but with less field mileage than the unix builds. See [Windows support](#windows-support-experimental).
² squeez routes risky commands around compression; it does not refuse to run them.

### Adjacent layers (different mechanism, same goal)

These compress context at the API-wire or MCP level rather than wrapping shell
commands; some bundle a command-output layer:

| Tool | ★ | Layer | Notes |
|---|---|---|---|
| [headroom](https://github.com/headroomlabs-ai/headroom) | ~57k | API proxy / library / MCP | AST-aware code compression, trained model; bundles rtk for shell output |
| [context-mode](https://github.com/mksglu/context-mode) | ~19k | MCP sandbox | output executes in a sandbox, never enters context; BM25 retrieval |
| [token-optimizer](https://github.com/alexgreensh/token-optimizer) | ~1.6k | hook archiving | outputs >4 KB archived and replaced with a retrieval pointer; PolyForm-NC license |
| [entroly](https://github.com/juyterman1000/entroly) | ~420 | API proxy / MCP | rank-select-compress with auditable "context receipts" |
| [tamp](https://github.com/sliday/tamp) | ~90 | localhost API proxy | 9-stage pipeline, default secret redaction |

Generic prompt compressors (Microsoft LLMLingua and similar) and
memory/codebase-index MCP servers solve a different problem and are omitted.

### Where l0-compressor is behind

- Per-command parsers (rtk, snip, chop) produce higher ratios and better
  semantic output on the commands they cover — e.g. grouped test failures.
  Generic filters do not match that on known commands.
- Windows support is newer and less field-tested than the others in the
  ✅ column, which have shipped Windows binaries for longer.
- No cross-call deduplication, file-read compression, MCP server, or session
  memory (squeez, sqz, lean-ctx, omni have one or more of these).
- The built-in `--stats` telemetry estimates tokens as bytes ÷ 4; only the
  offline [benchmark](benchmarks/RESULTS.md) is tokenizer-measured.
- The smallest install base in the table.

### Where l0-compressor differentiates

- Identical behavior on unknown and proprietary CLIs, with no parser or filter
  set to maintain.
- Adaptive auto-tuning with persistence: six rules adjust `(head, tail,
  tail_error)` per `(cmd, args_hash)` bucket and compound across runs — see
  [Adaptive auto-tuning](#adaptive-auto-tuning). Only lean-ctx and squeez have
  comparable learning behavior, at other layers.
- A destructive-command guard with an adversarial regression suite; most tools
  in the table have no refusal mechanism at all.
- Documented, tested hardening: bounded memory, signal/process-group
  forwarding, symlink-safe recovery files, multibyte-safe line transforms.
- Conservative telemetry accounting: local-only, credential redaction, and a
  dashboard that discloses when a single command dominates the savings.

## How it works

AI coding agents read the full output of every shell command they run. Most of
it is noise: passing-test lines, progress bars, repeated build output. The
relevant information concentrates in the first lines (headers, command echo)
and the last lines (errors, summary, exit status).

`l0-compressor` wraps the command and applies a pipeline of generic streaming
filters:

```
command output
  --> line normalization (progress bars, backspace/bell, giant JSON lines)
  --> ANSI escape stripping
  --> unified-diff context collapsing (keep changes, drop unchanged runs)
  --> line collapsing (identical & prefix-based) (×N)
  --> blank line squeezing
  --> head/tail buffering (30 + 30 lines)
  --> clean-success squelch (exit 0 + no error signal: trim the tail further)
  --> metrics logging
  --> filtered output
```

**Clean-success squelch:** on a zero exit with no error/warning signal anywhere
in the output, the tail window is trimmed further — the middle of a clean
build/test/install is progress noise, and 30 tail lines are rarely needed to
confirm success. The head (command echo) is always kept, the summary line
always survives, and the moment any `error`/`warn`/`fail`/`panic`/… signal
appears — or the command exits non-zero — the full tail is restored. Disable
with `--no-squelch` or `squelch = false` in config.

## Benchmarks

Savings measured with real tokenizers (tiktoken `o200k_base` / `cl100k_base`)
on captured real command outputs, replayed through the release binary at
default settings — not the bytes÷4 estimate the built-in telemetry uses:

| Output | Lines | o200k tokens | saved |
|---|---|---|---|
| `cargo test` (354 tests) | 373 → 22 | 5360 → 189 | 96.5% |
| `cargo build --release` (clean, 33 crates) | 30 → 2 | 387 → 27 | 93.0% |
| `git log --stat -120` | 1390 → 66 | 18439 → 807 | 95.6% |
| `git show` (code diff) | 429 → 42 | 5203 → 445 | 91.4% |
| `npm ls --all` | 222 → 42 | 4388 → 536 | 87.8% |
| service log dump, 5100 lines, exit 1 | 5100 → 156 | 161759 → 4716 | 97.1% |
| **Total (weighted)** | | 195536 → 6720 | **96.6%** |

These numbers describe noisy outputs (the corpus is committed for
reproducibility); short outputs below the truncation threshold pass through
and session-wide savings depend on your command mix — the telemetry reports
50–80% on typical sessions. Reproduce with
`cargo run --release --example token_benchmark`; details in
[benchmarks/RESULTS.md](benchmarks/RESULTS.md).

## Installation

### Prebuilt binary (no Rust needed)

Download a prebuilt binary for your platform (macOS arm64/x64, Linux x64) from the
latest release and install it to `~/.local/bin/` (with the `l0-comp` and `t` aliases):

```sh
curl -fsSL https://raw.githubusercontent.com/fabriziosalmi/l0-compressor/master/install-binary.sh | sh
```

### Homebrew (macOS / Linux)

```sh
brew tap fabriziosalmi/l0-compressor https://github.com/fabriziosalmi/l0-compressor
brew install l0-compressor
```

### From source (non-interactive)

Builds with Rust/Cargo and installs to `~/.local/bin/`:

```sh
curl -fsSL https://raw.githubusercontent.com/fabriziosalmi/l0-compressor/master/install.sh | bash
```

### Interactive Installer

Clone the repository and run the setup script for a guided interactive install:

```sh
git clone https://github.com/fabriziosalmi/l0-compressor.git
cd l0-compressor
./install.sh
```

### Manual Build

```sh
cargo build --release
cp target/release/l0-compressor /usr/local/bin/
```

### Verify

```sh
l0-compressor --version
# l0-compressor 0.3.0 (abc1234)
```

> **Upgrading from l0-cache:** the project was renamed in 0.2.0. On first run
> the new binary migrates the data and config directories non-destructively
> (`…/l0-cache/` → `…/l0-compressor/`); metrics, adaptive tunes, and config
> carry over. `L0_CACHE_GUARD` / `L0_CACHE_NO_TELEMETRY` are still read as
> deprecated fallbacks until the next major version.

## Usage

```sh
# Filtered output (default: 30 head + 30 tail, threshold 100 lines)
l0-compressor cargo test

# Full output, metrics still logged
l0-compressor --raw cargo test

# Interactive commands pass through unchanged
l0-compressor -i vim file.txt

# Token savings report
l0-compressor --stats
l0-compressor --stats --since 7d

# Custom head/tail
l0-compressor --head 50 --tail 50 cargo build

# More tail lines on error (default: 120)
l0-compressor --tail-error 200 cargo test

# Auto-tuning is enabled by default. To disable it:
l0-compressor --no-auto cargo test

# Diagnose system installation, shell configuration, and active LLM editors
l0-compressor --doctor

# Custom success optimization floor and failure backoff ceiling
l0-compressor --auto-floor 15 --auto-ceiling 500 cargo test

# Custom token divisor ratio (e.g. 8 bytes per token)
l0-compressor --token-factor 8 cargo test
```

## Options

```
--raw                Print output verbatim (no head/tail truncation, no collapsing
                     or JSON squashing); ANSI is still stripped and a 1 MB/line and
                     256 MB total OOM cap still apply. Metrics are still logged.
-i, --interactive    Passthrough mode (stdin/stdout/stderr inherited)
--head N             Lines to keep from start (default: 30)
--tail N             Lines to keep from end (default: 30)
--tail-error N       Tail lines on non-zero exit (default: 120)
--threshold N        Only truncate if output exceeds N lines (default: 100)
--no-squelch         Disable the clean-success squelch (keep the full tail on clean exits)
--only-errors        Keep only lines matching error/warn/fail/panic/exception/etc.
--recover            On a failing command whose output was truncated, save the full
                     output to a temp file and point to it in the banner (so the agent
                     can read the omitted lines without re-running). Off by default.
--idle-timeout N     SIGKILL the command (and its process group) after N seconds with
                     no output (prevents interactive-prompt deadlocks). 0 = off.
--no-auto            Disable adaptive auto-tuning of parameters
--quiet, -q          Suppress l0-compressor's own stderr notices (e.g. auto-tuning)
--guard              Force-enable the safety guard (see "Safety Guard" below)
--no-guard           Force-disable the safety guard
--doctor             Diagnose system installation, shell environment, and active LLM editors
--auto-floor N       Floor for success optimization decay (default: 10)
--auto-ceiling N     Ceiling for failure backoff tail expansion (default: 1000)
--token-factor N     Divisor for token estimation (default: 4)
--stats              Show token savings report
--since DURATION     Filter stats/discover (e.g. 7d, 24h, 30m)
--discover           Show an optimization advisory (keep / drop / footprint) from metrics
--json               Output --stats as JSON instead of the dashboard
--cost-per-mtok N    USD per million tokens; when > 0, show cost saved in --stats/--discover
--reset-stats        Delete ALL recorded telemetry (destructive, cannot be undone)
--completions SHELL  Generate shell completions (bash, zsh, fish, elvish, powershell)
--version            Print version with git commit hash
```

## Telemetry Dashboard

`l0-compressor --stats` renders an aggregated savings report — total runs, tokens
saved, per-command efficiency with proportional bars, and an `AUTO-TUNING`
section that reports per-event firing counts plus a `noisy` counter (failure
expansions that fired on zero-output runs — the false-positive surface):

```
┌─ l0-compressor TELEMETRY ───────────────────────────────── last 7d ─┐
│ Runs        35                                                 │
│ Saved       12.5k  of 17.4k raw · est. tokens                  │
│ Efficiency   71.7%  █████████████████░░░░░░░                   │
│ Median/run   65.2%  unweighted                                 │
│             cargo accounts for 66% of savings                  │
├────────────────────────────────────────────────────────────────┤
│ COMMAND     RUNS   SAVED  EFFIC. IMPACT                        │
│ cargo         15    8.2k   78.4% ██████████░░  ↑ most saved    │
│ git           12    3.1k   65.2% ██████░░░░░░                  │
│ npm            8    1.2k   54.2% ████░░░░░░░░                  │
├────────────────────────────────────────────────────────────────┤
│ AUTO-TUNING                                                    │
│ Firings     8  22.9% of 35 runs                                │
│   expand_tail_err    1   decay_mod   2   decay_strong   3      │
│   proactive_shrink     1   decay_steady   1   recover   0      │
│   noisy     0   0.0% of firings                                │
│ Top cmds (by firings)                                          │
│   cargo         5   Dm:2 Ds:3                                  │
│   git           2   P:1 Dsy:1                                  │
│   npm           1   E:1                                        │
│   E=expand Dm/Ds/Dsy=decay P=shrink R=recover                  │
└────────────────────────────────────────────────────────────────┘
```

The headline pairs the token-weighted **Efficiency** with the unweighted
**Median/run**, and discloses when a single command holds >50% of the savings
— so one huge benchmark can't quietly dress up the average. The **IMPACT** bar
is each command's share of all tokens saved (sqrt-scaled), colored by its own
efficiency. `⚠ low` flags commands worth un-prefixing (≥5 runs, <10% saved, or
no output at all); `(n<5)` marks low rows still below the sample gate.

Add `--json` to emit the same data as a single object (including the
`auto_tuning` block) for tooling.

`--doctor` shares the same boxed visual language for its health report. Color is
emitted only on an interactive terminal — piping or redirecting (or setting
`NO_COLOR`) yields clean, escape-free text; `FORCE_COLOR=1` forces it on for CI
captures.

## Adaptive auto-tuning

Enabled by default; disable with `--no-auto`. Six rules adjust `head`,
`tail`, and `tail_error` per `(cmd, args_hash)` bucket, where `args_hash` is
an FNV-1a hash of the (redacted) args string — so `curl https://api.x.com`
and `curl https://api.y.com` learn independently.

| Event tag | When it fires | What it does |
|---|---|---|
| `expand_tail_err` | ≥1 consecutive recent failure with `lines_raw > 0` | Grows `tail_error` by `(1 + streak) ×`, capped by `--auto-ceiling` (default 1000). |
| `decay_moderate` | 3-4 consecutive truncated successes | Shrinks `head` and `tail` by 20%, floored by `--auto-floor` (default 10). |
| `decay_strong` | 5+ consecutive truncated successes | Shrinks `head` and `tail` by 40%, same floor. |
| `recover_defaults` | 5 consecutive clean (success + not truncated) runs on a bucket tuned away from its base | The un-ratchet: restores `head`/`tail` to the configured base (unless the tune came from `proactive_shrink`, which a clean streak confirms) and an expanded `tail_error` back to base, in one firing. |
| `proactive_shrink` | ≥20 records in the bucket, all clean (success + not truncated), `max(lines_raw) + 5` ≤ half the current head+tail budget | Sets `head = max(lines_raw) + 5`, `tail = default_tail / 4`. Max-based: never introduces a new truncation vs. observed history. |
| `decay_steady` | ≥20 records in the bucket, all success, ≥80% truncated | Shrinks `head` and `tail` by 30%. Complements `decay_moderate/strong`: catches steady-state truncation when the streak is broken by occasional non-truncated runs. |

A failing run with **zero output** (e.g. `grep` "no match", `find` "not
found") does **not** grow the `expand_tail_err` streak — its failure mode is
not the kind that extra error context would help with. The `noisy` counter
in `--stats` tracks any past firings that did happen on such runs.

Each firing is **persisted** to `$XDG_DATA_HOME/l0-compressor/tuned.jsonl` keyed
by `(cmd, args_hash)` (compacted on write: one line per bucket, 30-day TTL).
The next run of the same bucket starts from the saved
`(head, tail, tail_error)` instead of the CLI defaults — so the decay rules
compound: one bucket's `head` can shrink 30 → 24 → 19 → 11 → 10 (`--auto-floor`)
over four firings, and `recover_defaults` walks it back to base once the
workload changes. Best-effort I/O; a missing or corrupt `tuned.jsonl`
degrades silently to the no-persistence behavior.

## Per-command configuration (optional)

There is no config file by default. When you want different head/tail budgets per
command — without per-tool parsers — drop a small file in
`$XDG_CONFIG_HOME/l0-compressor/` (or `~/.config/l0-compressor/`). l0-compressor auto-detects
`config.{json,toml,yaml,yml,conf,ini}` — **transparent multi-format, zero extra
dependencies** (JSON is parsed by serde; TOML/YAML/INI share a tiny flat parser):

```toml
# config.toml  (or config.json / config.yaml — same flat schema)
[defaults]
recover = true

[cargo]
tail_error = 300
head = 50

[git]
head = 10
tail = 40
```

Tunable keys per command: `head`, `tail`, `tail_error`, `threshold`, `only_errors`,
`recover`. Commands are matched by resolved name (so `sh -c "cargo test"` matches
`cargo`); `[defaults]` / `[*]` apply to every command. Precedence is **explicit CLI
flag > config > built-in default**, and auto-tuning then adjusts from the resolved
base. A missing/malformed file is ignored (one stderr note for bad JSON; the flat
formats skip unparseable lines), never fatal.

## Claude Code Integration (optional)

Normally you (or your AI assistant) prefix a command with `l0-compressor` explicitly.
For [Claude Code](https://claude.com/claude-code), the bundled `claude-hook.sh`
does it **transparently**: it registers a
[`PostToolUse`](https://code.claude.com/docs/en/hooks#posttooluse) hook that
filters the output of every successful Bash call before Claude reads it — the
model never has to prefix anything, and works the same in the CLI, the VS Code
extension and the desktop app (they share `~/.claude/settings.json`).

The command itself is **never rewritten**. It runs, is permission-checked, and
is recorded exactly as it would be without l0-compressor; the hook only replaces
the text Claude reads afterwards (`updatedToolOutput`). Your `allow`, `ask` and
`deny` rules keep matching the real command, and read-only commands Claude Code
approves on its own (`ls`, `cat`, `git log`, …) stay approved.

- **Only when it pays** — output under the truncation threshold (100 lines by
  default) is left exactly as Claude Code captured it.
- **Nothing is lost** — when output is truncated, the footer points at a full
  copy: Claude Code's own saved file for outputs over ~30 KB, otherwise a
  private (`0600`) recovery file.
- **Fail-safe** — any unexpected payload, I/O error or panic produces no
  response, and Claude reads the original output.
- **Runtime toggle** — enable/disable instantly, no restart.

Limits, set by Claude Code's hook API: a command that **fails** (non-zero exit)
fires `PostToolUseFailure`, whose output cannot be replaced, so failures reach
Claude unfiltered; and auto-tuning does not learn from hook runs.

```sh
./claude-hook.sh install     # register the hook (idempotent; needs jq)
./claude-hook.sh enable      # turn it ON  (instant)
./claude-hook.sh disable     # turn it OFF (instant)
./claude-hook.sh status      # show registration, hook binary version, on/off state
./claude-hook.sh uninstall   # remove the hook registration
```

> **Installed via Homebrew?** The same script ships as the `l0-compressor-claude-hook`
> command — use it instead of `./claude-hook.sh` (no clone needed):
>
> ```sh
> l0-compressor-claude-hook install && l0-compressor-claude-hook enable
> ```

> **Activation:** after `install` (or any change to `settings.json`), start a
> **new** Claude Code session so the hook is loaded — hooks are read at session
> startup. The `enable`/`disable` toggle then takes effect immediately.

The hook is registered with the **absolute path** of `l0-compressor` (or
`$L0_COMPRESSOR_BIN`), so it runs even where `PATH` differs, such as the VS Code
extension launched from the Dock; re-run `install` if the binary moves. The
script honors `$CLAUDE_CONFIG_DIR` and `$XDG_CONFIG_HOME`, edits Claude Code's
`settings.json` (saving a timestamped backup) and stores its on/off state as an
empty toggle file at `~/.config/l0-compressor/hook.enabled`.

> [!WARNING]
> **Upgrading from 0.3.0 or earlier:** the previous integration was a `PreToolUse` hook
> that rewrote `cmd` into `l0-compressor … cmd`. Claude Code evaluates
> permission rules against the rewritten command, so `allow` rules stopped
> matching and — under `bypassPermissions` — a command matching a `deny` rule
> ran anyway. Re-run `install`: it removes the old hook and registers the new
> one. `status` flags the old hook if it is still present.

> [!NOTE]
> `l0-compressor` is not a persistent cache — it filters output on the fly and does
> not store results to replay. Hook runs appear in `l0-compressor --stats` (strategy
> `claude_hook` in the JSON). If a session shows no savings, it was started before
> the hook was installed or enabled, or its outputs were all short — confirm with
> `./claude-hook.sh status`.

### Other agents (Gemini CLI)

For Gemini CLI, `agent-hook.sh` installs a conservative, fail-safe wrapper on
`BeforeTool`/`run_shell_command` that **rewrites** simple commands into
`l0-compressor --quiet --recover <cmd>` before they run. Unlike the Claude Code
integration it changes the command Gemini sees, so check that your Gemini
policy rules still match the rewritten form. `agent-hook.sh install claude`
(the default) hands off to `claude-hook.sh`.

```sh
./agent-hook.sh install gemini    # or: install claude   (default → claude-hook.sh)
./agent-hook.sh enable            # shared on/off toggle for all installed agents
./agent-hook.sh status gemini
```

> **Homebrew:** these ship as `l0-compressor-agent-hook` and `l0-compressor-agent-rules`
> (same arguments), so no clone is needed.

> **Cursor** and most other agents expose a hook that can only *allow/deny* a
> command, not rewrite it, so they cannot be wrapped transparently. For those,
> `agent-rules.sh install cursor|cline|copilot|codex` drops a project rule telling
> the model to prefix noisy read-only commands with `l0-compressor` (or
> `agent-rules.sh print` to paste it anywhere). This is best-effort (model-dependent),
> not a hard hook.

## Safety Guard

When `l0-compressor` detects it is running inside an AI coding assistant (Claude Code,
Gemini CLI, Cursor/VS Code terminals), it enables a **best-effort** guard that
blocks a few obviously destructive commands before they run, exiting with code
**126**:

- recursive force-removal of a critical system path (`rm -rf /`, `/etc`, `/usr`, …)
  or of the user's HOME and its first-level data folders (`rm -rf ~`, `$HOME`,
  `~/Documents`, …) — including inside `sh -c "…"` wrappers (unwrapped
  recursively), with `..`-traversal, trailing-slash/glob, `env`/`sudo`/`VAR=x`
  prefix, and `~user` variants;
- reverse shells / socket redirections (`/dev/tcp`, `/dev/udp`);
- credential exfiltration (`curl`/`wget`/`nc`/`ssh` touching `id_rsa`, `.env`, `shadow`, …);
- `DROP DATABASE` via `psql`/`mysql`/`sqlite3`/`sqlcmd`.

A 161-case adversarial test suite pins these decisions in CI (it exercises only
the guard's decision function, so it never runs a real `rm`). Documented
residual limits: command substitution / `eval`, glob expansion, targets via
stdin (`… | xargs rm -rf`), other destructive tools (`find -delete`, `dd`,
`shred`), symlinked aliases.

The guard runs only when `l0-compressor` itself launches the command. The
[Claude Code integration](#claude-code-integration-optional) filters output
*after* Claude Code ran the command, so there the guard never applies — Claude
Code's own permission rules are the control.

Control it explicitly with `--guard` / `--no-guard`, or the `L0_COMPRESSOR_GUARD`
environment variable (`1`/`true`/`on` to force on, `0`/`false`/`off` to force off).
Precedence: `--no-guard` → `--guard` → `L0_COMPRESSOR_GUARD` → auto-detect.

> This is a guard rail, not a sandbox. It pattern-matches argv and shell payloads
> and can be bypassed by a determined caller — do not rely on it as a security
> boundary. Bypass an intentional command with `--no-guard`.

## Architecture

Single-threaded, synchronous design. Zero async. The only thread ever spawned is
an optional output-inactivity watchdog, and only when `--idle-timeout` is set.

```
l0-compressor <command>
  |
  +-- sh -c '<command> 2>&1'     # merge stderr into stdout
  |
  +-- read_line_lossy()          # UTF-8 lossy, 1MB line cap
  |
  +-- FilterPipeline             # streaming, O(head+tail) memory
  |     |-- strip_ansi()
  |     |-- DiffCollapse
  |     |-- CollapseLines
  |     |-- WhitespaceSqueeze
  |     +-- HeadTailBuffer
  |
  +-- write_output()             # BrokenPipe-safe
  |
  +-- append_metric()            # JSONL, O_APPEND, 0600 perms
  |
  +-- exit(child_exit_code)      # 128+N for signal-killed processes
```

### Memory Model

- Filtered mode: O(head + tail) lines in memory, regardless of output size
- Raw mode: capped at 256 MB, then truncation with warning
- Line length: capped at 1 MB per line to prevent OOM on binary input

### Signal Handling

- The captured child runs in its own process group. `l0-compressor` installs SIGINT
  and SIGTERM handlers that **forward** the signal to that group, so Ctrl-C and a
  directed `kill <pid>` (or `timeout`, systemd, `docker stop`) terminate the whole
  child subtree — not just the `sh` wrapper — and `l0-compressor` then propagates the
  child's status.
- SIGPIPE: ignored in `l0-compressor`, BrokenPipe handled in code so metrics are logged
  before exit
- Exit codes: POSIX 128+N convention for signal-killed children

## Metrics

Each invocation logs a JSON line to `~/.local/share/l0-compressor/metrics.jsonl`:

```json
{
  "ts": "2026-06-10T10:30:00Z",
  "cmd": "cargo",
  "args": "test --all",
  "bytes_raw": 15000,
  "bytes_final": 3000,
  "tokens_raw": 3750,
  "tokens_final": 750,
  "tokens_saved": 3000,
  "lines_raw": 500,
  "lines_final": 62,
  "truncated": true,
  "strategy": "head_tail",
  "exit_code": 0,
  "duration_ms": 1234,
  "version": "0.3.0",
  "adaptive_event": "decay_moderate",
  "args_hash": "a1b2c3d4"
}
```

`adaptive_event` and `args_hash` are present from 0.1.10 on and may be absent
(both fields are `Option<String>` with `skip_serializing_if = None`); older
records parse cleanly without them.

The adaptive learner also reads/writes a small sidecar at
`$XDG_DATA_HOME/l0-compressor/tuned.jsonl` — one JSON line per `(cmd, args_hash)`
bucket, compacted on write. See [Adaptive auto-tuning](#adaptive-auto-tuning).

Data directory resolution: `$XDG_DATA_HOME/l0-compressor/` then `$HOME/.local/share/l0-compressor/`
then `/etc/passwd` lookup (for containers, cron, systemd).

File permissions are set to 0600. `metrics.jsonl` auto-rotates at 10 MB
(entries older than 30 days are pruned at rotation); `tuned.jsonl` is
compacted on write (one line per bucket, 30-day TTL). `--reset-stats`
deletes both. Set `L0_COMPRESSOR_NO_TELEMETRY=1` to skip all telemetry writes
for an invocation (useful for test harnesses and benchmarks).

## Cross-Platform Support

| Environment | Build target | Status |
|---|---|---|
| macOS arm64/x86_64 | native | Tested |
| Ubuntu 22.04 / 24.04 | `x86_64-unknown-linux-gnu` | Tested |
| Alpine Linux | `x86_64-unknown-linux-musl` | Static binary |
| LXC container | same as host | `/etc/passwd` fallback |
| Proxmox VE | same as host | Works in host and guests |
| cron / systemd | same as host | `/etc/passwd` fallback |
| SSH with PTY | same as host | Signals work normally |
| SSH without PTY | same as host | See known limitations |
| Windows 10/11 | `x86_64-pc-windows-msvc` | Experimental (CI-built and smoke-tested) |

### Windows support (experimental)

The binary builds and runs natively on Windows (`x86_64-pc-windows-msvc`,
built and smoke-tested in CI; `l0-compressor-x86_64-pc-windows-msvc.zip` is
attached to releases since v0.3.0). The mechanics differ from unix in ways
that matter:

- **No shell involved.** The command is spawned directly via `CreateProcess`;
  stdout and stderr are merged by two drain threads. Interleaving between the
  two streams is best-effort (per read chunk) rather than the kernel-exact
  order `sh -c '… 2>&1'` gives on unix. For shell syntax, invoke it
  explicitly: `l0-compressor cmd /C "a | b"`.
- **Ctrl-C** is delivered by the console to the whole process group (default
  console semantics); there is no signal forwarding. The `--idle-timeout`
  watchdog uses `taskkill /T`.
- **Data dir** resolves to `%XDG_DATA_HOME%`, then `%LOCALAPPDATA%`, then
  `%USERPROFILE%\.local\share`. The 0600/0700 permission hardening and the
  `--recover` symlink defenses are unix-only (Windows temp and profile dirs
  are per-user by default).
- The E2E test suite is unix-gated; Windows coverage is the unit suite plus
  a CI smoke test (native spawn, truncation, stderr merge, exit-code
  propagation).

### Cross-Compilation from macOS

```sh
# Prerequisites
brew install filosottile/musl-cross/musl-cross
rustup target add x86_64-unknown-linux-gnu
rustup target add x86_64-unknown-linux-musl

# Build
make build-linux    # glibc, dynamic
make build-alpine   # musl, fully static

# Deploy
make deploy-linux HOST=user@server
make deploy-alpine HOST=user@alpine-host
```

## Shell Completions

```sh
# Bash
l0-compressor --completions bash > /etc/bash_completion.d/l0-compressor

# Zsh
l0-compressor --completions zsh > ~/.zsh/completions/_l0-compressor

# Fish
l0-compressor --completions fish > ~/.config/fish/completions/l0-compressor.fish
```

## Known Limitations

- **SSH without PTY**: when running `ssh host l0-compressor cargo build` (no `-t` flag),
  Ctrl-C may not reach the child process. Use `ssh -t host l0-compressor cargo build`
  instead.

- **Binary output**: detected on the first 8 KB. If a command produces text
  followed by binary data after 8 KB, the binary portion is processed as text
  (with UTF-8 lossy conversion).

- **Shell requirement**: `l0-compressor` requires `/bin/sh` or `/usr/bin/sh` for the
  `2>&1` merge. In distroless containers without a shell, use `l0-compressor -i` for
  passthrough mode (no stderr merge, no filtering).

## Hardening

The following protections are in place for production use across diverse
environments:

- UTF-8 lossy reads: never drops lines on invalid encoding
- Multibyte-safe line transforms: char-boundary-safe slicing throughout
  (a `€`/emoji/CJK char straddling a transform offset used to crash the
  wrapper and swallow the child's exit code; regression-tested since 0.1.15)
- Line length cap: 1 MB per line prevents OOM on binary/minified input
- Raw mode cap: 256 MB prevents OOM on massive output
- SIGPIPE handling: metrics are always logged, even when piped to `head`
- Exit code 128+N: POSIX-correct for signal-killed processes
- `$HOME` fallback: `/etc/passwd` lookup for containers, cron, systemd
- Metrics rotation: auto-rename to `.old` at 10 MB
- File permissions: 0600 on metrics file; credential-shaped values in
  recorded `args` are redacted before writing
- `--recover` temp file hardened against shared-`/tmp` symlink attacks:
  private `0700` directory, `0600` file opened with `O_NOFOLLOW`
- Shell check: clear error when `/bin/sh` is missing

See [Hardening](https://fabriziosalmi.github.io/l0-compressor/internals/hardening.html)
in the docs for the threat-by-threat table.

## Development

```sh
cargo test           # 308 unit + 46 E2E integration tests
cargo clippy         # 0 warnings enforced
cargo build --release
```

## License

MIT. See [LICENSE](LICENSE).
