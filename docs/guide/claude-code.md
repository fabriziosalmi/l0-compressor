# Claude Code Integration

`l0-compressor` is normally invoked explicitly — you, or your AI assistant, prefix a
command with `l0-compressor`. For [Claude Code](https://claude.com/claude-code), the
bundled `claude-hook.sh` removes that step: it registers a
[`PostToolUse`](https://code.claude.com/docs/en/hooks#posttooluse) hook that
filters the output of every successful Bash call before Claude reads it, so the
model never has to prefix anything.

The integration is **opt-in and off by default**. It never rewrites your shell,
your aliases, or the commands Claude Code runs: it only changes the text Claude
reads after a command has finished.

## Install

`claude-hook.sh` ships in the repository root (Homebrew installs it as
`l0-compressor-claude-hook`). Installation requires
[`jq`](https://jqlang.github.io/jq/) to edit `settings.json`, and an
`l0-compressor` with the `--claude-hook` mode (0.3.1 or later).

```sh
./claude-hook.sh install     # register the hook (idempotent)
./claude-hook.sh enable      # turn it ON
```

Then **start a new Claude Code session** so the hook is loaded — Claude Code
reads hooks from `settings.json` at session startup. After that first load, the
`enable`/`disable` toggle takes effect immediately, with no restart.

The hook is registered in `~/.claude/settings.json`, which the Claude Code CLI,
the VS Code extension and the desktop app all read, so one install covers all
three. It is registered with the **absolute path** of the binary (`command -v
l0-compressor`, or `$L0_COMPRESSOR_BIN`), because an editor launched from the
Dock or a desktop launcher may not have `~/.local/bin` on its `PATH`. Re-run
`install` if the binary moves.

## Commands

| Command | Effect |
|---|---|
| `install` | Register a `PostToolUse` (matcher `Bash`) hook running `"<abs path>/l0-compressor" --claude-hook` in `settings.json`. Idempotent; removes the pre-0.3.1 `PreToolUse` wrapper; saves a timestamped backup. |
| `enable` / `on` | Create the toggle file `~/.config/l0-compressor/hook.enabled`. Instant. |
| `disable` / `off` | Remove the toggle file. Instant. |
| `status` | Show the registered command, the hook binary's version, the on/off state, and whether the old `PreToolUse` hook is still present. |
| `uninstall` / `remove` | Remove the hook registration (and the old wrapper, if any). |

The script honors `$CLAUDE_CONFIG_DIR` (default `~/.claude`) and `$XDG_CONFIG_HOME`
(default `~/.config`).

## How it works

After a Bash call succeeds, Claude Code runs `l0-compressor --claude-hook` with
the call — command and captured output — as JSON on stdin. If the output is
longer than the truncation threshold, the hook runs it through the same filter
pipeline as the CLI (ANSI stripping, line collapsing, diff-aware context
collapsing, head/tail with the clean-success squelch) and prints a response
whose `updatedToolOutput` replaces what Claude reads. Otherwise it prints
nothing and Claude reads the original output.

Head, tail, threshold, `only_errors` and `squelch` come from your
[configuration file](./configuration), looked up by the command's name exactly
as for an explicit `l0-compressor <command>` run.

### Permissions are untouched

Because the hook runs after the command, it cannot change what runs or whether
it is allowed:

- your `allow`, `ask` and `deny` rules match the real command, as without
  l0-compressor;
- read-only commands Claude Code approves on its own (`ls`, `cat`, `git log`, …)
  stay approved without extra rules;
- the hook never sets a permission decision.

### Nothing is lost

When the output is truncated, the footer says where the full text is, so Claude
can read the omitted lines without re-running the command:

- outputs over ~30 KB, which Claude Code already saves under
  `~/.claude/projects/…/tool-results/`, are filtered from that full file (not
  from the 30 KB excerpt in the payload), and the footer points at it;
- smaller outputs are saved to a private recovery file (`0700` dir, `0600`
  file, symlinks refused) under the system temp dir.

### What is left alone

The hook prints nothing — leaving Claude Code's output as is — when:

| Case | Why |
|---|---|
| Output under the threshold (100 lines by default) | Nothing worth cutting |
| The command failed (non-zero exit) | Claude Code fires `PostToolUseFailure`, whose output cannot be replaced |
| Interrupted, image or background calls | Not plain captured text |
| A `tool_response` field the hook does not know | Rebuilding the response would drop it |
| Binary output, or output already filtered by an explicit `l0-compressor` call | Nothing to gain |
| The toggle is off, or the payload is malformed | Fail-safe |

### Fail-safe

Any unexpected payload, I/O error or panic makes the hook exit `0` with no
output, so Claude Code keeps the original output. The hook never blocks a
command and never writes to stderr.

## Upgrading from 0.3.0 or earlier

Up to 0.3.0 the integration was a `PreToolUse` hook that rewrote `cmd` into
`l0-compressor --quiet --recover cmd`. Claude Code evaluates permission rules
against the rewritten command, which had two effects:

- `allow` rules (`Bash(git log *)`) stopped matching, so commands that never
  prompted started prompting;
- `deny` and `ask` rules (`Bash(git push *)`) stopped matching too. In
  `bypassPermissions` mode a denied command ran.

Run `install` again (or `l0-compressor-claude-hook install`): it removes the old
hook and wrapper and registers the new one. `status` reports the old hook if it
is still registered.

## Limits

- **Failures are not filtered.** Claude Code's hook API cannot replace the output
  of a failed call, which is where long error logs come from. Prefix a noisy
  command explicitly (`l0-compressor cargo test`) when you want its failures
  filtered too.
- **No auto-tuning from hook runs.** The adaptive rules learn from explicit
  `l0-compressor` runs; hook runs use the configured or default parameters.
- **The safety guard does not apply**, since `l0-compressor` does not launch the
  command. Claude Code's permission rules are the control.

## Verifying it works

In a session started after `install` + `enable`, have Claude run a command with
plenty of output (for example `seq 1 500`) without prefixing `l0-compressor`.
With the hook live, Claude reads the first and last lines, an
`... [N lines omitted for LLM] ...` marker, and an `l0-compressor` footer. Then:

```sh
./claude-hook.sh status        # registration, hook binary, on/off
l0-compressor --stats          # savings; hook runs have strategy "claude_hook"
```

## Other agents

**Gemini CLI**: `agent-hook.sh install gemini` registers a `BeforeTool` hook that
rewrites simple commands into `l0-compressor --quiet --recover <cmd>` before they
run. It changes the command Gemini sees, so check that your Gemini policy rules
still match the rewritten form. `agent-hook.sh install claude` (the default)
hands off to `claude-hook.sh`.

```sh
./agent-hook.sh install gemini
./agent-hook.sh enable            # shared on/off toggle
./agent-hook.sh status gemini
./agent-hook.sh uninstall gemini
```

**Cursor and most other agents** expose a `beforeShellExecution`-style hook that
can only *approve or block* a command, not change it or its output. For those,
`agent-rules.sh` drops a project rule telling the model to prefix noisy read-only
commands with `l0-compressor`:

```sh
./agent-rules.sh install cursor   # or: cline | copilot | codex
./agent-rules.sh print            # just print the snippet to paste anywhere
./agent-rules.sh remove cursor
```

This is **best-effort** (the model may ignore it), not a hard hook — it writes a
project-level rule file (e.g. `.cursor/rules/l0-compressor.mdc`,
`.github/copilot-instructions.md`, `AGENTS.md`). A prefixed command is what the
agent's permission rules see, so allow `l0-compressor <cmd>` forms explicitly
where your agent asks for approval.

## Uninstall

```sh
./claude-hook.sh disable     # stop filtering immediately (keeps the install)
./claude-hook.sh uninstall   # remove the registration entirely
```

`uninstall` saves a timestamped backup of `settings.json` before editing it.
Restart Claude Code to drop the hook from a running session.
