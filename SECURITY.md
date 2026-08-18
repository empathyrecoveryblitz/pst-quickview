# Security policy

PST QuickView is beta software. Security fixes are prioritized for the latest beta; older betas may not receive patches.

## Report a vulnerability

Do not report vulnerabilities through a public issue. Use [GitHub Private Vulnerability Reporting](https://github.com/empathyrecoveryblitz/pst-quickview/security/advisories/new). If the form is inaccessible, contact [GitHub Support](https://support.github.com/contact) for access help before sharing sensitive details.

Include the affected PST QuickView version, macOS version, reproduction steps, impact, and any relevant non-sensitive diagnostics. Do not attach real PST, OST, EML, or MSG files, mailbox contents, message bodies, credentials, private logs, workspace databases, or other private data without prior authorization.

The maintainer will make a reasonable effort to acknowledge a report within seven calendar days. An acknowledgment does not guarantee a fix date.

## Product security context

PST QuickView processes sensitive messages locally. Original PST, EML, and MSG sources are read-only; remote content is blocked by default; HTML is sanitized; and attachment Open exports a safe copy first. Reports should use synthetic files and sanitized reproduction steps. Review Diagnostics output for privacy before sharing it privately.
