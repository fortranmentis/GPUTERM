#!/usr/bin/osascript -l JavaScript
/* global ObjC, $ */
// GpuTerm status line for Claude Code on macOS.
//
// JavaScript for Automation and Foundation ship with macOS, so this helper
// does not require Python. Only the explicitly picked usage fields are written
// to ~/.cache/gputerm/agent-status/claude.

ObjC.import("Foundation");

function field(object, name) {
  return object && object[name] !== undefined && object[name] !== null
    ? object[name]
    : null;
}

function pick(object, names) {
  if (!object || typeof object !== "object") return null;
  var picked = {};
  names.forEach(function (name) {
    var value = field(object, name);
    if (value !== null) picked[name] = value;
  });
  return Object.keys(picked).length > 0 ? picked : null;
}

function remaining(limits, windowName) {
  var window = field(limits, windowName) || {};
  var used = field(window, "used_percentage");
  if (typeof used !== "number") return null;
  return Math.max(0, Math.min(100, 100 - used));
}

function environmentValue(name) {
  var value = $.NSProcessInfo.processInfo.environment.objectForKey($(name));
  return value ? ObjC.unwrap(value) : "";
}

function writeSnapshot(home, sessionId, snapshot) {
  var manager = $.NSFileManager.defaultManager;
  var directory =
    home + "/.cache/gputerm/agent-status/claude";
  manager.createDirectoryAtPathWithIntermediateDirectoriesAttributesError(
    $(directory),
    true,
    $(),
    null,
  );
  var target = directory + "/" + sessionId + ".json";
  var encoded = $(JSON.stringify(snapshot));
  // Foundation's atomic write uses a sibling temporary file followed by a
  // rename, so the telemetry reader never observes a partial JSON document.
  encoded.writeToFileAtomicallyEncodingError(
    $(target),
    true,
    $.NSUTF8StringEncoding,
    null,
  );

  var names = manager.contentsOfDirectoryAtPathError($(directory), null);
  if (!names) return;
  var cutoff = Date.now() / 1000 - 7 * 24 * 60 * 60;
  ObjC.deepUnwrap(names).forEach(function (name) {
    if (!name.endsWith(".json")) return;
    var path = directory + "/" + name;
    var attributes = manager.attributesOfItemAtPathError($(path), null);
    if (!attributes) return;
    var modified = attributes.objectForKey($.NSFileModificationDate);
    if (modified && modified.timeIntervalSince1970 < cutoff) {
      manager.removeItemAtPathError($(path), null);
    }
  });
}

// JXA invokes this global entry point.
// eslint-disable-next-line @typescript-eslint/no-unused-vars
function run() {
  try {
    var data =
      $.NSFileHandle.fileHandleWithStandardInput.readDataToEndOfFile;
    var input = $.NSString.alloc.initWithDataEncoding(
      data,
      $.NSUTF8StringEncoding,
    );
    var payload = JSON.parse(ObjC.unwrap(input));
    if (!payload || typeof payload !== "object") return "";

    var model = field(payload, "model") || {};
    var context = field(payload, "context_window") || {};
    var cost = field(payload, "cost") || {};
    var limits = field(payload, "rate_limits") || {};
    var snapshot = { captured_at: Math.floor(Date.now() / 1000) };
    var sessionId = field(payload, "session_id");
    if (typeof sessionId === "string" && sessionId) {
      snapshot.session_id = sessionId;
    }
    var workspace = field(payload, "workspace") || {};
    var cwd = field(payload, "cwd") || field(workspace, "current_dir");
    if (typeof cwd === "string" && cwd) snapshot.cwd = cwd;

    var pickedModel = pick(model, ["display_name", "id"]);
    if (pickedModel) snapshot.model = pickedModel;
    var pickedContext = pick(context, [
      "total_input_tokens",
      "context_window_size",
      "used_percentage",
      "remaining_percentage",
    ]);
    var currentUsage = pick(field(context, "current_usage") || {}, [
      "input_tokens",
      "output_tokens",
      "cache_creation_input_tokens",
      "cache_read_input_tokens",
    ]);
    if (currentUsage) {
      pickedContext = pickedContext || {};
      pickedContext.current_usage = currentUsage;
    }
    if (pickedContext) snapshot.context_window = pickedContext;
    var pickedCost = pick(cost, ["total_cost_usd", "total_duration_ms"]);
    if (pickedCost) snapshot.cost = pickedCost;

    var pickedLimits = {};
    ["five_hour", "seven_day"].forEach(function (windowName) {
      var picked = pick(field(limits, windowName) || {}, [
        "used_percentage",
        "resets_at",
      ]);
      if (picked) pickedLimits[windowName] = picked;
    });
    if (Object.keys(pickedLimits).length > 0) {
      snapshot.rate_limits = pickedLimits;
    }

    var home = environmentValue("HOME");
    if (home && snapshot.session_id) {
      writeSnapshot(home, snapshot.session_id, snapshot);
    }

    var parts = [];
    var modelName = field(model, "display_name") || field(model, "id");
    if (modelName) parts.push(String(modelName));
    var contextUsed = field(context, "used_percentage");
    if (typeof contextUsed === "number") {
      parts.push("ctx " + contextUsed.toFixed(0) + "%");
    }
    var fiveHour = remaining(limits, "five_hour");
    if (fiveHour !== null) parts.push("5h " + fiveHour.toFixed(0) + "%");
    var sevenDay = remaining(limits, "seven_day");
    if (sevenDay !== null) parts.push("wk " + sevenDay.toFixed(0) + "%");
    var totalCost = field(cost, "total_cost_usd");
    if (typeof totalCost === "number") {
      parts.push("$" + totalCost.toFixed(2));
    }
    return parts.join(" | ");
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
  } catch (error) {
    // A status-line failure must never interfere with the Claude session.
    return "";
  }
}
