# Safe public-history plan

The existing private history contains local paths and message-derived fixture details. Do not add its remote to a public repository.

## Preferred: fresh public repository

1. Finish review and commit the sanitized tree in this private archive.
2. Run `scripts/audit-public-repo.sh --history` and review every warning.
3. Create a history-free review tree: `scripts/create-public-export.sh /absolute/review/pst-quickview-public`.
4. Review that directory for privacy, licensing, icons, generated files, and synthetic-only screenshots.
5. Confirm the export contains the approved root `LICENSE`, both files under `LICENSES/`, and
   `THIRD_PARTY_NOTICES.md` exactly as audited.
6. Initialize locally only after review: `git init`, `git add .`, inspect `git diff --cached`, then create the new root commit.
7. A human may create an empty GitHub repository, configure its remote, and push only after a final audit. None of those actions are performed by the export script.

The private repository remains the development archive.

## Alternative: rewrite private history

A rewrite can remove sensitive historical blobs, but changes every affected commit and tag. Only consider it after an offline backup, a complete inventory of refs and forks, credential review, and explicit owner approval. Coordinate replacement of all clones. Never use a rewrite as an unreviewed shortcut.
