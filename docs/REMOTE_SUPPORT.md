# Approved app-only Remote Support

This is an implementation candidate, not a packaged-Mac acceptance result.
The current development version is `0.2.0-beta.3.support.3`. The verified
updater and update/resume revision still need fresh native, package, and
genuine-Mac installation/restart validation. No current CI pass or readiness
claim is implied by this document.
This revision keeps the message-read scratch buffer off the blocking worker's
stack. Nested optimized import/repair calls in the earlier build could exhaust
that stack. A constrained-stack regression is required in both debug and release
profiles; this source correction is not a claim of successful testing on a
recipient's Mac. Existing source files and workspaces must be preserved.
The automatic dashboard revision replaces invitation copying. Earlier
invitation-based development ZIPs do not implement this workflow. Validate a
fresh build containing the exact deployed service/key pins before sharing it.
The Mac build tools and WKWebView are unavailable on this Linux host; the separate
GitHub Mac gate must validate each new native revision. Source/build results
do not establish an actual packaged-Mac support connection.
The source-only GitHub development workflow builds universal app-only
development packaging, including a manual-install ZIP and updater archive,
after the native and packaged checks pass, and has no release/tag write
permission. Developer ID signing and notarization are not performed; the
toolchain may leave an ad-hoc
signature, which is not a distribution signature. Publishing a source
snapshot requires explicit owner approval. Mail content and private history
are excluded. A workflow definition is not evidence that its checks have passed.
The recipient needs only PST QuickView. No account, VPN client, Codex, SSH
server, screen-sharing app, camera, microphone, or desktop-capture permission
is required by this design. The technician uses the existing laptop, Node.js,
and a local browser. This is not a general-purpose
remote desktop or a remote terminal.

## Owner workflow

1. The technician opens the private laptop dashboard. Several owners can wait
   in the queue; this dashboard connects to one owner at a time.
2. The owner opens **Help → About PST QuickView → Remote Support**, checks the
   consent naming the configured helper, and chooses **Approve and request
   support**. The request appears automatically. There is nothing to copy and
   no customer account or additional software. Approval closes Help so app
   controls remain accessible; failure leaves Help open for recovery.
3. The technician checks their approval box and chooses **Connect** beside a
   waiting request. A separate bundled customer connection window reports
   negotiation. A queue claim is not proof of a connection: only **Connected**
   means the direct encrypted peer connection was verified.
4. The main window displays a persistent **Stop support** banner, outside Help.
   Either person can stop. Closing the app/window, 30-minute expiry, failed
   route checks, or heartbeat loss ends access. Stop revokes local sharing and
   control before cleanup. Reachable requests disappear from the dashboard on
   its next refresh; if Stop cannot reach the service, its 30-second lease
   expires (the next refresh removes it). There is no unattended access.
   A new ordinary session needs a new local approval. The sole continuity
   exception is the separately owner-approved, one-use update flow below.

The main view is a bounded live text/layout mirror, not a pixel-perfect screen
recording. Visible message text can be included only during the approved
session. Images, icons, native dialogs, other apps, attachments, separate
preview windows, and off-screen text are not intentionally exported. Input
values are not copied; a generic search-field representation is shown instead.
Private invitation/Stop UI is excluded. Ellipsized and occluded content is
conservatively omitted. Complex CSS may be represented incompletely.

The technician can use explicitly marked search, view/layout selectors, folder
navigation, result/conversation selection, reader modes, and scrolling. The
helper can also open and cancel the app's Repair/Delete recovery dialogs.
Opening a dialog performs no repair or deletion. Confirmation requires trusted
local input on the Mac; confirmation buttons are excluded from remote control.
No
arbitrary selector, script, key event, OS command, filesystem API, export,
attachment opening, workspace mutation/deletion, or native-dialog operation is
available through the support protocol. The owner must perform operations
outside this allowlist locally. The technician cannot deploy or execute an
arbitrary fix. The separately approved updater can install only a verified
development app archive, not a script, terminal command, extension, or new
remote-control permission.

### Local sharing-failure notices

The owner sees a fixed, theme-readable category if capture/publication fails:
`S01` for a capture exception, `S02` for a native frame rejection, `S03` when the
native session has already ended, and `S04` for an otherwise unclassified
publication failure. These notices never include raw errors, paths or message
content and are excluded from the shared view. A category identifies the failed
operation, not necessarily the initial cause; for example, `S03` can follow a
separate transport or watchdog shutdown. All such failures still revoke access
and require fresh local approval. They do not enable automatic reconnection.

## Owner-approved development updates

