#!/usr/bin/env bash
# ==============================================================================
# agent-hook.sh — transparent l0-compressor integration for AI coding agents.
#
# Installs a hook that routes the *simple* Bash commands an agent runs through
# `l0-compressor` (to cut token usage), with the model prefixing nothing. Conservative
# (compound/piped/interactive/stateful commands pass through untouched), fail-safe
# (any error → the command runs unchanged), and OFF by default — toggle at runtime.
#
# Supported agents:
#   * claude  — Claude Code: delegated to claude-hook.sh, which filters the
#               output AFTER the command ran (PostToolUse) and never rewrites
#               the command, so permission rules see the real command.
#   * gemini  — Gemini CLI   (BeforeTool → hookSpecificOutput)   ~/.gemini
#               Rewrites the command before it runs. Check that your Gemini
#               policy rules still match the rewritten `l0-compressor <cmd>`.
# Cursor's beforeShellExecution can only allow/deny (no rewrite), so it cannot be
# wrapped transparently — see the docs for the manual prefix approach.
#
# Usage:
#   ./agent-hook.sh install [agent]    Install the wrapper + register the hook
#   ./agent-hook.sh enable             Turn the hook ON  (shared by all agents)
#   ./agent-hook.sh disable            Turn the hook OFF
#   ./agent-hook.sh status  [agent]    Show install / enabled state + version
#   ./agent-hook.sh uninstall [agent]  Remove the hook registration and wrapper
#   ./agent-hook.sh help
#
#   [agent] defaults to "claude". Examples: ./agent-hook.sh install gemini
#
# Notes:
#   * install/uninstall edit the agent's settings.json and need `jq`.
#   * After install, start a NEW agent session so the hook loads. The enable/
#     disable toggle is then instant.
#   * Honors $CLAUDE_CONFIG_DIR, $GEMINI_CONFIG_DIR, and $XDG_CONFIG_HOME.
# ==============================================================================
set -euo pipefail

# Remove the jq scratch file even on an early `set -e` exit (e.g. a malformed
# settings.json). The real settings.json is only ever replaced via `mv`.
_l0_tmp=""
trap 'rm -f "$_l0_tmp"' EXIT

TOGGLE_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/l0-compressor"
TOGGLE="$TOGGLE_DIR/hook.enabled"

c_g=''; c_y=''; c_r=''; c_b=''; c_0=''
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  c_g=$'\033[0;32m'; c_y=$'\033[0;33m'; c_r=$'\033[0;31m'; c_b=$'\033[0;34m'; c_0=$'\033[0m'
fi
info() { printf '  %s●%s %s\n' "$c_b" "$c_0" "$*"; }
ok()   { printf '  %s●%s %s\n' "$c_g" "$c_0" "$*"; }
warn() { printf '  %s●%s %s\n' "$c_y" "$c_0" "$*"; }
err()  { printf '  %s●%s %s\n' "$c_r" "$c_0" "$*" >&2; }

need_jq() {
  command -v jq >/dev/null 2>&1 || { err "jq is required. Install it (e.g. 'brew install jq' / 'apt-get install jq')."; exit 1; }
}

# Per-agent configuration: directory, settings file, hook event, tool matcher,
# and the jq expression that emits the agent's rewrite output.
AGENT=''; AGENT_DIR=''; SETTINGS=''; HOOKS_DIR=''; WRAPPER=''; EVENT=''; MATCHER=''; OUTPUT_JQ=''
select_agent() {
  AGENT="${1:-claude}"
  # OUTPUT_JQ holds a jq program; `$new` is a jq variable, not shell expansion.
  # shellcheck disable=SC2016
  case "$AGENT" in
    claude)
      AGENT_DIR="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
      EVENT="PreToolUse"; MATCHER="Bash"
      OUTPUT_JQ='{hookSpecificOutput: {hookEventName: "PreToolUse", updatedInput: (.tool_input + {command: $new})}}'
      ;;
    gemini)
      AGENT_DIR="${GEMINI_CONFIG_DIR:-$HOME/.gemini}"
      EVENT="BeforeTool"; MATCHER="run_shell_command"
      OUTPUT_JQ='{hookSpecificOutput: {tool_input: {command: $new}}}'
      ;;
    *)
      err "Unknown agent '$AGENT'. Supported: claude, gemini."; exit 1 ;;
  esac
  HOOKS_DIR="$AGENT_DIR/hooks"
  SETTINGS="$AGENT_DIR/settings.json"
  WRAPPER="$HOOKS_DIR/l0-compressor-wrapper.sh"
}

