# Short-lived support coordination

This dependency-free Worker is a rendezvous service, not remote desktop software
or a mail-processing backend. It stores anonymous request IDs, ephemeral public
keys, signed helper claims, hashed role credentials, short leases/rate counters,
and opaque encrypted WebRTC setup envelopes. It has no frame/action/upload/file
endpoint, no analytics, no application logs, and no support-history database.
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

## Wire contract

- `POST /v3/requests`: exact `{v:3,id,token,guestPublicKey,expires}`.
  IDs are random 16-byte hex, tokens random 32-byte hex, expiry is at most
  30 minutes. Identical live registration retry is idempotent.
- `GET /v3/requests/:id`: guest bearer only, returns
  `{v:3,id,expires,claim:null|{payload,signature}}`.
- `GET /v3/admin/queue`: admin bearer only, anonymous public rows
  `{v:3,id,guestPublicKey,expires,status:'waiting'|'claimed'}`.
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

The canonical signed payload property order is
`v,id,guestPublicKey,helperPublicKey,expectedExit,expires,sid`. Signatures are
base64url raw 64-byte ECDSA P-256/SHA-256 with canonical low-S. The app verifies
its pinned operator key, request binding, expiry, and public key points before
deriving a shared ECDH/HKDF key bound to the complete payload and signature.
The key never crosses the setup service. Local user consent still gates actual
sharing. A claimed queue entry is not proof that the app view is connected.

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
request/role has one setup reader (128 total, at most 4 MiB buffered); heartbeat
and Stop have separate per-request/role 64-byte readers, so a held upload or
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

Synthetic Node tests exercise the actual exported handler with transactional
fake storage, not a published service. They do not replace an authorized
Cloudflare staging test, a real Mac application session, or package validation.

Reviewed platform references:
[SQLite-backed storage](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/),
[Wrangler configuration](https://developers.cloudflare.com/workers/wrangler/configuration/),
[Free-plan SQLite availability](https://developers.cloudflare.com/changelog/post/2026-07-09-restrict-new-kv-backed-namespaces/).
