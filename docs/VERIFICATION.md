# Verification record

Local validation on October 21, 2026, with Rust 1.99.1 and Xcode 26.6 on Apple silicon.

| Check | Result |
|---|---|
| Rust format and Clippy with warnings denied | Passed |
| Rust tests | 85 passed: 13 protocol, 1 C ABI, 12 demo ledger/API, 30 bank transport/outbox, 13 provider accounting/API, 8 recoverable setup, 6 Plaid HTTP/configuration, 2 webhook HTTP |
| Rust dependency advisory check | Passed, no reported advisories |
| Native arm64 simulator build | Passed |
| Native arm64 iPhone Rust static library | Built |
| Native UI navigation and amount keypad | Passed |
| Provider setup, bank actions and avoiding duplicate local reservations | Passed in native UI fixtures |
| Wallet persistence across termination/relaunch | Passed |
| Native app → signed Rust RPC → test funding → demo bank link → relaunch | Passed against the actual local Rust server |
| Two-wallet encrypted-fragment/HTTP/cash-out command-line demo | Passed |
| Production-mode startup guard | Refused startup as expected |
| Bank provider HTTP transport against local scripted API | Passed, including OAuth, exact cents, idempotency, Plaid secure exchange and linked bank-transfer legs |
| Bank outbox crash and concurrency tests | Passed, including process death after provider acceptance and expired-worker fencing |
| Authenticated webhook HTTP endpoint | Passed; duplicate events persisted once and tampered/missing signatures rejected |
| Dwolla external Sandbox authentication probe | Could not run: DWOLLA_KEY and DWOLLA_SECRET are not configured |

Simulator: iPhone 17 Pro, iOS 26.5. The screenshot fixtures are from the explicit `--preview` mode; they do not represent nearby real people or real funds. Native network validation used a separate normal wallet and local sandbox ledger.

Physical Bluetooth delivery, multi-hop radio performance, device authentication on a real iPhone, Plaid Sandbox with account credentials, and bank OAuth app-switch handling have not been validated. Live money movement is not implemented or enabled. See [device tests](DEVICE_TESTING.md) and [banking setup](BANKING.md).

The app's provider-backed Sandbox accounting is connected and locally tested. Tests cover atomic reserve/outbox commits, provider-event/ledger rollback, confirmed availability, terminal declines, concurrent reservations, expiry, deposit return debt and recovery, conservative returned cash-out freezes, recoverable enrollment, sensitive-payload removal and verified bank membership. Seven native UI tests passed, including the provider setup/bank screens and a $25 pending payment whose server reservation is not subtracted twice. Provider screenshots are explicit preview fixtures. Additional tests cover durable public-token intake, restart after token exchange, expired Link replacement, stale-worker fencing and immutable retry receipts. Interrupted/expired sign-in screens pass native UI checks. After rechecking the three affected preview tests, the simulator wallet file had the exact same SHA-256 before and after; sample screens did not overwrite saved wallet data. Actual external Dwolla/Plaid acceptance still requires configured Sandbox accounts. See [provider status](BANK_PROVIDER.md).

Run `scripts/check.sh`, `scripts/demo.sh`, and `scripts/ios.sh test` to reproduce. Start the local Rust server on port 8787 for the optional native API integration test; it is skipped when the server is unavailable. The bank UI/network test assumes a new or unchanged demo wallet balance of $125.40.

## Documentation validation — October 22, 2026

Documentation and source comments were expanded without changing runtime logic. This pass checked:

- Rust formatting and workspace API documentation with Rustdoc warnings denied.
- Both executable `mesh-core` Rustdoc examples, including encryption, out-of-order reassembly, and recipient verification.
- The API guide's Rust request-construction example, compiled and run in an isolated Cargo project using the local core crate.
- Local Markdown link targets/heading anchors, fenced JSON examples, and whitespace.
- The public C header's syntax and the documented read-only SQL queries against the repository schemas.

The native application, physical radios, and external provider accounts were not retested for this documentation-only pass. The implementation evidence above retains its original date and scope. Start reading at the [documentation index](README.md).

## Integration setup validation — November 2, 2026

- `scripts/check.sh` passed Rust formatting, Clippy, and workspace tests, including the new provider-name and configuration-readiness checks.
- All four `scripts/test-configure-banking.py` tests passed. They exercise exact OAuth redirect/AASA matching, invalid input rejection, preservation of existing configuration, and device versus simulator build arguments. This command now runs in `scripts/check.sh` and its existing CI job.
- `scripts/ios.sh` built the simulator app successfully.
- `scripts/demo.sh` passed the local two-wallet demo.
- Xcode build-settings inspection confirmed that local team, bundle identifier, and entitlement overrides resolve correctly, including an entitlement path containing spaces.
- Cloudflare OAuth hosting passed OpenTofu backend-disabled initialization, configuration validation, and formatting checks; Worker request tests passed using Node's standard Request/Response APIs. No deployment plan was applied.

Provider responses in automated tests remain local fixtures. No external Plaid/Dwolla credentials, physical-device OAuth acceptance, Apple provisioning, or deployment were exercised in this validation pass.
