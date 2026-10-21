# Short-lived support coordination

This dependency-free Worker is a rendezvous service, not remote desktop software
or a mail-processing backend. It stores anonymous request IDs, ephemeral public
keys, signed helper claims, hashed role credentials, short leases/rate counters,
and opaque encrypted WebRTC setup envelopes. It has no app-frame/action or
customer-file upload endpoint, no analytics, no application logs, and no
support-history database. Separately approved update hosting serves public
development software only, never customer content.
Actual app drawings and allowlisted controls travel only over the separately
verified direct encrypted peer connection. Direct connectivity can fail; there
is no TURN or other app-data relay fallback.

## Configuration and authority

Nothing here is deployed or provisioned by adding these files. Installing build
tools, creating hosting resources, setting credentials, and deploying require
separate owner authorization. Use an operator-owned Cloudflare account on the
Workers Free plan; recipients need neither an account nor additional software.
Do not enable a paid plan or a quota-overage upgrade without new authorization.
Free quotas and provider availability are limits, not an uptime guarantee.

The checked-in public origin and signing key are owner-selected pins. The
service remains unusable without its separately configured admin secret. Before
an authorized deployment, verify `SERVICE_ORIGIN` as the exact canonical HTTPS
`<worker>.<account>.workers.dev` origin, `OPERATOR_PUBLIC_KEY` as the canonical
base64url raw 65-byte uncompressed P-256 signing public key, and `ADMIN_TOKEN`
as a secret random 64-character lowercase hexadecimal value. Optional
`OPERATOR_LABEL` must match the reviewed operator label. The operator signing
private key stays on the helper laptop; it is never uploaded to Cloudflare or
shipped in PST QuickView. Do not paste private keys or admin credentials in chat.

Ship the reviewed public origin/key/label in `public/support/service.json` only
after deployment and source/package checks. Missing or empty pins fail closed.
Do not fetch replacement trust pins from this service. Rotation requires a
separately reviewed app configuration update. Preserve disabled observability,
invocation logs, Logpush, previews and CLI metrics; do not add log or tail sinks.

The current development app version is `0.2.0-beta.3.support.1`. The update and
continuity implementation is staged source, not evidence that its Mac build,
packaged installation, Keychain, restart, or genuine-Mac support gates passed.
The first updater-capable app needs a manual installation; only later verified
corrections can use its in-app updater. Customers still install only PST
QuickView and approve locally. They need no hosting account or extra software.

## Wire contract

- `POST /v3/requests`: exact `{v:3,id,token,guestPublicKey,expires}`.
  IDs are random 16-byte hex, tokens random 32-byte hex, expiry is at most
  30 minutes. Identical live registration retry is idempotent.
- `GET /v3/requests/:id`: guest bearer only, returns
  `{v:3,id,expires,claim:null|{payload,signature}}`; a resumed claim also has
  `resumeProof` inside `claim`.
- `GET /v3/admin/queue`: admin bearer only, anonymous public rows
  `{v:3,id,guestPublicKey,expires,status:'waiting'|'claimed'}`. During an approved
  update, status is `updating` with `handoff`, then `resuming` with `handoff`,
  `resumeGuestProof`, and `guestTokenSha256`.
- `POST /v3/admin/requests/:id/claim`: admin bearer plus a locally signed claim.
  Atomically claims once and returns `{token,claim}`. Exact retry returns the
  same controller token; changed or cross-request claims fail.
- `POST /v3/requests/:id/api/send` and `GET .../api/poll?after=N`:
  role-specific bearer, existing encrypted setup-envelope format. Plain
  `/send` and `/poll` aliases are equivalent. Admin credentials cannot signal.
- `POST .../heartbeat` with `{}`: each role independently refreshes its lease.
  Send every 10 seconds while waiting or connected. A failed/expired heartbeat
  must close the local direct peer; server deletion alone cannot revoke an
  already established direct channel.
- `DELETE /v3/requests/:id` or `POST .../stop` with `{}`: either session bearer
  revokes/deletes the live record and both encrypted queues immediately.
  While a handoff is pending, use nonce-bound revocation instead; old ordinary
  Stop credentials cannot delete a newly resumed claim.

The canonical signed payload property order is
`v,id,guestPublicKey,helperPublicKey,expectedExit,expires,sid`. Signatures are
base64url raw 64-byte ECDSA P-256/SHA-256 with canonical low-S. The app verifies
its pinned operator key, request binding, expiry, and public key points before
deriving a shared ECDH/HKDF key bound to the complete payload and signature.
The key never crosses the setup service. Local user consent still gates actual
sharing. A claimed queue entry is not proof that the app view is connected.

