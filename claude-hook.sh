#!/usr/bin/env bash
# ==============================================================================
# claude-hook.sh — manage the transparent l0-compressor integration for Claude Code.
#
# Registers a PostToolUse hook (`l0-compressor --claude-hook`) that filters the
# output of successful Bash tool calls before Claude reads it. The command
# itself is never rewritten, so Claude Code's permission rules (allow / ask /
# deny) and execution are exactly what they would be without l0-compressor.
# Fail-safe (any error → Claude reads the original output) and OFF by default —
# toggle it on/off at runtime with no restart.
#
# Usage:
#   ./claude-hook.sh install     Register the hook (idempotent; migrates the old one)
#   ./claude-hook.sh enable      Turn the hook ON  (create the toggle file)
#   ./claude-hook.sh disable     Turn the hook OFF (remove the toggle file)
#   ./claude-hook.sh status      Show install / enabled state + l0-compressor version
#   ./claude-hook.sh uninstall   Remove the hook registration
#   ./claude-hook.sh help
#
# Notes:
#   * `install`/`uninstall` edit Claude Code's settings.json and need `jq`.
#   * The hook is registered with the absolute path of `l0-compressor` (or
#     $L0_COMPRESSOR_BIN), so it works where PATH differs — e.g. the VS Code
#     extension started from the Dock. Re-run `install` if the binary moves.
#   * After install (or after changing settings), start a NEW Claude Code session
#     so the hook is loaded. The enable/disable toggle is then instant.
#   * Honors $CLAUDE_CONFIG_DIR and $XDG_CONFIG_HOME.
# ==============================================================================
set -euo pipefail

# Remove the jq scratch file even on an early `set -e` exit (e.g. a malformed
# settings.json). The real settings.json is only ever replaced via `mv`.
_l0_tmp=""
trap 'rm -f "$_l0_tmp"' EXIT

CLAUDE_DIR="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
SETTINGS="$CLAUDE_DIR/settings.json"
TOGGLE_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/l0-compressor"
TOGGLE="$TOGGLE_DIR/hook.enabled"
# The pre-0.4 integration: a PreToolUse wrapper that rewrote the command. It
# made Claude Code evaluate permission rules against `l0-compressor <cmd>`
# instead of `<cmd>`, so `install`/`uninstall` remove it.
LEGACY_WRAPPER="$CLAUDE_DIR/hooks/l0-compressor-wrapper.sh"

# Color only on an interactive terminal with NO_COLOR unset.
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  c_g=$'\033[0;32m'; c_y=$'\033[0;33m'; c_r=$'\033[0;31m'; c_b=$'\033[0;34m'; c_0=$'\033[0m'
else
  c_g=''; c_y=''; c_r=''; c_b=''; c_0=''
fi
info() { printf '  %s●%s %s\n' "$c_b" "$c_0" "$*"; }
ok()   { printf '  %s●%s %s\n' "$c_g" "$c_0" "$*"; }
warn() { printf '  %s●%s %s\n' "$c_y" "$c_0" "$*"; }
err()  { printf '  %s●%s %s\n' "$c_r" "$c_0" "$*" >&2; }

need_jq() {
  command -v jq >/dev/null 2>&1 || { err "jq is required for this command. Install it (e.g. 'brew install jq' / 'apt-get install jq')."; exit 1; }
}

