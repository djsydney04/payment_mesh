# MeshPay protocol v1

## Settlement model

The asset is a USD liability on one authoritative ledger, identified by a persisted random UUID. A signed payment is a 24-hour instruction to that ledger; it is not cash, a bearer asset, a proof of balance, or a promise of final offline settlement. Bluetooth carries the instruction without requiring internet. Either sender or recipient can later submit it. Third-party relays cannot submit it to the ledger on their own.

Sender-local reservations reduce the displayed available balance until synchronization. They improve the experience but cannot prevent an adversarial or second client from producing competing instructions. The server serializes those instructions in an immediate SQLite transaction and rejects overspending. Insufficient-funds declines are durably terminal; retries cannot revive a declined instruction after a top-up.

All amounts use signed 64-bit integer cents, bounded to 1–100,000 ($0.01–$1,000). Floating point is never used to make or settle a payment. Only USD is supported.

## Identity and trust

A wallet has a random 32-byte seed. Ed25519 signs messages. A distinct X25519 private key is derived using HKDF-SHA256 with salt `meshpay/identity/v1` and info `x25519`. The seed is held in the iOS Keychain with device-only accessibility. The server receives public keys, never private keys.

A self-signed profile binds protocol version, signing key, encryption key, display name and creation time. Profiles expire after 24 hours and allow 120 seconds of future clock skew. A signature proves control of a key, not the identity of a human. Names can collide. Compare the 12-hex-character fingerprint out of band; it is the first 12 characters of SHA-256 of the lowercase signing-key hex string, grouped in fours.

The server is trusted to maintain and report balances. TLS is required by Release builds. There is no key recovery, multi-device key rotation, hardware-backed attestation, or independently verifiable bank reserve in this prototype.

## Signed structures

JSON uses UTF-8, lowercase hexadecimal keys and signatures, and standard UUID strings. Signatures cover `domain UTF8 || 0x00 || compact JSON of the typed body`. Rust's declared struct field order is authoritative. JSON `Value` maps in RPC payloads use sorted keys. Unknown fields are rejected in signed structures. Never verify arbitrary raw JSON bytes or substitute a different canonicalization.

Payment body order:

```
version, ledger_id, id, sender, recipient, amount_cents, currency,
memo, created_at, expires_at
```

The domain is `meshpay/payment/v1`. The signature and body are wrapped in a `Payment`. The memo is limited to 80 characters, and control characters are forbidden. Sender and recipient must differ. Both must be registered before settlement. The ledger UUID prevents an instruction from being accepted by an unrelated database.

RPC body order:

```
version, public_key, timestamp, nonce, action, payload
```

The domain is `meshpay/rpc/v1`. The endpoint is `POST /v1/rpc`. Requests expire after 120 seconds in either direction. The server records `(public_key, nonce)` durably and consumes each request nonce once, including unsuccessful action attempts. Retry with a fresh RPC nonce and the same payment or withdrawal UUID. The server's UUID receipt handles action-level idempotency. Request bodies are capped at 32 KiB.

Actions: `register` (signed profile), `snapshot`, `settle` (signed payment), `link_token`, `link_bank` (`id`, `public_token`, `account_id`), `withdraw` (`id`, `amount_cents`), the provider Sandbox actions `bank_enroll` (`id`, synthetic `profile`, `synthetic_test_data`) and `bank_fund` (`id`, `amount_cents`, `authorize_test_debit`), and the local demo actions `demo_fund` and `demo_link_bank`. Demo actions are refused on provider-backed databases.

Response: `{"ok":true,"data":...}` or `{"ok":false,"error":{"code":...,"message":...}}`. Framework-level malformed-body errors can have a different format. Internal database and provider details never enter the API's user-facing errors.

## Sealed mesh packets

An ephemeral X25519 sender key agrees with the recipient's signed encryption key. A non-contributory shared secret is rejected. HKDF-SHA256 uses salt `meshpay/sealed/v1` and info `chacha20poly1305` to derive a 32-byte key. ChaCha20-Poly1305 encrypts the full signed payment with a random 12-byte nonce.