write_wrapper() {
  mkdir -p "$HOOKS_DIR"
  {
    printf '%s\n' '#!/usr/bin/env bash'
    printf '%s\n' "# l0-compressor transparent hook for $AGENT. Managed by agent-hook.sh."
    printf '%s\n' '# CONSERVATIVE + FAIL-SAFE + OFF by default.'
    printf 'OUTPUT_JQ=%q\n' "$OUTPUT_JQ"
    cat <<'WRAP'
toggle="${XDG_CONFIG_HOME:-$HOME/.config}/l0-compressor/hook.enabled"
[ -f "$toggle" ] || exit 0
command -v l0-compressor >/dev/null 2>&1 || exit 0
command -v jq >/dev/null 2>&1 || exit 0

input="$(cat)"
# Field paths differ slightly across agents; try the common ones, then bail.
cmd="$(printf '%s' "$input" | jq -r '.tool_input.command // .toolInput.command // empty' 2>/dev/null)"
[ -n "$cmd" ] || exit 0

# Already wrapped?
case "$cmd" in
  l0-compressor\ * | t\ * | */l0-compressor\ * | */t\ *) exit 0 ;;
esac

# Risky to wrap: shell operators, redirects, subshells, substitution, multi-line,
# or stateful builtins that must affect the real shell.
case "$cmd" in
  *'&&'* | *'||'* | *';'* | *'|'* | *'>'* | *'<'* | *'`'* | *'$('* | *'&'*) exit 0 ;;
  *$'\n'*) exit 0 ;;
  cd | cd\ * | export\ * | source\ * | .\ * | eval\ * | exec\ * | set\ * | unset\ * | alias\ *) exit 0 ;;
  exit | exit\ * | true | true\ * | false | false\ * | : | :\ * | wait | wait\ * | trap\ *) exit 0 ;;
  for\ * | while\ * | until\ * | if\ * | case\ * | function\ * | '{'* | '('*) exit 0 ;;
esac

# Interactive / TUI / REPL programs: l0-compressor would capture instead of passthrough.
first="${cmd%% *}"; first="${first##*/}"
case "$first" in
  vim | vi | nvim | nano | emacs | less | more | man | htop | top | btop | ssh | telnet | fzf | tmux | screen | watch | python | python3 | node | irb | psql | mysql | sqlite3 | tig | lazygit) exit 0 ;;
esac

# Wrap. Keep every other field; only the command changes. No permission decision
# is set, so wrapped commands still go through the agent's normal approvals.
# `--recover` saves full output to a temp file on a failing, truncated command.
printf '%s' "$input" | jq -c --arg new "l0-compressor --quiet --recover $cmd" "$OUTPUT_JQ" 2>/dev/null || exit 0
WRAP
  } > "$WRAPPER"
  chmod +x "$WRAPPER"
}

