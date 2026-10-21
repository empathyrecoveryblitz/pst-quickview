# Approved app-only Remote Support

This is an implementation candidate, not a packaged-Mac acceptance result.
The automatic dashboard revision replaces invitation copying. Earlier
invitation-based development ZIPs do not implement this workflow. Validate a
fresh build containing the exact deployed service/key pins before sharing it.
Rust and the Mac build tools are unavailable on this Linux host; the separate
GitHub Mac gate must validate each new native revision. Source/build results
do not establish an actual packaged-Mac support connection.
The source-only GitHub development workflow
produces only a zipped universal development `.app`, after the native and
packaged checks pass, and has no release/tag write permission. Developer ID
signing and notarization are not performed; the toolchain may leave an ad-hoc
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
   expires (the next refresh removes it). No automatic restart or unattended
   access is authorized. A new session always needs a new local approval.

The main view is a bounded live text/layout mirror, not a pixel-perfect screen
recording. Visible message text can be included only during the approved
session. Images, icons, native dialogs, other apps, attachments, separate
preview windows, and off-screen text are not intentionally exported. Input
values are not copied; a generic search-field representation is shown instead.
Private invitation/Stop UI is excluded. Ellipsized and occluded content is
conservatively omitted. Complex CSS may be represented incompletely.

The technician can use explicitly marked search, view/layout selectors, folder
navigation, result/conversation selection, reader modes, and scrolling. No
arbitrary selector, script, key event, OS command, filesystem API, export,
attachment opening, workspace mutation/deletion, or native-dialog operation is
available through the support protocol. The owner must perform operations
outside this allowlist locally. The support feature does not install app
updates or permit the technician to deploy/execute arbitrary fixes remotely.

## Technician dashboard

Use Node.js 22.12 or newer. The launcher does not install anything on either
computer. In this repository:

```sh
node scripts/support-dashboard.mjs
```

The launcher reads the pre-provisioned public service pin and an owner-only
private operator configuration. It binds only `127.0.0.1` and prints the
location of a private temporary state file, not its capability URL or secrets.
Open its `dashboardURL` privately in the local browser. Keep the launcher
running during support; stopping it removes the temporary state and ends its
active request. A fresh launch generates a new dashboard access capability.

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

The native broker checks the caller window, session, expiry, main-window
heartbeat, frame size/rate, and fresh target tokens. A support WebView has only
bootstrap, bounded frame polling/action submission, and Stop permissions.
It has no application command, event, file, dialog, or WebView-creation rights.
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
no automatic resumption, and source-file integrity using fictional data.
Repeat the existing offline mail/security/import/search gates. Linux browser
success or a generic direct-connectivity PASS does not establish Mac readiness.
