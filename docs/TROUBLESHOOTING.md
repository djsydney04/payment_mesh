# Troubleshooting and common questions

[Documentation index](README.md) · [Operations](OPERATIONS.md)

Start with the layer that failed: native discovery, delivery, signed HTTP acceptance, wallet accounting, or provider processing. A successful earlier step does not establish a later one. Preserve the payment/operation ID, current status, rail, and ledger UUID while diagnosing a retry. Avoid copying seeds, bank tokens, or identity data into logs.

## Build and local setup

| Symptom | Likely cause / safe next step |
|---|---|
| `cargo` is missing | Ensure Rust is installed and `~/.cargo/bin` is on your PATH |
| iOS build cannot find SDK/tools | Select full Xcode; install the requested simulator runtime and run Xcode once to complete setup |
| Swift changes are absent from the project | Regenerate with `scripts/generate-xcode.py`; new Swift files need project references |
| New Rust FFI behavior is absent from iOS | Rebuild with `scripts/build-rust-ios.sh`; Xcode links the generated XCFramework |
| Rust iOS target cannot be built | Check both arm64 targets installed by the build script; current packaging does not include Intel simulator binaries |
| Plaid download/checksum fails | Preserve the pinned version/hash; check connectivity and the official release rather than bypassing integrity verification |
| UI test refuses a generic destination | Set `MESH_IOS_DESTINATION` to a specific installed simulator UUID |
| Keychain unavailable or OSStatus error | Use the script's ad hoc simulator signing and the correct entitlements; preserve the existing wallet rather than generating a replacement identity |
| Test wallet cannot reach its server | Start a demo server on 8787 for native server-backed tests; a physical phone needs the Mac's LAN address |
| Address already in use | Identify the existing development server or use another port; do not start two workers for the same provider wallet database |

See [development](DEVELOPMENT.md) for exact commands. A clean build can fix stale generated libraries; deleting the application or its journal is a different action that can lose financial intent.

## Nearby discovery and Bluetooth delivery

**No people nearby:** confirm this is a normal run, Bluetooth is enabled and permitted on both phones, and both apps are foregrounded within range. Both participants should first register online with the same ledger. The simulator does not emulate a successful physical BLE peer network. Preview friends are fixtures.

**Names match but identity codes differ:** names are self-selected. Compare the code displayed by your friend's device before authorizing. The public key, rather than the name or Bluetooth device UUID, identifies the wallet.

**A friend disappears or reconnects slowly:** iOS can suspend background activity; the app removes stale nearby entries. RSSI is a rough signal indicator, not a distance measurement. Bring both apps forward and retry discovery. Do not promise delivery while both apps are suspended.

**A partial transfer never appears:** incomplete or conflicting fragments do not become payments. Reassembly has bounded capacity and a 30-second lifetime. The originating phone preserves its outgoing instruction and can rebroadcast on reconnect. Intermediate relay queues are memory-only and may be lost.

**A third phone cannot settle a relayed packet:** only the intended recipient decrypts the payment. Relays carry ciphertext; they are not authorized to submit a stranger's `settle` request. The sender or recipient must later reach the server.

## Pending payments and unexpected balances

| Symptom | Meaning / next step |
|---|---|
| Received over Bluetooth, still pending | No final ledger result yet; connect either participant to the original server |
| HTTP returned success but provider payment is queued/submitted | Reservation was accepted; recipient credit waits for processing |
| Same request is rejected as replayed | Sign a fresh RPC envelope/nonce while retaining the original operation ID |
| `wrong_ledger` | This instruction was signed for another database; return to that ledger |
| Payment expired | New settlement/submission may be disallowed; an existing saved result remains authoritative |
| `insufficient_funds` remains after adding money | The original payment's decline is terminal; sending again requires a new explicit confirmation |
| Balance is lower than expected | Inspect pending outgoing payments, cash-out reservations, provider holds, debt, and restrictions |
| Incoming pending amount is not spendable | Intent receipt does not establish funds; this is expected |
| Provider reservation appears subtracted twice in a custom client | Exclude `reserved_payment_ids` from extra local payment subtraction; server balance already includes those holds |
| Old history disappears from a snapshot | Recent activity is capped; snapshots are not full accounting exports |

For a lost HTTP response, preserve the saved intent. A new signed request with the same operation UUID retrieves or resumes that operation. Generating a new financial UUID can authorize a second transfer. The app performs this persistence/reconciliation itself; do not delete its pending journal to make the screen look clear.