# Absolute path of the l0-compressor binary the hook will run.
resolve_bin() {
  local bin="${L0_COMPRESSOR_BIN:-}"
  [ -n "$bin" ] || bin="$(command -v l0-compressor 2>/dev/null || true)"
  [ -n "$bin" ] || { err "l0-compressor not found in PATH (or set L0_COMPRESSOR_BIN)."; exit 1; }
  case "$bin" in
    /*) ;;
    *) bin="$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")" ;;
  esac
  [ -x "$bin" ] || { err "$bin is not executable."; exit 1; }
  # Binaries before 0.4 do not have the hook mode.
  "$bin" --help 2>/dev/null | grep -q -- '--claude-hook' || {
    err "$bin has no --claude-hook mode ($("$bin" --version 2>/dev/null || echo unknown version)). Upgrade l0-compressor first."
    exit 1
  }
  printf '%s\n' "$bin"
}

# jq program: drop every hook entry that is ours — the current PostToolUse
# `--claude-hook` command and the legacy PreToolUse wrapper — then prune
# matcher groups and events left empty. Unrelated hooks are untouched.
# shellcheck disable=SC2016
JQ_DROP_OURS='
  def ours: (.command // "") as $c
    | ($c == $legacy) or ($c | test("l0-compressor[^ ]*\"? --claude-hook$"));
  if .hooks then
    .hooks |= with_entries(
      .value |= map(.hooks = ((.hooks // []) | map(select(ours | not))))
               | .value |= map(select((.hooks | length) > 0))
    )
    | .hooks |= with_entries(select((.value | length) > 0))
    | (if .hooks == {} then del(.hooks) else . end)
  else . end'

rewrite_settings() { # $1 = jq program, remaining args passed to jq
  local prog="$1"; shift
  local tmp; tmp="$(mktemp)"; _l0_tmp="$tmp"
  jq "$@" "$prog" "$SETTINGS" > "$tmp"
  jq empty "$tmp"            # validate
  mv "$tmp" "$SETTINGS"
}

cmd_install() {
  need_jq
  local bin; bin="$(resolve_bin)"
  info "Installing l0-compressor Claude Code hook ($bin)..."

  mkdir -p "$CLAUDE_DIR"
  [ -f "$SETTINGS" ] || echo '{}' > "$SETTINGS"
  cp "$SETTINGS" "$SETTINGS.bak.$(date +%s)"

  # Quote the path for the shell Claude Code runs the hook command in.
  local hook_cmd; hook_cmd="\"$bin\" --claude-hook"
  # Idempotent: remove our previous entries (incl. the legacy wrapper), then add one.
  # shellcheck disable=SC2016  # $cmd is a jq variable
  rewrite_settings "$JQ_DROP_OURS"' | .hooks.PostToolUse = ((.hooks.PostToolUse // []) + [ { matcher: "Bash", hooks: [ { type: "command", command: $cmd } ] } ])' \
    --arg legacy "$LEGACY_WRAPPER" --arg cmd "$hook_cmd"
  ok "Registered PostToolUse(Bash) hook in $SETTINGS (backup saved)."
  if [ -f "$LEGACY_WRAPPER" ]; then
    rm -f "$LEGACY_WRAPPER"
    ok "Removed the old PreToolUse command-rewriting wrapper."
  fi

  mkdir -p "$TOGGLE_DIR"
  [ -f "$TOGGLE" ] || warn "The hook is OFF by default. Enable it with: ./claude-hook.sh enable"
  warn "Start a NEW Claude Code session so the hook is loaded."
}

cmd_uninstall() {
  need_jq
  if [ -f "$SETTINGS" ]; then
    cp "$SETTINGS" "$SETTINGS.bak.$(date +%s)"
    rewrite_settings "$JQ_DROP_OURS" --arg legacy "$LEGACY_WRAPPER"
    ok "Removed the hook registration from $SETTINGS (backup saved)."
  else
    warn "No settings.json at $SETTINGS — nothing to remove."
  fi
  rm -f "$LEGACY_WRAPPER"
  rm -f "$TOGGLE"; ok "Toggle cleared (hook OFF)."
  warn "Restart Claude Code to drop the hook from the running session."
}

cmd_enable() {
  mkdir -p "$TOGGLE_DIR"; touch "$TOGGLE"
  ok "Hook ENABLED ($TOGGLE)."
}

cmd_disable() {
  rm -f "$TOGGLE"
  ok "Hook DISABLED (toggle removed). Takes effect immediately."
}

cmd_status() {
  printf '%sl0-compressor Claude Code hook%s\n' "$c_b" "$c_0"
  if command -v l0-compressor >/dev/null 2>&1; then ok "l0-compressor: $(l0-compressor --version 2>/dev/null)"; else warn "l0-compressor: not in PATH"; fi
  if command -v jq >/dev/null 2>&1 && [ -f "$SETTINGS" ]; then
    local registered
    registered="$(jq -r '[.hooks.PostToolUse[]?.hooks[]?.command // empty | select(test("--claude-hook$"))] | first // empty' "$SETTINGS" 2>/dev/null || true)"
    if [ -n "$registered" ]; then
      local reg_bin="${registered% --claude-hook}"; reg_bin="${reg_bin#\"}"; reg_bin="${reg_bin%\"}"
      ok "registered: $registered"
      if [ -x "$reg_bin" ]; then ok "hook binary: $("$reg_bin" --version 2>/dev/null)"; else err "hook binary missing: $reg_bin — re-run install"; fi
    else warn "NOT registered in settings.json (run: install)"; fi
    if jq -e --arg h "$LEGACY_WRAPPER" '[.hooks.PreToolUse[]?.hooks[]?.command] | index($h)' "$SETTINGS" >/dev/null 2>&1; then
      err "old PreToolUse command-rewriting hook still registered — run: install (it migrates)"
    fi
  else
    warn "cannot inspect settings.json (missing file or jq)"
  fi
  if [ -f "$TOGGLE" ]; then ok "state: ENABLED ($TOGGLE)"; else warn "state: DISABLED (run: enable)"; fi
}

usage() { awk 'NR==1{next} /^[^#]/{exit} {sub(/^# ?/,""); if ($0 !~ /^=+$/) print}' "$0"; }

case "${1:-help}" in
  install)             cmd_install ;;
  uninstall|remove)    cmd_uninstall ;;
  enable|on)           cmd_enable ;;
  disable|off)         cmd_disable ;;
  status)              cmd_status ;;
  help|-h|--help)      usage ;;
  *) err "Unknown command: ${1:-}"; echo; usage; exit 1 ;;
esac
