import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  defaultSearchScope,
  normalizeSearchScope,
  saveSearchScope,
  searchScopeStorageKey,
  storedSearchScope,
} from "../src/searchScopePreference.ts";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");

function memoryStorage(initial = {}) {
  const values = new Map(Object.entries(initial));
  return {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: (key) => values.delete(key),
  };
}

test("search Scope defaults to Current PST without a saved preference", () => {
  assert.equal(defaultSearchScope, "current");
  assert.equal(storedSearchScope(memoryStorage()), "current");
});

test("changing search Scope saves the selected valid value", () => {
  const storage = memoryStorage();
  saveSearchScope(storage, "all_open");
  assert.equal(storage.getItem(searchScopeStorageKey), "all_open");
});

test("saved search Scope restores after a simulated application restart", () => {
  const storage = memoryStorage();
  saveSearchScope(storage, "all_open");

  assert.equal(storedSearchScope(storage), "all_open");
});

test("invalid saved search Scope falls back to Current PST", () => {
  assert.equal(normalizeSearchScope("selected_psts"), "current");
  assert.equal(
    storedSearchScope(memoryStorage({ [searchScopeStorageKey]: "malformed" })),
    "current",
  );
});

test("session restore and Start Fresh leave the search Scope preference intact", () => {
  const previousSessionStorageKey = "pstQuickView.previousSession";
  const storage = memoryStorage({ [previousSessionStorageKey]: "saved-session" });
  saveSearchScope(storage, "all_open");

  storage.removeItem(previousSessionStorageKey);

  assert.equal(storedSearchScope(storage), "all_open");
  assert.doesNotMatch(app, /setSearchScope\("current"\)/);
  assert.doesNotMatch(app, /removeItem\(searchScopeStorageKey\)/);
});

test("search Scope initializes synchronously and persists independently of search text", () => {
  assert.match(
    app,
    /useState<SearchScope>\(\(\) =>\s*storedSearchScope\(/,
  );
  assert.match(app, /saveSearchScope\(window\.localStorage, searchScope\)/);
  assert.doesNotMatch(app, /pstQuickView\.(?:searchQuery|searchText)/);
});
