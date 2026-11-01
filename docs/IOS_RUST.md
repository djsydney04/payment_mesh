# iPhone app and Rust boundary

[Documentation index](README.md) · [Development setup](DEVELOPMENT.md)

Swift owns Apple platform access and presentation. Rust owns payment construction/validation, cryptography, wire models, packet framing/reassembly, relay decisions, and server accounting. The client does not implement a second signing or amount-validation algorithm in Swift.

## File map

| File | Role |
|---|---|
| [`MeshPayApp.swift`](../ios/MeshPay/MeshPayApp.swift) | Startup, onboarding/root screens, foreground synchronization |
| [`HomeView.swift`](../ios/MeshPay/HomeView.swift) | Balance, nearby selection, activity, bank/settings sheets |
| [`SendView.swift`](../ios/MeshPay/SendView.swift) | Recipient identity comparison, keypad, confirmation/result presentation |
| [`BankView.swift`](../ios/MeshPay/BankView.swift) | Synthetic setup, Plaid presentation, bank actions, interruption messages |
| [`WalletModel.swift`](../ios/MeshPay/WalletModel.swift) | Durable intent journal, authentication, available balance, signed HTTP synchronization |
| [`RustCore.swift`](../ios/MeshPay/RustCore.swift) | Rust handle wrapper, Keychain identity, protected wallet file |
| [`BluetoothMesh.swift`](../ios/MeshPay/BluetoothMesh.swift) | CoreBluetooth discovery, characteristics, queues, backpressure |
| [`Models.swift`](../ios/MeshPay/Models.swift) | Codable wire/cache/view models and snake-case conversion |
| [`Theme.swift`](../ios/MeshPay/Theme.swift) | Shared colors, avatars, button styles, visual components |

`WalletModel`, `RustCore`, and `BluetoothMesh` are confined to the main actor. CoreBluetooth managers use the main queue. Treat this as the handle's exclusive-access contract, not evidence that the underlying C ABI is thread-safe.

## The five C functions

The header is [`MeshCore.h`](../ios/MeshPay/MeshCore.h); implementation is [`mesh-ffi`](../crates/mesh-ffi/src/lib.rs).

| Function | Input/output contract |
|---|---|
| `mesh_seed_new()` | Returns an owned NUL-terminated hexadecimal seed string |
| `mesh_engine_new(seed)` | Borrows a readable UTF-8 seed string for the call; returns a new engine or null |
| `mesh_engine_call(engine, request)` | Borrows exclusive access to a live engine and a NUL-terminated JSON request; returns an owned JSON string |
| `mesh_engine_free(engine)` | Releases one owned handle exactly once; null is allowed |
| `mesh_string_free(value)` | Releases a string allocated by `mesh_seed_new` or `mesh_engine_call` exactly once; null is allowed |

Never free returned strings with Swift/C's ordinary allocator. Copy their UTF-8 content before calling `mesh_string_free`. Never reuse a freed engine/string or call `mesh_engine_free` while a call is in progress. Pointer validity is the caller's responsibility; catching Rust panics does not make invalid C pointers safe.

`mesh_engine_call` checks the decoded text length against 256 KiB. This is separate from the server's 32 KiB HTTP limit. The request must already be a valid readable C string for the length check to occur.

FFI results use `{"ok":true,"data":...}` or `{"ok":false,"error":"text"}`. That error string is a different shape from the HTTP API's nested `error.code`/`error.message` object.

## JSON operations

All operations include `op`. Complex arguments use the complete typed Rust structures.

| `op` | Additional fields | Successful `data` |
|---|---|---|
| `parse_amount` | `amount` as decimal dollar string | `amount_cents` |
| `profile` | `name` | `profile`, `fingerprint`, `public_key` |
| `verify_profile` | `profile` | Validated profile and fingerprint |
| `request` | `action`, optional `payload` | Signed HTTP request with fresh nonce/time |
| `create_payment` | `ledger_id`, `recipient` profile, `amount`, optional `memo`, `balance_cents`, `pending` payments | Signed `payment` and encrypted `packet` |
| `spendable` | `balance_cents`, `pending` payments | `amount_cents` remaining under the local calculation |
| `fragment` | `packet`, optional `mtu` (default 180) | `frames` as Base64 strings |
| `receive_frame` | `source` radio identifier, Base64 `frame` | Empty object until completion; then `packet`, optional decrypted `payment`, optional `forward` packet |
| `validate_payment` | `payment` | `valid: true` after signature/structure/expiry checks |

