# Push-Notification Relay

This document describes `octo-relay` — the push-notification relay that delivers
notifications from self-hosted lairs to mobile devices — covering its deployment,
the trust model, and every anti-spam / anti-abuse control in the request path.

---

## 1. Overview

A lair has no way to reach Apple Push Notification service (APNs) directly: it
holds no Apple credentials, and it sits behind an encrypted Noise tunnel that the
mobile app dials out to. `octo-relay` bridges that gap.

```
 lair  ──signed POST /notify──►  octo-relay  ──HTTP/2──►  APNs  ──►  device
 (Ed25519 private key)          (Apple .p8 key)
```

The relay:

- holds **Apple's APNs key** and forwards pushes — it never sees Noise traffic and
  holds no per-user secrets;
- accepts `/notify` POSTs **signed with a lair's Ed25519 key** and forwards them to
  the device tokens registered under that lair's public key;
- accepts device **registrations**, gated by a proof-of-ownership challenge.

A single relay fans out to many devices and serves many lairs; subscriptions are
keyed on `(device_token, lair_pubkey)`.

The crate lives in `relay/` — `main.rs` (HTTP server + config), `routes.rs`
(handlers), `db.rs` (SQLite registry), `apns.rs` (APNs HTTP/2 client).

---

## 2. Deployment Topology

The relay runs as a plain systemd service on its **own host**, separate from any
lair. TLS is terminated on the box by Caddy; the relay binds loopback only.

```
 Internet ──443──► Caddy (TLS, Let's Encrypt) ──► 127.0.0.1:8080  octo-relay
                                                        │
                                                        ├── /etc/octo-relay/apns.p8     (APNs key)
                                                        ├── /etc/octo-relay/relay.env   (KEY=VAL config)
                                                        └── /var/lib/octo-relay/relay.db (SQLite)
```

- **Process** — `octo-relay.service`, binary at `/usr/local/bin/octo-relay`. The
  unit is hardened: `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome`,
  `MemoryDenyWriteExecute`, `RestrictAddressFamilies`, `ReadWritePaths` limited to
  `/var/lib/octo-relay`.
- **Config** — APNs Key ID / Team ID / Bundle ID come from environment variables
  (`/etc/octo-relay/relay.env`); the `.p8` private key is read from disk at
  startup. **No Apple secret is ever compiled into the binary or committed to the
  repo** — a CI-built artifact is safe to publish.
- **Build & release** — the `relay.yml` GitHub Actions workflow builds per-Linux-arch
  and publishes a `relay-v<version>` release. Deploying = drop the matching-arch
  binary into `/usr/local/bin/octo-relay` and restart the service.

The host is currently a `t4g.nano` (2 vCPU, ~400 MB RAM, burstable CPU credits) —
small enough that flood protection matters (see §7).

---

## 3. HTTP Endpoints

| Method & path           | Auth                              | Purpose |
|-------------------------|-----------------------------------|---------|
| `GET  /health`          | none                              | Liveness probe. |
| `POST /register/challenge` | none (rate-limited)            | Step 1 of enrollment — sends an ownership-proving silent push. |
| `POST /register`        | challenge nonce                   | Step 2 — binds `(device_token, lair_pubkey)`. |
| `POST /unregister`      | none (pubkey shape only)          | Removes a subscription. |
| `POST /notify`          | Ed25519 signature                 | Sends a push to a lair's registered devices. |

---

## 4. Sending a Push — `/notify` Authentication

`/notify` is the authenticated half of the system. It is **not** rate-limited by
identity because forging a request is cryptographically infeasible.

A caller must supply two headers plus a JSON body:

```
X-Lair-Pubkey:  <base32, no padding>   the lair's Ed25519 public key
X-Lair-Sig:     <base64>               Ed25519 signature over the raw body
```

The handler (`relay/src/routes.rs::notify`):

1. decodes `X-Lair-Pubkey` → must be a valid 32-byte Ed25519 key;
2. verifies `X-Lair-Sig` over the **raw request bytes** with that key — a mismatch
   is `401`;
3. checks the body's `ts` is within `FRESH_WINDOW_SECS` (60 s) of server time;
4. checks the body's `nonce` is unique for that pubkey;
5. looks up subscriptions for the pubkey and forwards to APNs.

