# Introduction

`l0-compressor` is a lightweight CLI proxy that sits between your AI coding assistant and
shell commands. It filters, truncates, and compresses command output to reduce
token consumption by 50-80%.

## Why

AI coding assistants (Claude Code, Gemini CLI, Cursor) read the full output of
every shell command they execute. Most of this output is noise:

- Hundreds of passing test lines when only the failures matter
- ANSI color codes that consume tokens but carry no semantic value
- Repeated identical lines from build tools
- Blank lines that waste token budget

The useful information is almost always concentrated in the first few lines
(headers, command echo) and the last few lines (errors, summary, exit status).

## How

Instead of running:

```sh
cargo test
```

Run:

```sh
l0-compressor cargo test
```

The output is filtered through a streaming pipeline:

1. **ANSI stripping** -- removes color codes, cursor movement, etc.
2. **Line normalization** -- progress bars collapsed to their final state
   (interior `\r`), backspace/bell resolved, giant single-line JSON payloads
   truncated
3. **Line collapsing** -- identical and same-prefix runs become one line with
   a `(×N)` count
4. **Diff context collapsing** -- inside unified diffs, long runs of unchanged
   context lines are collapsed; changed lines are always kept
5. **Blank line squeezing** -- consecutive blanks reduced to one
6. **Head/tail buffering** -- keeps first 30 and last 30 lines, discards the middle
7. **Clean-success squelch** -- on a zero exit with no error signal, the tail
   is trimmed further (the summary line always survives)

On top of the pipeline, [adaptive auto-tuning](./configuration#parameter-auto-tuning-enabled-by-default)
adjusts the head/tail budgets per command from your execution history, and
persists what it learns between runs.

The result is a compact output that preserves the information an LLM needs
to understand what happened.

## Design Principles

- **Universal**: works with any command, no per-tool parsers
- **Unintrusive**: the binary never rewrites your shell, aliases, or `$PATH` --
  you invoke it explicitly. An optional, off-by-default
  [Claude Code hook](./claude-code) filters what Claude reads after a command
  ran, without rewriting the command or touching its permission check.
- **Safe**: never modifies the child command's behavior. The one deliberate
  exception is the [safety guard](/reference/#safety-guard), which refuses to
  run a small set of clearly destructive commands (exit 126) when an
  AI-assistant terminal is detected; `--no-guard` opts out.
- **Lightweight**: single Rust binary, zero runtime dependencies
- **Observable**: every invocation logs metrics for analysis