## One-use approved update continuity

The owner must choose an update and confirm installation in a native dialog.
These controls are not remotely actionable. Updating does not grant the helper
a terminal, arbitrary code execution, desktop access, or additional app controls.
Without explicit update-resume approval, support stops at the update.

The canonical handoff order is
`v,parentId,parentSid,nonce,targetVersion,artifactSha256,nativeSessionId,originalExpires,resumeDeadline,resumeTicketSha256`.
Version is `1`; IDs/nonces are lowercase 32-character hex and hashes/secrets/
proofs are lowercase 64-character hex. The restart deadline is at most 90
seconds and cannot exceed the original session expiry. A handoff binds the
existing direct session, exact target version/archive, and native session.

- The claimed guest posts the exact handoff without `resumeTicketSha256` to
  `POST /v3/requests/:id/handoff`. The Worker returns a distinct one-use
  `resumeTicket` plus the complete `handoff` with its ticket hash. Old setup
  signaling is frozen. The ticket stays with the customer; the continuity
  secret never enters the Worker.
- Only the already verified direct channel carries `{handoff,resumeAuth}` to
  the current helper. It returns a domain-separated HMAC acknowledgement bound
  to every handoff field. Frames/actions are disabled throughout the pause.
  Native validation and local approval are required before the customer can
  save a minimal nonsynchronized Keychain grant and restart.
- The helper retains one matching grant in RAM only. Its local authenticated
  loopback `/api/resume` also requires knowledge of that secret: a new browser
  page cannot borrow a lingering Node-process grant. No continuity credentials
  are saved in operator configuration, launcher state, URLs, logs, or chat.
- The continuing old controller uses `POST .../handoff/heartbeat` with `{nonce}`
  every 10 seconds while status is `updating`. Customer resume rotates both old
  role tokens atomically; an old heartbeat racing that transition must verify
  the current `resuming` row rather than authorize another ordinary session.
- `POST /v3/requests/:id/resume` uses the one-use ticket and fresh guest
  credentials. The guest proof binds the canonical handoff, fresh guest public
  key, and fresh guest-token hash. The same helper verifies that proof locally
  before signing a fresh claim and adding a helper HMAC proof over the handoff,
  canonical new claim payload, and signature. The customer verifies both the
  pinned operator signature and that continuity proof before peer approval.
- `POST /v3/admin/requests/:id/handoff/stop` with admin bearer and `{nonce}`
  revokes only that pending handoff; the customer's ticket/authorized pending
  session can likewise use `POST .../handoff/stop`. Fresh claim removes this
  authority. Late old-peer Stop cannot delete the new role tokens/session.

Fresh connection uses new guest/helper ephemeral keys, role tokens, setup keys,
and a new direct session ID. Old role credentials/WebRTC keys are not persisted
for restart. Successful connection clears continuity secrets. Stop, expiry,
service failure, helper-page close, wrong build/boot, and failed authentication
close continuation. A normal new support session still needs local approval.
macOS may require a local Keychain/install prompt; denial must fail closed.
Do not bypass OS protections or substitute plaintext grant files.

## Public development update assets

Use Workers Static Assets on the existing usage-limited Free account, not R2
or a paid server. Cloudflare's individual static-asset limit is 25 MiB; the
native downloader enforces the same archive bound. Free quotas and availability
still apply. Customers need no Cloudflare account. This is software delivery,
not a mailbox upload or parsing service.

The configured `cloudflare/support/update-assets` staging directory starts with
only reviewed `.gitignore`, `.assetsignore`, and `_headers` control files; no
manifest or archive is checked in. `.gitignore` prevents software payloads from
being committed. `.assetsignore` excludes everything except
`/updates/development.json`, reviewed immutable archives under
`/updates/artifacts/<version>/<sha256>.app.tar.gz`, and the `_headers` rules
which Cloudflare parses rather than serves. A path allowlist is not signature
verification: every staged archive still needs the artifact/source gates.

An authorized deployment may use Wrangler's documented `--assets` directory
override for an equivalently allowlisted public staging copy outside the
private-key directory. Copy the reviewed `.assetsignore` and `_headers` controls
too; inspect the complete selected directory before deploying. No deployment
is performed by these configuration files. Never bind assets to the repository root,
app profiles, source fixtures, private operator configuration, signing keys, or
support screenshots/logs. Preserve queue Durable Object bindings/migrations;
the `ASSETS` binding must not shadow its API, and `/v3/*` runs the Worker first.
HTML handling and not-found handling are explicitly `none`, so there is no
HTML rewrite or SPA fallback for unrecognized paths. The manifest has
`Cache-Control: no-store`; content-bound archives have immutable caching and
`no-transform`. A staging directory or configuration change is not proof of a
deployment or of actual response headers.