Only the holder of a lair's Ed25519 **private key** can produce a valid signature,
so an attacker cannot impersonate an existing lair. The public key is published to
mobile over the Noise tunnel (via lair's `/info`), so it never has to be trusted
*through* the relay.

**Replay protection** — `ts` freshness bounds the window, and `record_nonce`
(`db.rs`) records each `(pubkey, nonce)` pair; a repeat within the retention window
is rejected. Nonce rows older than 5 minutes are garbage-collected on every insert,
so the table stays bounded.

---

## 5. Enrolling a Device — the Push-Challenge

Registration binds a device's APNs token to a lair's public key. The hard problem:
an APNs `device_token` is a **routing address, not a secret**. If `/register` were
simply "POST a `(token, pubkey)` pair," anyone who *learned* a victim's token could
bind it under **their own** keypair and then push arbitrary notifications —
including attacker-controlled lock-screen text — to the victim's device.

A signature-based voucher does **not** fix this: the attacker controls their own
lair keypair and can sign anything. The only unforgeable capability tied to a
device token is *receiving a push at it*. So registration is a two-step,
ownership-proving handshake.

```
 mobile                              octo-relay                    APNs
   │  POST /register/challenge          │                            │
   │  {device_token, platform}          │                            │
   ├───────────────────────────────────►│  generate 128-bit nonce     │
   │                                    │  store (token, nonce)       │
   │                                    │  silent push {octo_challenge:nonce}
   │                                    ├───────────────────────────►│
   │  202 Accepted  (no nonce in body!) │                            │
   │◄───────────────────────────────────┤                            │
   │                                                                 │
   │            ◄────── silent push delivered to THIS device ─────────┤
   │  (native handler extracts nonce)                                 │
   │                                                                 │
   │  POST /register                                                  │
   │  {device_token, platform, lair_pubkey, challenge_nonce}          │
   ├───────────────────────────────────►│  consume_challenge()         │
   │                                    │  → bind (token, pubkey)      │
   │  204 No Content                    │                              │
   │◄───────────────────────────────────┤                              │
```

### 5.1 `POST /register/challenge`

`relay/src/routes.rs::register_challenge`:

- validates `platform == "ios"` and `device_token` is hex, non-empty, ≤ 512 bytes;
- generates a fresh **128-bit random nonce** (`gen_nonce`);
- `upsert_challenge` records `(device_token, nonce, created_at)`;
- sends a **silent push** carrying the nonce via `apns::Client::push_background`:

  ```json
  { "aps": { "content-available": 1 }, "octo_challenge": "<nonce>" }
  ```

  (`apns-push-type: background`, `apns-priority: 5` — Apple's requirements for a
  content-available push.)

**The critical invariant: the response is a bare `202` and never contains the
nonce.** The nonce travels *only* via the APNs push. A caller who merely knows the
token string calls this endpoint, gets `202`, and learns nothing — the push lands
on the real device, which they do not control.

### 5.2 `POST /register`

`relay/src/routes.rs::register`:

- validates `lair_pubkey` is a real base32 Ed25519 key and `device_token` ≤ 512 bytes;
- requires `challenge_nonce`; `consume_challenge` **atomically verifies and deletes**
  the matching, non-expired challenge row (single-use);
- on success, upserts the `(device_token, lair_pubkey)` subscription.

A successful `/register` is proof the whole chain worked: the device received the
silent push, extracted the nonce, and echoed it back. The nonce is single-use and
expires after `CHALLENGE_TTL_SECS` (5 minutes).

### 5.3 Mobile side

`mobile/src/registerWithRelay.ts` drives the three steps; the native iOS layer
(`PushModule.swift` + `AppDelegate.swift`) receives the silent push in
`didReceiveRemoteNotification`, extracts `octo_challenge`, and hands it to JS via
`awaitRegistrationChallenge` (with a latch so a push that arrives before JS asks is
not lost). Requires the `remote-notification` background mode in `Info.plist`.

---

## 6. Other Anti-Abuse Controls

| Control | Where | What it bounds |
|---|---|---|
| **Challenge cooldown** | `db.rs::upsert_challenge` — `CHALLENGE_COOLDOWN_SECS` (30 s) | A caller cannot spam silent pushes at a device: within the cooldown the relay records no new challenge and sends no push. |
| **Input validation** | `routes.rs` — `valid_lair_pubkey`, `MAX_DEVICE_TOKEN_LEN` (512) | `/register` and `/unregister` reject malformed `lair_pubkey` and oversize `device_token`, bounding what an unauthenticated write can persist. |
| **Subscription TTL prune** | `db.rs::prune_stale_subscriptions`, scheduled in `main.rs` | A `last_seen` column is refreshed on every (idempotent) re-register; a background task every 6 h deletes rows older than `--subscription-ttl-days` (default 30). Live devices re-register on each chat-mount and stay; abandoned or abusively-created rows age out, bounding table growth. |
| **Dead-token pruning** | `routes.rs` + `apns.rs` | When APNs reports `410 Unregistered` / `400 BadDeviceToken`, the relay drops the subscription (`forget_invalid_token`). |
| **Replay cache GC** | `db.rs::record_nonce` | `/notify` nonce rows are GC'd after 5 minutes so the replay cache stays bounded. |

The challenge-push-spam vector is intrinsic — `/register/challenge` must accept a
raw token from any caller to onboard new devices — so it is *rate-limited*, not
eliminated. The cooldown plus the on-box limits (§7) keep it to a harmless trickle
of background wakes.

---

## 7. On-Box DDoS / Flood Hardening

The relay logic is cheap (an Ed25519 verify, a SQLite point query). The real cost
under a flood is **TLS handshakes** draining the `t4g.nano`'s burst CPU credits.
The application controls in §4–6 do not help against raw connection floods, so the
host carries a transport-layer layer:

- **nftables** (`/etc/sysconfig/nftables.conf`) — table `inet ddos_guard` with a
  per-source new-connection rate limit on ports 80/443 (40 conn/s, burst 80; excess
  dropped). The chain is `policy accept` — the EC2 security group is the real
  firewall, so a rule mistake fails open rather than locking the box out.
- **fail2ban** (`/etc/fail2ban/jail.local`) — an `sshd` jail (brute-force) and a
  `caddy-flood` jail that bans any IP exceeding 240 requests / 60 s, read from
  Caddy's JSON access log.
- **Caddy** — `request_body max_size 16KB` (relay payloads are well under 1 KB),
  server timeouts (`read_header 5s`, `read_body 10s`, `write 30s`, `idle 2m` — kills
  slow-loris connections), and JSON access logging to `/var/log/caddy/access.log`.
- **sysctl** (`/etc/sysctl.d/99-relay-hardening.conf`) — SYN cookies on, larger
  `tcp_max_syn_backlog` / `somaxconn` / `netdev_max_backlog`, fewer synack retries.
- **AWS Shield Standard** — free and automatic at the AWS edge; absorbs common
  L3/L4 (SYN/UDP reflection) attacks. Does nothing for L7.

These stop a single noisy client or a small botnet — which is essentially all a
relay this size will ever see. A genuinely *distributed* volumetric attack
saturates the host before any rule runs; that requires upstream absorption (see §8).

---

## 8. Known Gaps & Future Work

### `/unregister` is not ownership-gated

`/register` requires the push-challenge, but `/unregister` only validates the
`lair_pubkey` shape. A caller who knows a victim's `device_token` + `lair_pubkey`
can delete that subscription, silently disabling the victim's notifications until
they next open the app (re-registration is idempotent). Lower-stakes than the
`/register` hijack — denial, not attacker-controlled content — but the same
challenge mechanism would close it. Tracked in `TODO.md`.

### Single APNs gateway

`apns.rs` talks to one gateway, selected by `APNS_PRODUCTION` at startup. A device
token's environment is fixed by the app build: a debug build (Xcode → device) gets
a **sandbox** token; a TestFlight / App Store build gets a **production** token.
The production gateway rejects sandbox tokens with `BadDeviceToken` and vice versa.
Today the relay must be configured to match the build under test. A dual-gateway
fallback — retry the other gateway on `BadDeviceToken` — would let one relay serve
both. Tracked in `TODO.md`.

### Distributed volumetric attacks

The §7 hardening is on-box and cannot help once the host's bandwidth or CPU is
saturated by a large distributed flood. Fronting the relay with CloudFront + AWS
WAF (edge absorption + rate-based rules, no DNS migration) is the planned upstream
defense. Tracked in `TODO.md`.
