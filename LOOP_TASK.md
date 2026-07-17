# Supervised Codex Loop Task

Replace the bracketed guidance before running `scripts/codex-loop.sh`. The safe
default is that no repository file is authorized for modification.

## Objective

- [Describe one narrow, testable repair objective.]

## Allowed files

- None by default.
- [List every repository-relative file or tightly bounded directory that Codex
  may modify.]

## Forbidden changes

- Any file outside **Allowed files**.
- Application features or behavior unrelated to the objective.
- Source PST, EML, or MSG files or fixtures.
- HTML sanitizer weakening or default remote-resource enablement.
- Attachment opening that bypasses export-first behavior.
- Workspace deletion, source-file mutation, release installation, signing, or
  notarization.
- Telemetry, analytics, cloud processing, or new network behavior.
- Dependency versions, exact pins, manifests, or lockfiles unless the user
  explicitly provides a separate authorized dependency task.
- Automatic commit, push, upload, installation, release, or destructive
  cleanup.

## Acceptance criteria

- [State the expected behavior or narrowly scoped repair.]
- All commands in **Commands** pass.
- `scripts/agent-check.sh` passes in full.
- The final Codex self-review reports no safety blocker or unrelated feature
  change.
- Existing user changes remain intact.

## Commands

```sh
scripts/agent-check.sh
```

- [Add focused non-destructive reproduction or regression commands if needed.]

## Manual validation still required

- Review every changed and untracked file.
- Perform relevant visual UI checks.
- Exercise relevant real PST workflows with read-only source files and verify
  source hashes before and after.
- [Add task-specific manual checks.]

## Stop conditions

- All automated acceptance checks pass, the self-review passes, and the final
  acceptance rerun passes.
- Maximum three repair iterations are reached, unless `MAX_ITERATIONS` sets a
  smaller or larger bound; the hard cap is five.
- A required fix falls outside **Allowed files**.
- Safety or product behavior is ambiguous.
- The loop encounters signing credentials, source-file mutation, workspace
  deletion, or release installation.
- Codex requests broader permissions, network access, or a prohibited command.
- A merge conflict, unexpected Git commit, or protected fixture hash change is
  detected.