The signed RPC timestamp allows 120 seconds of clock skew. Profiles and new payment instructions expire after their permitted lifetime. Set device/server time correctly; changing signed fields after signing invalidates the signature. Amount input accepts ordinary dollar strings such as `12.50`, with one or two fractional digits, from $0.01 to $1,000.00. Commas, currency symbols, negatives, spaces, and floating-point rounding are not part of the amount contract.

## Server startup and storage

**Rail mismatch:** the database is pinned to `demo` or `dwolla_sandbox`. Use the original rail or a new database for a separate Sandbox. Do not edit rail metadata or import demo balances into provider funding.

**Credentials will not decrypt after restart:** recover the original `.vault-key` file or configured `MESH_VAULT_KEY`. A new random key cannot decrypt old ciphertext. The whole database is not encrypted by that key, so the process may open while particular credential operations still fail.

**Ledger audit fails:** preserve the database and associated files, stop write activity, and inspect a consistent copy with the [read-only queries](OPERATIONS.md#read-only-inspection). Direct balance/cursor edits can hide evidence while breaking accounting. A tested restore/reconciliation path is required.

**Wallet changed after a reinstall/reset:** a wallet is its signing identity. Server data cannot recreate the old seed. There is no supported recovery or key-rotation flow in this build. Keep existing Keychain and journal data when investigating local startup errors.

## Plaid and provider Sandbox

| Symptom | Check / recovery |
|---|---|
| Demo bank appears instead of Plaid | Check server Plaid settings and the snapshot's `plaid_configured`; `.env` is not auto-loaded |
| Provider server refuses startup | Supply both providers' Sandbox credentials plus registered HTTPS Plaid redirect; use a fresh correctly pinned database |
| Link opens but bank-app return fails | Verify registered redirect, entitlement, association file, team/bundle ID and actual device behavior in [BANKING.md](BANKING.md) |
| Link was open when app was terminated | Restart sign-in using the app's interrupted-session screen; an SDK object cannot survive process termination |
| Bank link is acknowledged but still setting up | Intent is durably saved; the worker advances the remaining provider phases |
| Bank setup says `relink_required` | Complete a fresh sign-in with a fresh bank-link intent; preserve old terminal receipts |
| Test enrollment never becomes verified | Check the synthetic profile and provider customer status; spending waits for verified setup |
| Deposit is queued but balance has not increased | Funding is credited after authoritative processing, not when the debit is queued |
| Wallet is restricted after a return | Inspect debt/review reason; read [ledger return accounting](LEDGER.md) before attempting another action |
| Transfer remains `needs_review` | Preserve provider IDs/observations and reservation; this build has no automatic review-clearing workflow |
| Webhook is rejected | Check raw-body HMAC secret/header, body size, and configured outbox; wallet RPC signatures do not authenticate webhooks |
| Webhooks are absent but transfers advance | Polling is the fallback and is expected to work without webhooks |

A timeout can leave an unknown external outcome. The outbox retains the original operation and polls a known transfer rather than posting it again. Expired submission windows and inconsistent transfer observations can require review. Do not create a new payment to bypass that state.

Fresh bank sign-in is different from retrying money movement: replacing an expired pre-resource Link session is safe through its explicit recovery flow. Replacing an ambiguous funding/payment/cash-out UUID is not the same recovery.

## Questions about the design

**Does money move offline?** Phones can exchange an authorized instruction offline. Spendable balances change only through the server/provider accounting path. The app does not create final offline dollars.

**Why use Bluetooth if a server is required?** Bluetooth discovers and addresses a nearby friend and carries their instruction without requiring internet at that moment. Later synchronization resolves the financial result. See the [worked walkthrough](HOW_IT_WORKS.md).

**Can someone spend the same cached balance twice while offline?** A modified client can sign multiple competing instructions. The server checks actual funds and serializes accounting; it does not trust the cached client balance. Some instructions can fail. The honest app's local reservations reduce accidental overspending but are not the financial authority.

**Does Plaid move the P2P money?** In this app, Plaid links bank accounts and creates the processor connection. Dwolla Sandbox handles the implemented external wallet transfers. Default demo banking is local simulation.

**Can this be pointed at production keys?** No. Environment guards and fixed provider origins restrict this implementation to Sandbox. Production also needs a separately approved product, financial/operational design, and implementation work described in [security boundaries](SECURITY_BOUNDARIES.md).

**Does a passing test prove the product works on real phones and banks?** Each test establishes its own scope. The current [verification record](VERIFICATION.md) distinguishes local Rust, simulator, physical-device, and external-provider evidence.
