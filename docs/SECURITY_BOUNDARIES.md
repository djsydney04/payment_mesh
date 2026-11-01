# Trust, privacy, and security boundaries

[Documentation index](README.md) · [Protocol](PROTOCOL.md) · [Architecture](ARCHITECTURE.md)

This explains the current design and its limits. It is not an independent audit or a claim of production readiness. The application uses established cryptographic primitives, but the protocol composition, native radio behavior, banking integration, and operational controls still require independent review and real-environment acceptance.

## What each component is trusted to do

| Component | Authority / boundary |
|---|---|
| Wallet identity | Authorize actions with its Ed25519 key; possession of the seed is control of the wallet |
| Swift app and device | Present the intended recipient/amount, protect the seed/journal, request device confirmation, operate Apple APIs |
| Rust on device | Validate/sign typed instructions, encrypt/decrypt, frame packets, check local reservations |
| Nearby peer/relay | Untrusted transport; cannot establish a balance or alter signed fields without rejection |
| HTTP server and SQLite | Authenticate callers, select wallet-bound resources, check funds, own authoritative local accounting |
| Plaid connection | Obtain the selected test bank connection and processor token; not the wallet's settlement authority |
| Dwolla Sandbox adapter | Request and observe test transfers with server-owned resource bindings |
| Operator/filesystem | Protect keys, configuration, database, backups, and external account access |

The server sees the plaintext payment submitted for settlement. End-to-end packet encryption protects the radio path from intermediate relays, not from the ledger that must apply the instruction. A compromised server or wallet process remains outside the protection provided by signatures over honest requests.

## Identity and device confirmation

A wallet starts from a random 32-byte seed. Rust derives its signing and encryption identity; the phone stores the seed in Keychain with device-only, after-first-unlock accessibility. The app asks for Face ID or the device passcode before authorizing payment and rechecks availability afterward.

Confirmation is a native-app behavior, not a wire-level proof of biometric presence. Anyone controlling the seed or modifying the application can sign directly. The seed is not backed by a recovery service, server password, or implemented key-rotation system. The server cannot recover it from the public key.

Names are self-signed display labels. They do not establish a person's identity. Compare the code shown by the friend's phone to reduce name confusion. The short displayed fingerprint is a convenience check derived from the public key; it is not an external identity credential or a substitute for stronger verification where required.

## Radio confidentiality and visibility

Signed profiles bind a display name, signing key, encryption key, version, and creation time. A payment is separately signed and bound to its sender, recipient, cents, memo, expiry, UUID, and ledger UUID. An ephemeral X25519 exchange and HKDF derive a ChaCha20-Poly1305 key for the packet; envelope headers are authenticated. The recipient verifies both envelope and inner instruction.

Relays can see public routing keys, packet IDs, expiry, hop information, timing, and sizes. Nearby profile names/keys are also visible through discovery. They cannot read the encrypted amount/memo through the intended protocol. The ledger later receives those fields in plaintext over its HTTP channel.

Encryption uses a long-term recipient key. Recording ciphertext and later obtaining that key can expose past packets; the protocol does not provide forward secrecy. It does not conceal the payment graph or make a radio user anonymous.

The honest relay budget starts at four hops. The hop counter is intentionally mutable and is not signed, so it cannot enforce a malicious radio's behavior. Packet-size, fragment-count, assembly, queue, and cache bounds reduce resource use but do not provide robust radio admission control or a complete flood defense.

## HTTP authentication and replay

Wallet RPC requests sign a typed body with domain separation, a timestamp, a fresh nonce, action, and payload. The server accepts limited clock skew and durably consumes nonces. It requires enrollment for actions after registration and limits payment submission to the sender or recipient. A relayed packet does not authorize an intermediary to act as either party.

Financial operation UUIDs are stable across retries and checked against immutable inputs. The ledger UUID prevents an instruction from being accepted as a payment on a different database. Request replay protection and financial idempotency solve different problems; both are necessary.

Release clients require HTTPS. Debug builds permit selected local-development addresses. Signatures authenticate the wallet's request body; they do not encrypt HTTP responses or authenticate a server response to a phone by themselves. TLS and correct server configuration remain essential. The current snapshot response is not a separately signed portable proof of funds.

## Stored information

| Location | Contents and protection |
|---|---|
| iOS Keychain | Wallet seed; device-only accessibility policy |
| iOS Application Support journal | Cached snapshot, signed pending instructions, stable bank/transfer intents; file protection and backup exclusion |
| Server database | Public identities, ledger history, amounts/memos, receipts, setup/outbox state, encrypted credential fields |
| Server vault key | Separate configured secret or 0600 file next to the database |
| Provider accounts | Sandbox customer/bank resources and external test-transfer history |

The local journal is not independently application-encrypted; it relies on iOS file protection. It can temporarily contain a Plaid public token while saving an unacknowledged bank-link intent. Plaid access and processor tokens stay server-side. Setup stages encrypt those values and discard them when the next durable resource replaces them. Bank credentials are never included in Bluetooth payments.

Server credential encryption uses ChaCha20-Poly1305 with owner-bound associated data. The vault key does not encrypt names, amounts, memos, or the entire database. Losing the key loses credential access; changing it is not rotation. See [backup and restore](OPERATIONS.md#backup-and-restore).

## Money authorization and provider observations

Client availability is advisory. The server serializes balanced accounting and prevents user accounts from going negative. Provider payments/cash outs reserve funds before a transfer outbox record can be submitted. Deposits and recipient credits wait for authoritative processing. Repeated observations use applied/reversed flags and an atomic event cursor to avoid repeated accounting effects.

The adapter checks transfer identity, source, destination, amount, and correlation, including bounded related transfer legs. Authenticated webhooks only wake reconciliation; a notification topic cannot mint funds. Ambiguous outcomes, contradictory observations, returns, and balance drift can freeze a wallet instead of guessing a favorable result. These safeguards do not make ACH processing irrevocable.

Customer and funding-source resources are selected by the server and bound to the signed wallet. Clients cannot provide arbitrary provider resource URLs to fund or pay out. Current customer setup accepts synthetic test profiles; this does not establish a production identity-verification program. The demo faucet is once per wallet key, so new keys can obtain new test grants. It has no real-person uniqueness guarantee.

## Current limits before a live product

The implementation deliberately refuses production endpoints. A live product additionally needs a provider-approved custody/funding model, validated identity/account ownership and authorization, defined availability and return policies, fraud/abuse controls, operational review and dispute handling, disaster recovery, key management/recovery, and independent assessment. These are product and operational decisions as well as code changes.

This repository does not supply production rate/admission controls, a distributed service design, comprehensive telemetry/alerting, a general manual-review resolution workflow, or a full history/export API. Physical BLE/background behavior and external Sandbox transfers also require the acceptance work listed in [verification](VERIFICATION.md). Do not treat a successful local demonstration as evidence that those boundaries have been exercised.