The first updater-capable PST QuickView build must be installed manually on
the Mac. An older app cannot gain an updater merely because a server was
changed. After that initial install, a future verified correction can be
offered through the app without another manual replacement download.

Upgrading from `support.1` to `support.2` requires fresh support approval after
restart because recovery-dialog controls expand the approved app controls.
Choose the update without resumption for this transition, then request support
again. The local grant approval revision rejects older-scope grants; subsequent
updates with the same approved control scope can still resume as described below.

1. The owner opens **Help → About PST QuickView → Development updates** and
   chooses **Check for a development update**. There is no periodic update
   polling or ordinary background update check. A check downloads and verifies
   a candidate; it does not approve installation.
2. The owner chooses whether an existing approved support session should
   resume. Resumption is optional and is meaningful only for a live approved
   session. Updating without support must not authorize a new session.
3. The owner deliberately chooses Update and confirms the native dialog on
   the Mac. The update controls and native confirmation are excluded from
   remote operation. Every future update needs another local approval.
4. If resumption was approved, sharing and controls pause before the restart.
   The same helper dashboard keeps the request marked **Updating/reconnecting**.
   A matching acknowledgement must arrive through the already verified direct
   encrypted channel before a one-use continuity grant can be saved.
5. Only the updater-launched, exact new build can consume that grant, on the
   same Mac boot, within 90 seconds and before the original support expiry.
   The same still-open helper browser must retain its matching grant in RAM.
   Fresh guest/helper ephemeral keys, role tokens, setup keys, and a new direct
   session ID are required. Sharing and controls resume only after fresh
   authentication and direct-route verification. Stop, expiry, service failure,
   or closing the helper page cancels continuation; there is no reconnect to a
   different helper or automatic reapproval after failure.

The app pins the update origin, channel, archive path, and update-signing public
key. It checks the bounded manifest, archive size and SHA-256, and the archive
signature with its signed version; a changed URL, unknown key, stale/downgrade
version, or changed archive fails closed. This does not claim a separate
signature over every manifest metadata field. Source/package gates must bind
each published archive to the audited source commit and compiled support/update
pins. Signing the update archive is not Apple Developer ID signing,
notarization, or Gatekeeper approval.

Only a minimal one-use grant is stored in the Mac's local, nonsynchronized
Keychain. Old queue role tokens, WebRTC keys, mailbox content, and source paths
are not persisted for resumption. The helper's continuity secret is RAM-only,
not in its operator configuration, launcher state, URL, or cloud service.
Fresh successful connection clears the continuity grant; it cannot extend the
original approval. macOS may show a local Keychain or installation permission
prompt, particularly when an unsigned development build changes. Denial must
fail closed; do not bypass Keychain, Gatekeeper, or other OS protections.
If installation or an OS prompt exceeds the short continuation deadline, a
fresh local support approval is required even if the app update installed.

## Technician dashboard

Use Node.js 22.12 or newer. The launcher does not install anything on either
computer. In this repository:

```sh
node scripts/support-dashboard-launcher.mjs
```

The launcher reads the pre-provisioned public service pin and an owner-only
private operator configuration. It binds only `127.0.0.1` and opens the local
browser without printing its private access link or secrets. Reopening the
launcher reuses only an authenticated dashboard with the exact configured
service pin. The existing manual laptop shortcut uses its configured Node.js 24
runtime and this entry point. It is not a
login service and does not save customer approval or start support by itself.
The local server remains available after the launcher exits. The dashboard's
Stop button ends its active request; each new customer session still requires
fresh approval on both sides. Private temporary access state is replaced on a
fresh server launch, not published as an account or shareable dashboard link.

Each claim checks the laptop's current Mullvad route, without changing VPN,
firewall, router, or OS settings. No tunnel, incoming SSH, screen-sharing
service, or customer listening port is needed. Some network pairs still
cannot connect directly; there is no automatic app-data relay fallback.

Provisioning is a separate authorized operation, documented in
`cloudflare/support/README.md`. `scripts/setup-support-dashboard.mjs --init`
creates a fresh private operator signing identity and admin credential under
the private user configuration directory, never replacing an existing one.
The signing key remains on the laptop; only its public pin is shipped in the
app. The admin credential is configured as a Cloudflare Worker secret, never
in the app, public source, URLs, or chat. The hosting account belongs to the
operator and must remain on the usage-limited Free plan unless separately
authorized. Customers do not sign in to Cloudflare.

The updater uses a separate private archive-signing key, also kept locally and
outside the repository. It is not the operator claim-signing key or an Apple
signing identity. Public development software can be staged as Cloudflare
Workers Static Assets, with each archive at most 25 MiB. Only the reviewed
update manifest and immutable archive paths belong in that staging directory;
never point asset hosting at the repository, a workspace, or a mail folder.
This design needs no R2 bucket, paid server, or customer account. Staged source
and a proposed asset configuration are not proof of a deployed working updater.