The app separately pins the exact configured update origin/channel/path and
archive-signing public key. The private update-signing key stays locally outside
source and is separate from operator claim signing. Tauri's signed-version
archive verification prevents substituting another version; native checks also
enforce the exact URL, SHA-256 and size. Do not describe this as a separate
signature over every manifest metadata field or as Apple Developer ID
signing/notarization. No Gatekeeper changes are authorized.

Before publishing a manifest, finish the source/native/universal-package gates,
bind the software and compiled pins to the reviewed public source commit,
verify the signed archive, and stage all referenced immutable assets together.
The manifest must not be cached as an old update announcement; archive names
are content-bound and must never be silently replaced. Preserve required older
archives when replacing a manifest. No private history, mail, credentials, or
customer data belongs in any hosted artifact. Update checks happen only after
the owner chooses Check in the app, not on a periodic background schedule.

## Bounds, cleanup and privacy

One SQLite-backed Durable Object holds at most 64 live requests. Transactional
KV methods use that SQLite storage; there is no Workers KV/D1/database binding.
Each role may send 128 sequential, retry-idempotent setup envelopes and at most
256 KiB of ciphertext. Individual stored messages stay below 32 KiB. Request
bodies, concurrency, calls, global rates, per-minute HMAC-pseudonymous client
rate keys and anti-replay tombstones are bounded. No raw client address is
stored. Duplicate request IDs cannot resurrect after Stop during their original
lifetime. Each role has a 30-second lease; expiry or either stale lease deletes
the entire live request. Alarms and request-time cleanup both enforce expiry.

Upload reads occur outside the serialized state transaction queue. Anonymous
registration readers are capped at eight and admin claims at two. Each
request/role has one setup reader (192 total including one-use resume tickets,
at most 6 MiB buffered); heartbeat and Stop have separate per-request/role
64-byte readers (384 total including nonce-bound ticket/admin revocation,
at most 24 KiB), so a held upload or
heartbeat cannot occupy another recipient's slots or block Stop. All body
reads retain a three-second deadline. Fast admission/state work has its own
32-slot bound, reserved in addition to bounded body readers.

Short-lived anti-replay IDs remain until their original expiry; rate counters
expire at the minute boundary. These are not support histories. Stop removes
live state when the service is reachable. Offline Stop must revoke local access
immediately; remote disappearance is then bounded by the lease, not promised
as synchronous. Cloudflare necessarily processes network metadata and may keep
platform backups/PITR or security metadata under its own policies; live TTL
deletion is not a claim of immediate provider-level physical erasure.

If the helper uses an AI operator, displayed app text may additionally be
processed by that tool/service only with the owner's explicit approval. The
direct peer boundary does not make an AI operator an offline processor. Do not
upload mail, support views, screenshots, credentials, or private history to
GitHub, the coordination service, or the static software asset directory.

Synthetic Node tests exercise the actual exported handler with transactional
fake storage, not a published service. They do not replace an authorized
Cloudflare staging test, a real Mac application session, or package validation.
The verified updater also requires genuine-Mac tests of native local approval,
Keychain allow/deny/locked states, actual installation/restart, exact new-build
identity, Stop at each stage, same-helper one-use continuation, changed/stale
credentials, short/absolute deadlines, and synthetic source-file integrity.
Source tests or an earlier CI result do not establish this revision's readiness.

Reviewed platform references:
[SQLite-backed storage](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/),
[Wrangler configuration](https://developers.cloudflare.com/workers/wrangler/configuration/),
[Free-plan SQLite availability](https://developers.cloudflare.com/changelog/post/2026-07-09-restrict-new-kv-backed-namespaces/),
[Static Asset limits](https://developers.cloudflare.com/workers/platform/limits/#static-assets),
[Static Asset billing](https://developers.cloudflare.com/workers/static-assets/billing-and-limitations/),
[Worker-first asset routing](https://developers.cloudflare.com/workers/static-assets/routing/worker-script/),
[Asset exclusion rules](https://developers.cloudflare.com/workers/static-assets/binding/#ignoring-assets),
[Asset response headers](https://developers.cloudflare.com/workers/static-assets/headers/),
[Deployment directory override](https://developers.cloudflare.com/workers/ci-cd/builds/configuration/#deploy-command).