cmd_install() {
  need_jq
  info "Installing l0-compressor hook for $c_b$AGENT$c_0..."
  write_wrapper
  ok "Wrapper written: $WRAPPER"

  mkdir -p "$AGENT_DIR"
  [ -f "$SETTINGS" ] || echo '{}' > "$SETTINGS"
  cp "$SETTINGS" "$SETTINGS.bak.$(date +%s)"

  # Idempotent: drop any prior entry pointing at our wrapper, then append a fresh one.
  local tmp; tmp="$(mktemp)"; _l0_tmp="$tmp"
  jq --arg ev "$EVENT" --arg m "$MATCHER" --arg h "$WRAPPER" '
    .hooks[$ev] = (
      ((.hooks[$ev] // []) | map(select((.hooks // []) | any(.command == $h) | not)))
      + [ { matcher: $m, hooks: [ { type: "command", command: $h } ] } ]
    )
  ' "$SETTINGS" > "$tmp"
  jq empty "$tmp"            # validate
  mv "$tmp" "$SETTINGS"
  ok "Registered $EVENT($MATCHER) hook in $SETTINGS (backup saved)."

  mkdir -p "$TOGGLE_DIR"
  warn "The hook is OFF by default. Enable it with: ./agent-hook.sh enable"
  warn "Then start a NEW $AGENT session so the hook is loaded."
}

cmd_uninstall() {
  need_jq
  if [ -f "$SETTINGS" ]; then
    cp "$SETTINGS" "$SETTINGS.bak.$(date +%s)"
    local tmp; tmp="$(mktemp)"; _l0_tmp="$tmp"
    jq --arg ev "$EVENT" --arg h "$WRAPPER" '
      (if .hooks[$ev] then .hooks[$ev] |= map(select((.hooks // []) | any(.command == $h) | not)) else . end)
      | (if (.hooks[$ev] // []) == [] then (.hooks |= del(.[$ev])) else . end)
      | (if (.hooks // {}) == {} then del(.hooks) else . end)
    ' "$SETTINGS" > "$tmp"
    jq empty "$tmp"
    mv "$tmp" "$SETTINGS"
    ok "Removed the hook registration from $SETTINGS (backup saved)."
  else
    warn "No settings.json at $SETTINGS — nothing to remove."
  fi
  rm -f "$WRAPPER"; ok "Removed wrapper $WRAPPER"
  warn "Restart $AGENT to drop the hook from the running session."
}

cmd_enable() {
  mkdir -p "$TOGGLE_DIR"; touch "$TOGGLE"
  ok "Hook ENABLED ($TOGGLE) — applies to every installed agent."
}

cmd_disable() {
  rm -f "$TOGGLE"
  ok "Hook DISABLED (toggle removed). Takes effect immediately."
}

cmd_status() {
  printf '%sl0-compressor hook — %s%s\n' "$c_b" "$AGENT" "$c_0"
  if command -v l0-compressor >/dev/null 2>&1; then ok "l0-compressor: $(l0-compressor --version 2>/dev/null)"; else err "l0-compressor: not found in PATH"; fi
  if [ -f "$WRAPPER" ]; then ok "wrapper installed: $WRAPPER"; else warn "wrapper NOT installed (run: install $AGENT)"; fi
  if command -v jq >/dev/null 2>&1 && [ -f "$SETTINGS" ] && jq -e --arg ev "$EVENT" --arg h "$WRAPPER" '(.hooks[$ev] // []) | any((.hooks // []) | any(.command == $h))' "$SETTINGS" >/dev/null 2>&1; then
    ok "registered in $SETTINGS"
  else
    warn "NOT registered in $SETTINGS (run: install $AGENT)"
  fi
  if [ -f "$TOGGLE" ]; then ok "state: ENABLED ($TOGGLE)"; else warn "state: DISABLED (run: enable)"; fi
}

usage() { awk 'NR==1{next} /^[^#]/{exit} {sub(/^# ?/,""); if ($0 !~ /^=+$/) print}' "$0"; }

# Claude Code has its own, post-execution integration: hand off to claude-hook.sh
# (next to this script in a checkout, or `l0-compressor-claude-hook` from Homebrew).
claude_hook() {
  local here; here="$(cd "$(dirname "$0")" && pwd)"
  if [ -x "$here/claude-hook.sh" ]; then exec "$here/claude-hook.sh" "$@"; fi
  if command -v l0-compressor-claude-hook >/dev/null 2>&1; then exec l0-compressor-claude-hook "$@"; fi
  err "claude-hook.sh not found next to $0 nor l0-compressor-claude-hook in PATH."; exit 1
}

action="${1:-help}"
case "$action:${2:-claude}" in
  install:claude|uninstall:claude|remove:claude|status:claude) claude_hook "$action" ;;
esac
case "$action" in
  install)             select_agent "${2:-claude}"; cmd_install ;;
  uninstall|remove)    select_agent "${2:-claude}"; cmd_uninstall ;;
  enable|on)           cmd_enable ;;
  disable|off)         cmd_disable ;;
  status)              select_agent "${2:-claude}"; cmd_status ;;
  help|-h|--help)      usage ;;
  *) err "Unknown command: $action"; echo; usage; exit 1 ;;
esac