The old Pinggy invitation launcher is preserved only as a legacy development
harness. Its native invitation command has no permission in any current app
window; it is not the dashboard workflow.

## Network and privacy boundary

Cloudflare holds the anonymous waiting queue and encrypted setup messages.
Cloudflare and Google STUN discover routes. These services see connection
metadata, not the direct app-view frames or control messages. Live metadata
and setup messages expire and are removed; platform backups/security records
can persist under the provider's policies. Stop is not a claim of immediate
physical erasure of provider backups. No mailbox content is stored there.
The main mail window's offline CSP is unchanged. Only the locally bundled,
isolated support window contacts the exact compiled HTTPS service origin.

The app pins the operator's public signing key. Claims are signed and bound
to a fresh request, both ephemeral public keys, verified helper route,
session ID, and expiry. ECDH/HKDF derives a fresh setup key without sending it
to Cloudflare. Signaling is additionally encrypted with directional
HKDF/AES-GCM keys, binding role, session, sequence, and expiry. Only
canonical public UDP server-reflexive candidates are signaled. The laptop's
advertised candidate must match its verified Mullvad exit, and the app verifies
the selected remote route. Session descriptions allow only a data channel;
audio/video and relay candidates are rejected. Both sides approve before
WebRTC/STUN starts. No legacy tunnel-screening header is sent to Cloudflare.
Normal authentication, TLS, exact CORS, and local approval remain required.

After direct encryption/route verification and mutual protocol confirmation,
frames and actions use only that data channel. There is no protocol message
for transferring app data through signaling. The app, controller, and launcher
do not record frames, message text, keystrokes, or mail files. The latest view
and a small recent-frame authorization map are held only in memory. Stop
clears them. A remote technician can independently copy or record what is
shown, so invite only a person the owner trusts.
If the technician uses an AI operator to view or operate that mirror, the
displayed text may also be processed by that tool or its service. Obtain the
owner's explicit approval for that processing before displaying content to
the AI operator; do not describe it as purely offline processing. This does
not authorize mailbox uploads, GitHub mail/screenshot uploads, Cloudflare
frame storage, or recordings by the support tools.

The native broker checks the caller window, session, expiry, main-window
heartbeat, frame size/rate, and fresh target tokens. A support WebView has only
bootstrap, bounded frame polling/action submission, Stop, and narrowly scoped
update-handoff/confirmation permissions.
It has no general mail/indexing command, event, file, dialog, or WebView-creation
rights.
The main DOM bridge independently checks current geometry, visibility,
occlusion, target kind, freshness, and explicit control tags. Email HTML is
marked untrusted and cannot create actionable support controls. A lost main
heartbeat closes the session within five seconds; a lost peer heartbeat within
twelve seconds. Stale canvas content and controls are cleared while awaiting
a fresh frame.

Original mailbox files and canonical indexes are not read or written by the
support subsystem. Existing local source-preservation rules still apply.
Real email, screenshots, identifiers, diagnostics, session credentials, and
logs must never be committed or uploaded to GitHub. Tests contain fictional
DOM/text only and make no use of private fixtures.

## Validation and remaining gate

Automated tests cover real encryption/replay/tampering checks, strict wire
schemas, role/origin/token separation, app-only command scopes, mock-paired
frame/action exchange, explicit approvals, candidate sequencing, resource
bounds, view privacy, stale/hidden controls, and Stop revocation. A real Linux
Chromium browser fixture exercises capture, click/input operation, and private,
off-screen, transparent, and covered-text exclusion. Mocked transport tests
are not evidence of a packaged Mac connection.

Required before distribution: Rust formatting/check/Clippy/tests; fresh
universal app-only Mac build; current packaged CSP/capability inspection;
actual WKWebView secure-context, WebRTC, CORS/preflight, and native broker
checks; and a two-computer approved session against the packaged Mac app.
Test owner Stop, helper Stop, close, freeze, disconnect/reconnect, expiration,
and source-file integrity using fictional data. Test an actual approved update,
Keychain allow/deny/locked states, installed-build identity, restart timing,
one-use same-helper resumption, Stop during each update stage, and failure to
resume after page close, changed keys/builds, stale grants, or deadline expiry.
Outside this explicit update flow, there must be no automatic resumption.
Repeat the existing offline mail/security/import/search gates. Linux browser
success or a generic direct-connectivity PASS does not establish Mac readiness.
