#!/bin/sh
# GpuTerm status line for Claude Code.
#
# Claude Code's status-line hook is the only supported source for a session's
# 5-hour and 7-day usage limits: they are not written to the session transcript.
# This script publishes a snapshot of that data so GpuTerm's AI DASH can show
# the remaining balance, then prints a status line of its own.
#
# Install by adding this to ~/.claude/settings.json (on every host you want to
# monitor, including remote ones):
#
#   {
#     "statusLine": {
#       "type": "command",
#       "command": "~/.claude/gputerm-claude-statusline.sh",
#       "padding": 0
#     }
#   }
#
# The snapshot carries only the fields picked out below: session id, working
# directory, model, context window, cost, the two rate-limit windows, a capture
# time, and the agent pid. Prompts, responses, tool input and output, transcript
# paths, session names, and repository details are never copied into it.

set -u

INPUT=$(cat)

# The agent process id lets GpuTerm attribute usage to the right session when
# several are running. Walk up from this script until a `claude` process is
# found; if the chain is exec'd away, the field is simply omitted.
find_agent_pid() {
  pid=$$
  depth=0
  while [ "$depth" -lt 6 ]; do
    parent=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')
    [ -n "$parent" ] || return 0
    [ "$parent" -gt 1 ] 2>/dev/null || return 0
    # Spaces are dropped to trim the column; an interior space in a directory
    # name cannot change the final path segment being compared.
    name=$(ps -o comm= -p "$parent" 2>/dev/null | tr -d ' ')
    case "${name##*/}" in
      claude | claude.exe)
        printf '%s' "$parent"
        return 0
        ;;
    esac
    pid=$parent
    depth=$((depth + 1))
  done
}

AGENT_PID=$(find_agent_pid)

PYTHON=$(command -v python3 || command -v python) || {
  # Without Python there is nothing to parse the hook payload with. Fall back to
  # a status line that at least keeps the row from going blank.
  printf 'GpuTerm status line needs python3\n'
  exit 0
}

# Fixed location: this is the path GpuTerm's collector reads.
SNAPSHOT_DIR="$HOME/.cache/gputerm/agent-status/claude"

# The payload travels in the environment because the interpreter itself is fed
# on stdin.
GPUTERM_PAYLOAD="$INPUT" AGENT_PID="${AGENT_PID:-}" SNAPSHOT_DIR="$SNAPSHOT_DIR" "$PYTHON" - <<'GPUTERM_STATUSLINE'
import json
import os
import sys
import time

# Whitelist of what may leave the session. Everything else in the hook payload
# stays where it is.
def pick(source, keys):
    if not isinstance(source, dict):
        return None
    picked = {key: source[key] for key in keys if source.get(key) is not None}
    return picked or None


try:
    payload = json.loads(os.environ.get("GPUTERM_PAYLOAD") or "")
except Exception:
    print("")
    sys.exit(0)
if not isinstance(payload, dict):
    print("")
    sys.exit(0)

model = payload.get("model") if isinstance(payload.get("model"), dict) else {}
context = payload.get("context_window") if isinstance(payload.get("context_window"), dict) else {}
cost = payload.get("cost") if isinstance(payload.get("cost"), dict) else {}
limits = payload.get("rate_limits") if isinstance(payload.get("rate_limits"), dict) else {}

snapshot = {"captured_at": int(time.time())}
session_id = payload.get("session_id")
if isinstance(session_id, str) and session_id:
    snapshot["session_id"] = session_id
cwd = payload.get("cwd") or (payload.get("workspace") or {}).get("current_dir")
if isinstance(cwd, str) and cwd:
    snapshot["cwd"] = cwd
picked_model = pick(model, ("display_name", "id"))
if picked_model:
    snapshot["model"] = picked_model
picked_context = pick(
    context,
    (
        "total_input_tokens",
        "context_window_size",
        "used_percentage",
        "remaining_percentage",
    ),
)
current_usage = pick(
    context.get("current_usage") or {},
    (
        "input_tokens",
        "output_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
    ),
)
if current_usage:
    picked_context = picked_context or {}
    picked_context["current_usage"] = current_usage
if picked_context:
    snapshot["context_window"] = picked_context
picked_cost = pick(cost, ("total_cost_usd", "total_duration_ms"))
if picked_cost:
    snapshot["cost"] = picked_cost
picked_limits = {}
for window in ("five_hour", "seven_day"):
    picked = pick(limits.get(window) or {}, ("used_percentage", "resets_at"))
    if picked:
        picked_limits[window] = picked
if picked_limits:
    snapshot["rate_limits"] = picked_limits
agent_pid = os.environ.get("AGENT_PID", "").strip()
if agent_pid.isdigit():
    snapshot["pid"] = int(agent_pid)

ACCOUNT_SNAPSHOT_NAME = "account.json"
directory = os.environ["SNAPSHOT_DIR"]


def write_atomically(target, value):
    # Written to a sibling temp file first so GpuTerm never reads a partial
    # document mid-write.
    temporary = target + ".tmp"
    with open(temporary, "w") as handle:
        json.dump(value, handle, separators=(",", ":"))
    os.replace(temporary, target)


if snapshot.get("session_id"):
    try:
        os.makedirs(directory, exist_ok=True)
        write_atomically(
            os.path.join(directory, snapshot["session_id"] + ".json"), snapshot
        )
        # The 5-hour and weekly windows are account-wide, and Claude only
        # includes them after a session's first API response. Publishing them to
        # one fixed file keeps short-lived sessions - which write quota-less
        # snapshots - from crowding the only useful reading out of the reader's
        # newest-files window. Absent limits leave the account file untouched.
        if snapshot.get("rate_limits"):
            write_atomically(
                os.path.join(directory, ACCOUNT_SNAPSHOT_NAME),
                {
                    "scope": "account",
                    "captured_at": snapshot["captured_at"],
                    "session_id": snapshot["session_id"],
                    "rate_limits": snapshot["rate_limits"],
                },
            )
        cutoff = time.time() - 7 * 24 * 3600
        for name in os.listdir(directory):
            # The account file is refreshed only when limits are published, so
            # age is not a reason to delete it.
            if name == ACCOUNT_SNAPSHOT_NAME:
                continue
            path = os.path.join(directory, name)
            if name.endswith(".json") and os.path.getmtime(path) < cutoff:
                os.remove(path)
    except Exception:
        pass


def remaining(window):
    used = (limits.get(window) or {}).get("used_percentage")
    if not isinstance(used, (int, float)):
        return None
    return max(0.0, min(100.0, 100.0 - used))


parts = []
name = model.get("display_name") or model.get("id")
if name:
    parts.append(str(name))
used_percentage = context.get("used_percentage")
if isinstance(used_percentage, (int, float)):
    parts.append("ctx {:.0f}%".format(used_percentage))
five_hour = remaining("five_hour")
seven_day = remaining("seven_day")
if five_hour is not None:
    parts.append("5h {:.0f}%".format(five_hour))
if seven_day is not None:
    parts.append("wk {:.0f}%".format(seven_day))
total_cost = cost.get("total_cost_usd")
if isinstance(total_cost, (int, float)):
    parts.append("${:.2f}".format(total_cost))
print(" · ".join(parts))
GPUTERM_STATUSLINE