Associated data is UTF-8:

```
meshpay/sealed/v1:{payment UUID}:{sender hex}:{recipient hex}:{expires_at}
```

The outer envelope body contains `version, id, sender, recipient, expires_at, ephemeral_key, nonce, ciphertext` and is signed with domain `meshpay/envelope/v1`. The recipient checks both signatures and that the inner instruction matches the outer identity, ID and expiry. Relays validate the signed outer envelope without decrypting amounts or memos. Sender/recipient public keys, expiry, timing and packet sizes remain visible. Static recipient keys mean this design does not provide forward secrecy after recipient-key compromise.

`MeshPacket` adds a mutable `hops_remaining` counter, initially four. Honest relays decrement it and stop at zero. The hop counter is not signed; malicious relays can reset it. Bounded caches and expiry limit honest resource use, but this is not a proof of path length or a solution to radio flooding. There is no radio acknowledgement that can be mistaken for settlement.

## BLE service

| Attribute | UUID suffix / behavior |
|---|---|
| Service | `8F650001-4D45-5348-5041-590000000001` |
| Profile | `8F650002-4D45-5348-5041-590000000001`, readable signed JSON profile, long-read offsets supported |
| Packets | `8F650003-4D45-5348-5041-590000000001`, write with response and notify |

Each value begins with a 20-byte header: payment UUID raw bytes (16), big-endian zero-based fragment index (2), big-endian total count (2). Remaining bytes carry the compact packet JSON. The MTU is 40–512 bytes; the final frame can be shorter. Each connected peer's actual write/notification limit determines the MTU. Links with less than 40 bytes available are skipped. Central writes and peripheral notifications honor CoreBluetooth backpressure.

Reassembly is keyed by source radio and UUID, supports out-of-order and duplicate fragments, rejects conflicting duplicates and changing counts, caps complete packets at 16 KiB, and permits 32 incomplete assemblies with 30-second expiry. It permits up to 1,024 fragments per packet. A validated completed packet enters a 2,048-ID duplicate cache before delivery or relay. The Swift bridge caps connections at eight, relay packets at 128 and send queues at 4,096 frames.

Packet queues for the wallet's own payments are durable on iOS. Intermediate relay packets are memory-only and bounded; the mesh does not guarantee delivery if devices leave range, the app is killed, or the operating system suspends it. Discovery lists direct neighbors; a known packet may travel through relays, but the UI does not discover friends through an unbounded global directory.


## Provider-backed ledger states

`MESH_RAIL=dwolla_sandbox` persists the rail identity in a separate database. A payment reservation and its immutable outbox record commit together. `settle` may return `queued`, `submitting` or `submitted`; only `settled` means provider processing was observed and the recipient credit committed. Terminal `failed`/`cancelled` decisions release a reservation once. `returned`/`needs_review` can require a frozen wallet and operational reconciliation. The Swift client never turns a pending receipt into completion.

Provider setup/funding requests are signed HTTP actions and are never sent through the Bluetooth mesh. The public API accepts no customer/funding-source IDs. Verified customer and bank resources are selected from server-owned records. Bank deposit authorization is a one-time Sandbox debit tied to a stable UUID. Provider observations must match the operation's exact cents, source, destination, correlation ID and transfer ID before being applied. See [provider accounting](BANK_PROVIDER.md).


Bank linking has its own durable intent UUID. On the provider rail, `link_bank` returns `id` and the current setup status after encrypting and committing the request, before any provider call. Repeating the same UUID and immutable input returns that intent's status; changing its wallet, token or selected account is an ID conflict. The server then exchanges the one-time public token, commits encrypted Item credentials, validates the selected account, and commits a processor token before contacting Dwolla. Each stage uses a fenced lease. A consumed/expired Plaid session can become `relink_required` only before a Dwolla resource exists. A new sign-in gets a new intent, and late retries of the old intent cannot replace it.
