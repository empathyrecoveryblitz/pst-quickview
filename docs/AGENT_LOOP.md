# Bounded Supervised Codex Loop

## Purpose

`scripts/codex-loop.sh` is a local, bounded repair harness. It runs the full
PST QuickView acceptance script, gives the latest check output and an explicitly
scoped task to Codex, permits at most a small number of repair iterations, runs
a read-only Codex safety review, reruns acceptance, and stops for human review.

The loop does not replace engineering judgment. It does not commit, push,
upload, install, release, sign, notarize, delete workspaces, or authorize product
changes outside `LOOP_TASK.md`.

## Prerequisites

- A macOS Git worktree with no unresolved merge conflicts.
- An authenticated current Codex CLI with `codex exec` support.
- Node.js/npm dependencies already installed.
- The Rust toolchain required by `src-tauri/Cargo.toml`.
- Xcode command-line tools for macOS builds and release inspection.
- A completed `LOOP_TASK.md` with a narrow objective and exact allowed paths.

The loop warns loudly when the worktree is dirty. Use strict mode when changes
must be attributable to one run. A real run refuses the unchanged template's
default no-files authorization; `DRY_RUN=1` remains available to inspect it.

## Safe Example

Review and edit `LOOP_TASK.md`, then run:

```sh
MAX_ITERATIONS=3 scripts/codex-loop.sh
```

This normal path emits a prominent dirty-worktree warning because the edited
task file is itself tracked. Use `STRICT_WORKTREE=1` only when a completed
`LOOP_TASK.md` is already present in a clean worktree, such as a dedicated
human-prepared branch. The wrapper never commits the task file for you.

Preview the preflight, prompt, and exact repair command without invoking Codex
or acceptance checks:

```sh
DRY_RUN=1 scripts/codex-loop.sh
```

The repair invocation used by the loop is:

```sh
codex --ask-for-approval never exec \
  --sandbox workspace-write \
  --cd "$ROOT_DIR" \
  --ephemeral \
  --ignore-user-config \
  -c 'sandbox_workspace_write.network_access=false' \
  -c 'web_search="disabled"' \
  --output-schema "$REPAIR_SCHEMA" \
  --output-last-message "$LAST_MESSAGE" \
  -
```

The prompt is supplied on standard input and contains `AGENTS.md`,
`LOOP_TASK.md`, the starting worktree status and tracked diff, the latest check
output, and any self-review diagnostic. Repair execution uses workspace-write
sandboxing, approval-never fail-closed behavior, explicit shell-network denial,
and disabled web search.
The safety review uses the same non-interactive command with a read-only
sandbox. Complete logs remain in `.agent-loop/`; prompt excerpts larger than
256 KiB retain the first and last 128 KiB with an explicit truncation marker.
Diagnostic excerpts are line-prefixed and explicitly labeled untrusted so
embedded file content cannot masquerade as loop instructions.

## Environment Variables

- `MAX_ITERATIONS`: repair iterations; default `3`, minimum `1`, hard cap `5`.
- `STRICT_WORKTREE=1`: refuse to run unless the worktree is clean.
- `DRY_RUN=1`: run preflight and write a prompt/report without invoking Codex or
  acceptance checks.
- `VERIFY_RELEASE=1`: make `scripts/agent-check.sh` run
  `scripts/verify-macos-release.sh`. The verifier also runs automatically when
  the universal packaged app already exists.
- `PST_QUICKVIEW_RICH_MSG_FIXTURE`: absolute read-only rich-content MSG fixture.
- `PST_QUICKVIEW_RICH_MSG_EXPECTED_SHA256`: optional private expected hash; never record it in tracked files.
- `PST_QUICKVIEW_LEGACY_MSG_FIXTURE`: absolute read-only legacy-RTF MSG fixture.
- `PST_QUICKVIEW_LEGACY_MSG_EXPECTED_SHA256`: optional private expected hash; never record it in tracked files.

Fixture tests run only when the corresponding environment variable names an
existing file. The tests hash their known source before and after parsing.

## Iteration And Stop Behavior

The harness first runs `scripts/agent-check.sh`. A failure becomes the diagnostic
for repair iteration 1. After every Codex repair attempt, the full acceptance
script runs again even if Codex exits with an error or asks to stop.

The finite loop stops when:

- Acceptance passes, the read-only self-review passes, and the final acceptance
  rerun passes.
- The configured repair limit is reached.
- A required fix is outside the task's allowed files.
- Product or safety behavior is ambiguous.
- A merge conflict, unexpected Git HEAD change, exact dependency-pin or lockfile
  change, or source-fixture hash change is detected.
- Signing credentials, source mutation, workspace deletion, release
  installation, prohibited commands, broader permissions, or network access are
  encountered.

The preflight rejects common Apple, Electron, and Tauri signing/notarization
credential environment variables without printing their values.

Command shims add defense in depth by blocking Git history/worktree destruction,
`git commit`, `git push`, recursive `rm`, `find -delete`, `unlink`, `rmdir`,
`sudo`, `codesign`, application launch/installation or disk-image mounting,
`xcrun notarytool`, quarantine removal, and Gatekeeper-disable commands issued
through normal `PATH` lookup. The sandbox and prompt remain required; these
shims are not a security boundary against deliberately crafted absolute-path or
alternate-runtime bypasses.

Reports are appended under `.agent-loop/<UTC timestamp>-<pid>/`. Old reports are
not deleted. Each run records prompts, Codex output, acceptance output,
invariant checks, self-review findings, and the final human-gate summary.

## Human Review

Completion always prints:

- Result and repair iteration count.
- Current changed files.
- Passing checks and skipped optional checks.
- Unresolved warnings and self-review findings.
- Manual validation copied from `LOOP_TASK.md`.
- The report directory.
- The exact Git status/diff command for review.

Do not commit from the loop. Review tracked changes with:

```sh
git status --short --untracked-files=all
git diff --no-ext-diff -- . ':(exclude).agent-loop'
```

Plain `git diff` omits untracked file contents, so open every untracked file
listed by `git status`. Compare the result with the allowed-file list and the
starting dirty-worktree warning before staging anything. Dirty-mode reports
retain the starting tracked staged and unstaged diffs for comparison, but only
strict clean-worktree mode provides reliable attribution for untracked files.

## Aborting

Press `Ctrl-C`. The wrapper exits with an aborted result, does not clean or
revert the worktree, and leaves available reports under `.agent-loop/`. Inspect
partial changes before running again.

## Manual Validation Remains Required

Automated checks cannot reliably validate native macOS layout, pane resizing,
Finder Open With registration, drag and drop, Gatekeeper prompts, attachment
launch behavior, or visual HTML rendering. They also cannot replace read-only
end-to-end tests on representative PSTs, large archives, malformed data, legacy
ANSI PSTs, real calendar MSGs, or clean Intel and Apple Silicon machines without
Homebrew.

Real PST workflows may create or reuse workspaces and can consume substantial
disk space. Workspace deletion and application installation are intentionally
outside this loop. A human must select safe source files, record hashes when
appropriate, confirm export-first behavior, inspect the displayed workspace
path, and verify that original PST, EML, and MSG files remain unchanged.
