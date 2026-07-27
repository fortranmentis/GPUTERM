#!/usr/bin/env python3
"""GpuTerm Claude Code status line.

Reads Claude's status-line JSON from stdin and writes only the whitelisted
usage fields needed by GpuTerm. Prompts, responses, tool data, transcript
paths, session names, and credentials are never copied.
"""

import json
import os
import sys
import time


def pick(source, keys):
    if not isinstance(source, dict):
        return None
    picked = {key: source[key] for key in keys if source.get(key) is not None}
    return picked or None


try:
    payload = json.load(sys.stdin)
except Exception:
    print("")
    raise SystemExit(0)

if not isinstance(payload, dict):
    print("")
    raise SystemExit(0)

model = payload.get("model") if isinstance(payload.get("model"), dict) else {}
context = (
    payload.get("context_window")
    if isinstance(payload.get("context_window"), dict)
    else {}
)
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

if isinstance(session_id, str) and session_id:
    directory = os.path.expanduser("~/.cache/gputerm/agent-status/claude")
    try:
        os.makedirs(directory, exist_ok=True)
        target = os.path.join(directory, session_id + ".json")
        temporary = target + ".tmp"
        with open(temporary, "w", encoding="utf-8") as handle:
            json.dump(snapshot, handle, separators=(",", ":"))
        os.replace(temporary, target)
        cutoff = time.time() - 7 * 24 * 3600
        for name in os.listdir(directory):
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
