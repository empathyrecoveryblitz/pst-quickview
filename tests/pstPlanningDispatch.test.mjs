import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const source = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");

test("PST planning dispatch yields before source hashing or workspace inspection", () => {
  const start = source.indexOf("#[tauri::command]\nasync fn plan_pst_open(");
  assert.notEqual(start, -1, "planning must be an asynchronous Tauri command");
  const workerStart = source.indexOf("async fn run_pst_planning_worker", start);
  const blockingStart = source.indexOf("fn plan_pst_open_blocking(", workerStart);
  assert.ok(workerStart > start && blockingStart > workerStart);
  const command = source.slice(start, workerStart);
  const worker = source.slice(workerStart, blockingStart);
  assert.match(command, /state\.runtime_profile\(\)\?\.paths\(\)\.clone\(\)/);
  assert.match(command, /run_pst_planning_worker\(move \|\| \{[\s\S]*plan_pst_open_blocking/);
  assert.match(command, /\.await/);
  assert.doesNotMatch(command, /identify_pst\(|find_existing_workspaces\(|existing_workspace_summary\(/);
  assert.match(worker, /tauri::async_runtime::spawn_blocking\(work\)\s*\.await/);
});
