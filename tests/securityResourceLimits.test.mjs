import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const backend = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const cancellation = readFileSync(
  new URL("../src-tauri/src/search_cancellation.rs", import.meta.url),
  "utf8",
);
const cursor = readFileSync(
  new URL("../src-tauri/src/search_pagination.rs", import.meta.url),
  "utf8",
);
const messages = readFileSync(new URL("../src/MessageResultsList.tsx", import.meta.url), "utf8");
const conversations = readFileSync(
  new URL("../src/ConversationResultsList.tsx", import.meta.url),
  "utf8",
);
const variableHeightWindow = readFileSync(
  new URL("../src/variableHeightWindow.ts", import.meta.url),
  "utf8",
);

test("workspace, offset, operation, and cursor state has explicit backend ceilings", () => {
  assert.match(backend, /const MAX_OPEN_WORKSPACES: usize = 64/);
  assert.equal(
    (backend.match(/validate_workspace_batch_size\((?:workspace_ids|scopes)\.len\(\)\)\?/g) ?? [])
      .length,
    4,
  );
  assert.equal((backend.match(/validate_search_offset\(offset\)\?/g) ?? []).length, 3);
  assert.match(backend, /checked_result_count\(total_count, count\)\?/);
  assert.match(cancellation, /const MAX_ACTIVE_OPERATIONS_PER_WINDOW: usize = 64/);
  assert.match(cursor, /const MAX_CURSOR_SCALARS: usize = 192/);
});

test("temporary outputs are exclusive, private, and no-follow", () => {
  assert.match(
    backend,
    /fn create_private_file_new[\s\S]*create_new\(true\)[\s\S]*PRIVATE_FILE_MODE[\s\S]*O_NOFOLLOW/,
  );
  assert.match(backend, /temporary_write_probe_path/);
  assert.doesNotMatch(backend, /File::create\(&test_path\)/);
});

test("mounted result DOM and lifecycle registries remain bounded", () => {
  assert.match(variableHeightWindow, /DEFAULT_MAXIMUM_ORDINARY_ROWS = 80/);
  assert.match(messages, /windowModel\.calculateWindow\(/);
  assert.match(conversations, /windowModel\.calculateWindow\(/);
  assert.match(cancellation, /MAX_TRACKED_WINDOWS: usize = 16/);
  assert.match(cancellation, /completed_operations_do_not_grow_the_registry/);
});
