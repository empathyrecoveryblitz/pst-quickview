# Approved app-only Remote Support

This is an implementation candidate, not a packaged-Mac acceptance result.
The local application/JavaScript suite currently passes, and the development
dependency audit is clean after the authorized development-only lock updates.
Native checks are blocked on this Linux host because Rust and the Mac build
tools are unavailable. The separate source-only GitHub development workflow
produces only a zipped unsigned universal `.app`, after the native and packaged
checks pass, and has no release/tag write permission. Publishing a source
snapshot requires explicit owner approval. Mail content and private history
are excluded. A workflow definition is not evidence that its checks have passed.
The recipient needs only PST QuickView. No account, VPN client, Codex, SSH
server, screen-sharing app, camera, microphone, or desktop-capture permission
is required by this design. The technician uses the existing laptop, Node.js,
system OpenSSH client, and a local browser. This is not a general-purpose
remote desktop or a remote terminal.

## Owner workflow

1. The technician starts the local support launcher. It creates a private,
   short-lived invitation and a localhost controller. The technician opens the
   controller and deliberately chooses **Start support**.
2. The owner opens **Help → About PST QuickView → Remote Support**, pastes the
   complete private invitation, checks the consent box, and chooses **Approve
   and connect**. No browser/account setup is needed on the owner's Mac.
3. A separate bundled connection window reports negotiation and connection
   state. Only a verified direct encrypted UDP data channel is accepted.
4. The main window displays a persistent **Stop support** banner, outside Help.
   Either person can stop. Closing the main app or support window, invitation
   expiry, failed route checks, or loss of heartbeats ends access. Invitations
   last at most 30 minutes and do not authorize a subsequent unattended session.

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

## Technician launcher

Use Node.js 22.12 or newer. The launcher does not install anything on either
computer. In this repository:

```sh
node scripts/support-session.mjs --pinggy
```

The launcher checks that the laptop is using Mullvad, without changing VPN,
firewall, or router settings. It exposes only an encrypted signaling endpoint
through an outbound SSH tunnel to the official `free.pinggy.io` service. Both
local listeners bind to `127.0.0.1`; the controller listener is never tunneled.
No incoming SSH, screen-sharing service, or listening port is created on the
owner's Mac. This remains a direct-only connection; some network pairs cannot
connect, and there is no automatic data-relay fallback.

SSH uses a dedicated private known-hosts file in the user's local data area,
strict host-key checking, no personal SSH config/agent/identities, and no shell
or debug UI. First-use host trust must be explicit. The optional
`--accept-new-host-key` flag uses OpenSSH trust-on-first-use for that exact
service; it does not independently verify the first key or accept changed keys.
Do not substitute disabled host-key verification. A prior independently
verified service key can instead be placed in that dedicated file.

The launcher prints the location of its private temporary state file, not the
invitation or tokens. Read its `controllerURL` privately and open it in the
local browser; share only its `guestURL` invitation with the owner. The state
directory is mode 0700 and file is 0600. Keep the launcher running during
support. Controller Stop, expiration, or Ctrl+C closes the two listeners and
SSH helper and removes that temporary state. No email data is in this file.

## Network and privacy boundary

Pinggy relays only the setup HTTP traffic. Cloudflare and Google STUN discover
routes. Those services can see network/access metadata. They do not receive
the direct app-view frames or control messages. The main mail window's
offline CSP is unchanged. Only the locally bundled, isolated support window
can contact the exact validated HTTPS signaling origin for the session.

Signaling is additionally encrypted with directional HKDF/AES-GCM keys from
the private invitation, binding role, session, sequence, and expiry. Only
canonical public UDP server-reflexive candidates are signaled. The laptop's
advertised candidate must match its verified Mullvad exit, and the app verifies
the selected remote route. Session descriptions allow only a data channel;
audio/video and relay candidates are rejected. Both sides approve before
WebRTC/STUN starts. The official `X-Pinggy-No-Screen` header is used only for
the app's programmatic guest API requests; normal authentication, TLS, CORS,
and local approval are still required.

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