For example, `{"op":"parse_amount","amount":"12.50"}` returns `data.amount_cents: 1250`. An amount of `12.501` is rejected rather than rounded.

`spendable` is a local calculation over the caller's journal, not a network balance proof or a replacement for payment verification. Swift supplies already-validated journal entries. The authoritative ledger checks funds again.

## Storage and synchronization

The seed is kept in Keychain under service `app.mesh.wallet`, account `identity-v1`, with `AfterFirstUnlockThisDeviceOnly` accessibility. It is not saved in `wallet-v1.json`. There is no recovery or export UI.

The wallet file lives under the app's Application Support `Mesh` directory. It includes the cached server snapshot, payment journal, saved funding/cash-out IDs, enrollment ID, and temporary bank-link intent/interruption flag. Writes are atomic, use complete-until-first-authentication file protection, and exclude the file from backup. This is OS-protected JSON, not an application-encrypted history database. Losing either identity or journal can affect recovery; preserve both when diagnosing problems.

A sync pass registers a fresh signed profile, refreshes the snapshot, reconciles known local payment states, retries saved bank linking/payments/funding/cash-out work, and obtains a final snapshot. New RPC attempts get new nonces; saved action IDs stay stable. The app also attempts sync on network changes, foregrounding, and a nominal 15-second timer. Timers are not background-delivery guarantees.

Device-owner authentication happens before a new outgoing payment or bank debit/cash out is persisted. The model rechecks relevant local state after the prompt because another operation may have changed it while authentication was displayed.

## Bluetooth adapter behavior

The phone runs central and peripheral roles. It reads signed profiles and writes/notifies packet fragments using the UUIDs in [PROTOCOL.md](PROTOCOL.md#ble-service). Each peer's actual write/notification limit sets fragmentation size. Central writes advance on write completion; notifications pause when CoreBluetooth reports backpressure and resume when ready.

Connection and queue limits bound memory use. Nearby entries are pruned after they have not been seen for 65 seconds; relay packets are pruned by expiry. Radio addresses identify reassembly sources, while wallet public keys identify signed people and payments. Do not treat a peripheral UUID or RSSI as a person's authenticated wallet identity or precise distance.

## Plaid sessions and previews

The app retains one Plaid session for the open sheet and prevents duplicate initialization. Normal bank return behavior belongs to the pinned SDK. After process termination, it offers a fresh Link session; it does not serialize or restore a SDK session object. A completed bank-selection intent is saved before HTTP handoff. Its temporary public token is removed after acknowledgement; server access/processor tokens never enter Swift models.

Preview mode exits before Bluetooth/network monitoring starts, blocks RPC, and keeps model commits in memory. It can still initialize/read the Keychain and wallet storage adapter. Preview combinations used by UI tests include:

```text
--preview
--preview --preview-bank
--preview --preview-bank --preview-bank-linked
--preview --preview-bank --preview-link-interrupted
--preview --preview-bank --preview-link-expired
```

Every specialized fixture also needs `--preview`. These flags are screen fixtures, not a substitute for actual provider or radio acceptance tests.

## Changing the boundary

Keep snake-case Rust fields and Swift Codable conversion in agreement; acronym spelling matters (`serverUrl`, `accountId`, `ledgerId`). Optional cache fields let older saved wallets decode when a field is newly introduced. Required wire-field changes still require coordinated client/server updates.

After editing Rust, rebuild `MeshCore.xcframework`; a normal Xcode build alone does not rebuild the static Rust library. After adding Swift files, rerun the deterministic Xcode project generator. See [development change loops](DEVELOPMENT.md#typical-change-loops).
