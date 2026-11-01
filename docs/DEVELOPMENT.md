# Development guide

[Documentation index](README.md) · [Configuration reference](OPERATIONS.md#configuration)

This guide runs the current Sandbox. Start with the demo rail: it needs no bank accounts, provider keys, Apple team, or physical phones. Move to the native app and provider Sandbox separately so failures have a clear source.

## Prerequisites

| Task | Requirements |
|---|---|
| Rust tests, HTTP server, command-line demo | Rust stable with Cargo, rustfmt, Clippy; macOS or Linux; a native C toolchain for bundled SQLite |
| Native iPhone app | Apple silicon Mac, full Xcode and an installed iOS simulator runtime, Python 3, network access for the pinned Plaid SDK and Rust targets |
| Physical Bluetooth test | Two compatible iPhones, iOS 17+, developer signing, device passcodes, Bluetooth permission, one reachable server |
| Provider-backed Sandbox | Plaid and Dwolla Sandbox accounts/keys, configured HTTPS Plaid return URI and associated domain |

The checked-in toolchain selects `stable`, not a frozen compiler version. `Cargo.lock` pins the Rust dependency resolution. The app builds arm64 device and Apple silicon simulator libraries; an Intel simulator slice is not supplied. Select full Xcode as your active developer directory if command-line tools alone are selected.

Run commands below from the repository root. The server reads its process environment directly; copying `.env.example` to `.env` does **not** load it. Supply settings through your local development environment, without committing keys.

## 1. Run the complete command-line demo

```sh
export PATH="$HOME/.cargo/bin:$PATH"
MESH_RAIL=demo ./scripts/demo.sh
```

The script builds the server/client, creates a temporary database, starts a server on `127.0.0.1:18787`, and runs two generated wallets against it. It signs and encrypts a payment, reassembles out-of-order fragments, submits the payment, checks a duplicate retry, and queues a demo cash out. It stops its own server on exit. It does not use the app's normal database on port 8787. Temporary output, including `server.log`, stays in the generated `mesh-demo.*` directory for inspection.

If port 18787 is occupied, use `MESH_DEMO_PORT=18788 MESH_RAIL=demo ./scripts/demo.sh`. Setting the rail explicitly matters because the script inherits other environment settings. This demo exercises protocol and HTTP code; it does not transmit through a Bluetooth radio.

## 2. Start a persistent development server

```sh
MESH_RAIL=demo MESH_DATABASE=.local/dev.sqlite cargo run -p mesh-server
```

In a second terminal:

```sh
curl --fail http://127.0.0.1:8787/health
```

Expected response:

```json
{"status":"ok","mode":"sandbox","version":1,"real_money_enabled":false}
```

This confirms the HTTP process is responding. It does not verify provider credentials, Bluetooth, or a bank transfer. Preserve both `.local/dev.sqlite` and `.local/dev.sqlite.vault-key`. The database records its ledger UUID and payment rail on first use. See [operations](OPERATIONS.md) before resetting or copying it.

## 3. Build and run the native app

```sh
./scripts/ios.sh
open ios/MeshPay.xcodeproj
```

The build script performs these steps:

1. Download and checksum the pinned official Plaid LinkKit 7.2.2 framework.
2. Install Rust's `aarch64-apple-ios` and `aarch64-apple-ios-sim` targets and build the release FFI library for both.
3. Package `MeshCore.xcframework` and regenerate the Xcode project from its source list.
4. Build the MeshPay scheme for a generic iOS simulator using ad hoc signing, which enables the app's Keychain use.

Select an installed simulator in Xcode and run MeshPay. Enter a name and use `http://localhost:8787` as the development server. In demo mode, add the $125.40 test grant. Nearby people appear only when discovered; there are no sample people in a normal run.

The script regenerates `ios/MeshPay.xcodeproj`. Keep lasting source/build-setting changes in [`generate-xcode.py`](../scripts/generate-xcode.py), entitlements, or source files as appropriate; manual project changes may be replaced by the next generation. Physical signing requires your Apple team in Xcode after generation. Provider secrets belong on the server, never in Xcode build settings or the app bundle.

## Screen previews

Add launch arguments under **Scheme → Run → Arguments** in Xcode:

| Arguments | Fixture |
|---|---|
| `--preview` | Nearby friends, demo balance, and history |
| `--preview --preview-bank` | Provider wallet before test enrollment |
| `--preview --preview-bank --preview-bank-linked` | Linked provider wallet and pending reservation |
| `--preview --preview-bank --preview-link-interrupted` | Sign-in interrupted by app termination |
| `--preview --preview-bank --preview-link-expired` | Expired sign-in with a restart path |

Specialized flags require `--preview`. Preview mode initializes the storage adapter, but keeps fixture changes in memory, disables ledger RPC and Bluetooth startup, and does not persist sample history. Remove preview arguments to use the real Sandbox wallet. Preview screenshots validate presentation only.

## 4. Test two iPhones

Run the server on your trusted development network:

```sh
MESH_RAIL=demo MESH_BIND=0.0.0.0:8787 MESH_DATABASE=.local/phones.sqlite cargo run -p mesh-server
```

Install a development build on both phones. Set each app's server to your Mac's reachable LAN address, such as `http://192.168.1.10:8787`; `localhost` on a phone refers to that phone. Allow local-network and Bluetooth access. Register both wallets online with this same database, and fund one wallet.

Compare identity codes, send a small payment with both apps foregrounded, then repeat with Wi-Fi/cellular off and Bluetooth on. The second payment must stay pending until either participant can reach the server. Follow the complete [physical acceptance matrix](DEVICE_TESTING.md), including relay, reconnect, passcode, and OAuth cases. The simulator cannot establish radio reliability.

Development HTTP is allowed only by the app's Debug address policy. Release server addresses require HTTPS. Binding to all interfaces makes the Sandbox reachable on your network; this repository does not supply a public production deployment.

## 5. Enable provider Sandbox separately

Keep your demo database. Choose a **new** `MESH_DATABASE`, set `MESH_RAIL=dwolla_sandbox`, and supply the Plaid/Dwolla settings in [operations](OPERATIONS.md#configuration). Configure the native Universal Link as described in [bank linking](BANKING.md).

Then use Bank to create a synthetic test wallet, link a test account, add money, and observe processing before making it spendable. Work through [provider acceptance](BANK_PROVIDER.md). A local HTTP success or passing mock test does not establish external provider acceptance. No setting enables production endpoints.

## Verification commands

```sh
./scripts/check.sh
MESH_RAIL=demo ./scripts/demo.sh
cargo test --workspace --doc --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked
```

For native UI tests, start a demo server on port 8787, select a simulator UUID from `xcrun simctl list devices available`, then run:

```sh
MESH_IOS_DESTINATION='platform=iOS Simulator,id=YOUR-SIMULATOR-UUID' ./scripts/ios.sh test
```

Use a dedicated test simulator: some UI tests exercise and persist a normal Sandbox wallet, while preview tests use fixtures. The native server-backed test expects the local demo server. A generic simulator destination can build but cannot run UI tests.

| Verification | What it establishes |
|---|---|
| `check.sh` | Formatting, Clippy, Rust unit/integration tests with locked dependencies |
| `demo.sh` | Actual protocol and signed HTTP flow against an isolated local demo server |
| Rustdoc commands | Public documentation builds and executable Rust examples compile/run |
| Native UI tests | Simulator navigation, fixture states, local persistence and demo API flow |
| Physical acceptance | Actual radio delivery, device authentication and platform lifecycle behavior |
| Provider acceptance | Your configured Sandbox account's external setup/transfer behavior |

CI runs Rust checks and the local demo on Linux, and an iOS build on macOS. It does not currently run native UI or external-provider acceptance. Historical results and their scope are in [VERIFICATION.md](VERIFICATION.md).

## Typical change loops

For protocol or accounting changes, run the affected Rust tests first, then `check.sh` and the local demo when the behavior crosses components. Update [protocol](PROTOCOL.md), [API](API.md), or [ledger](LEDGER.md) contracts alongside wire/accounting changes.

For Rust FFI changes, rebuild the XCFramework before testing Swift; otherwise Xcode may still link an old Rust binary. For a Swift-only change, build/test the existing project directly in Xcode; the full script is useful when libraries or project references changed. New Swift source files require project regeneration. Keep financial decisions in Rust and follow the ownership rules in [iOS and Rust integration](IOS_RUST.md).
