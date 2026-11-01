# Mesh documentation

Mesh is a nearby-payments app: Bluetooth carries a signed instruction between phones, and a Rust server checks and records its financial result. The native iPhone app supplies the screens, radio access, secure storage, and device confirmation.

**Current scope:** USD Sandbox only. The code includes both a local demo ledger and a Dwolla/Plaid Sandbox integration. Physical Bluetooth delivery and external banking acceptance remain unverified. Start with the [verification record](VERIFICATION.md) when assessing what has actually been demonstrated.

## Choose a starting point

| You want to… | Read |
|---|---|
| Understand the product without reading code | [How it works](HOW_IT_WORKS.md) |
| Run it on your Mac or two iPhones | [Development guide](DEVELOPMENT.md) |
| Find a component and understand its responsibilities | [Architecture](ARCHITECTURE.md) |
| Follow a payment and its failure cases | [Payment lifecycle](PAYMENT_LIFECYCLE.md) |
| Integrate a client or inspect signed HTTP traffic | [API reference](API.md) |
| Understand signatures, encryption, and Bluetooth frames | [Protocol specification](PROTOCOL.md) |
| Understand the balances, journals, and bank worker | [Ledger and storage](LEDGER.md) |
| Change the native app or the C interface | [iOS and Rust integration](IOS_RUST.md) |
| Configure Plaid and its bank sign-in return | [Plaid guide](BANKING.md) |
| Enable the provider-backed Sandbox rail | [Bank-provider guide](BANK_PROVIDER.md) |
| Understand the banking-partner choice | [Partner decision](PARTNER_DECISION.md) |
| Inspect a running server or preserve its data | [Operations guide](OPERATIONS.md) |
| Investigate a visible problem | [Troubleshooting](TROUBLESHOOTING.md) |
| Understand trust, privacy, and current limits | [Security boundaries](SECURITY_BOUNDARIES.md) |
| Run physical acceptance tests | [Device testing](DEVICE_TESTING.md) |
| See evidence from previous checks | [Verification record](VERIFICATION.md) |

## Three concepts to keep separate

1. **Radio delivery:** a phone received and validated an encrypted instruction. This does not establish available money.
2. **Ledger acceptance:** the server authenticated the request, checked the instruction and balance, and committed a result. In the provider rail, acceptance may only reserve money and queue a transfer.
3. **Provider processing:** the bank adapter observed the expected transfer and the ledger applied its accounting effect. Later bank returns can still require reversal or review.

“Sent over Bluetooth” and “spendable in the recipient's wallet” are different events. The app deliberately keeps them separate.

## Where the implementation lives

| Source | Responsibility |
|---|---|
| [`mesh-core`](../crates/mesh-core/src/lib.rs) | Identities, exact cents, signatures, encryption |
| [`protocol.rs`](../crates/mesh-core/src/protocol.rs) | Signed profiles/payments and encrypted envelopes |
| [`framing.rs`](../crates/mesh-core/src/framing.rs) | Fragmentation, reassembly, duplicate suppression |
| [`mesh-ffi`](../crates/mesh-ffi/src/lib.rs) | Owned-string C interface for Swift |
| [`mesh-server`](../crates/mesh-server/src/lib.rs) | Signed RPC routing and application errors |
| [`store.rs`](../crates/mesh-server/src/store.rs) | Demo ledger and shared accounting primitives |
| [`provider.rs`](../crates/mesh-server/src/provider.rs) | Provider reservations, event application, balance restrictions |
| [`setup.rs`](../crates/mesh-server/src/setup.rs) | Recoverable customer/bank setup and balance comparison |
| [`mesh-banking`](../crates/mesh-banking/src/lib.rs) | Sandbox transport, transfer outbox, authenticated notifications |
| [`WalletModel.swift`](../ios/MeshPay/WalletModel.swift) | Client journal, synchronization, payment confirmation |
| [`BluetoothMesh.swift`](../ios/MeshPay/BluetoothMesh.swift) | CoreBluetooth central/peripheral adapter |

Generate Rust API documentation with `cargo doc --workspace --no-deps`. The guides describe the current checkout; they do not replace the typed wire structures or database schemas.

## Maintaining these docs

When changing a wire field, update [API](API.md), [protocol](PROTOCOL.md), and the Swift models. When changing financial transitions, update the [lifecycle](PAYMENT_LIFECYCLE.md), [ledger](LEDGER.md), and relevant tests. When changing a script or setting, update [development](DEVELOPMENT.md), [operations](OPERATIONS.md), and [`.env.example`](../.env.example).

Record the scope and date of verification. A local mock, simulator screenshot, physical-radio test, and external provider transfer prove different things. Preserve that distinction when updating [VERIFICATION.md](VERIFICATION.md).
