# Contributing

PST QuickView is licensed under `GPL-3.0-or-later`. By submitting a contribution, you agree that it is submitted under `GPL-3.0-or-later` and that you have the right to license the work on those terms. This project does not require a contributor license agreement.

Do not submit employer-owned work, confidential information, private-message content, or third-party code unless you have explicit authorization to do so. Code copied or adapted from another project must retain its original license and attribution, and its license must be compatible with this project and the intended distribution.

No real PST, OST, EML, MSG, attachment, message body, private log, workspace, diagnostics output, or private screenshot may be committed. Use synthetic fixtures and sanitized reproduction data. Optional external fixtures must stay outside the repository, remain read-only, and be configured with `PST_QUICKVIEW_RICH_MSG_FIXTURE` or `PST_QUICKVIEW_LEGACY_MSG_FIXTURE`; tests must verify their bytes are unchanged.

Accepted contributions may make future relicensing require permission from contributors. Do not submit work if you are not willing or authorized to contribute it under the current project license.

Use macOS, Node/npm, Rust/Cargo, and the Tauri v2 prerequisites. Run `npm ci`, then `scripts/agent-check.sh`. For interactive development use `npm run tauri dev`.

Never commit credentials, local paths, extracted content, or generated private data.

Changes must preserve local-only processing, sanitization, blocked remote content, export-first attachments, workspace deletion guards, and exact pins `time = "=0.3.51"`, `cfb = "=0.7.3"`, and `msg_parser = "=0.3.6"`. UI changes need visible focus, semantic labels, adequate contrast, reduced-motion support, and practical pointer targets.

Before a pull request: run the audit, frontend/Rust tests, build, `git diff --check`, and shell syntax checks; describe privacy/security impact; use only synthetic reproduction data; and confirm that no dependency pin or release behavior changed unintentionally.
