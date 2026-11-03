# Plaid bank integration

The Rust server now includes the Plaid-to-Dwolla processor-token helper, and `mesh-banking` provides the Sandbox API/outbox/webhook layer. See [provider integration status and acceptance steps](BANK_PROVIDER.md). The app's provider-backed Sandbox rail includes recoverable customer setup, bank linking, deposits, nearby payments and cash outs.

Plaid Link is used for bank account connection. In default demo mode, cash outs are simulated locally. In `dwolla_sandbox` mode, deposits and cash outs use Dwolla Sandbox and await authoritative processing. Neither mode moves live money.

Plaid's current [Transfer documentation](https://plaid.com/docs/transfer/creating-transfers/) excludes peer-to-peer payments and same-person account transfers and points those use cases to Auth. Consequently Mesh uses [Link/Auth](https://plaid.com/docs/auth/add-to-app/) and does not call `/transfer/authorization/create` or `/transfer/create` for the wallet flow.

## Configure Sandbox

1. Obtain Plaid Sandbox API keys in your own Plaid Dashboard. Do not put them in the iOS app or commit them.
2. Register an HTTPS redirect URI with Plaid, for example a `/plaid/` path on a domain you control.
3. Configure that domain as a Universal Link in the app's Associated Domains entitlement and serve the Apple association file described below. This is required to complete OAuth bank flows.
4. Start the Rust server with `PLAID_CLIENT_ID`, `PLAID_SECRET`, `PLAID_REDIRECT_URI`, `PLAID_ENV=sandbox`, and `MESH_MODE=sandbox` set in its environment. A `.env` file is not read automatically. Avoid pasting secrets into shell history.
5. Connect the app to that server, open Cash out, and select Connect with Plaid. Use Plaid's current Sandbox test credentials for a supported test institution. Choose the checking/savings account to link.
6. In default demo mode, cash outs use the local simulated rail. To test provider transfers, configure `MESH_RAIL=dwolla_sandbox` and the Dwolla Sandbox settings with a fresh database, then create a test wallet in Bank before linking Plaid. Follow [the provider acceptance steps](BANK_PROVIDER.md).

Pass a fresh intent `id` UUID, the `public_token`, and the selected account ID back to the server. The app saves that intent before sending it. The server exchanges the token, fetches eligible account data, and, in demo mode, saves an encrypted credential object associated with the authenticated wallet. In provider mode it first queues the encrypted public-token intent and acknowledges it. A worker exchanges the public token and commits encrypted Item credentials before fetching accounts or creating a Sandbox processor token. The access token is discarded after the processor token is saved; the processor token is discarded after the Dwolla exchange is recorded. Each intermediate step survives a server restart. Only the display label/mask are returned to the phone. No bank credentials appear in BLE packets. The token exchange is not a substitute for verifying bank ownership against a verified customer identity.

## OAuth return configuration

The planned Terraform-managed Cloudflare Worker uses `https://payment-mesh.dylanmitic.workers.dev/plaid/`. This URL is prospective until the infrastructure is applied and its HTTPS/AASA responses are verified; no custom domain is needed. Generate a local configuration using that return URL and your Apple signing team:

```sh
python3 scripts/configure-banking.py \
  --redirect-uri https://payment-mesh.dylanmitic.workers.dev/plaid/ \
  --team-id YOUR_TEAM_ID
```

Replace the team placeholder; the team ID must be ten uppercase letters/digits. The default bundle ID is `app.mesh.sandbox`; use `--bundle-id` if your registered app uses another identifier. Use a plain URL path without query, fragment, wildcards or a custom port. The generated AASA matches that **exact path**, including its trailing slash, and the app identifier `TEAM_ID.BUNDLE_ID`. For an older Apple account whose application identifier prefix differs from its team ID, this helper is not sufficient: configure and verify the identifier with your signing administrator.

The command writes four files to ignored `.local/banking/`: an AASA document, `MeshPay.entitlements`, `ios.xcconfig`, and non-secret `configuration.json`. It never changes checked-in entitlements, uploads anything, or writes credentials. Existing conflicting files are preserved; choose another `--output` directory when changing configuration. Repeat the command with `--check` to verify its existing files without writing.

1. Publish the generated `apple-app-site-association` through the Terraform-managed Worker at `https://payment-mesh.dylanmitic.workers.dev/.well-known/apple-app-site-association`, with HTTPS, JSON content type and **no redirect**. The host need not be your bank API server. Supply the absolute path to `.local/banking/apple-app-site-association` to the Terraform AASA file input. Publish the AASA file only.
2. Register the **same complete redirect URI** in the Plaid Dashboard and set server `PLAID_REDIRECT_URI` to that value.
3. Enable Associated Domains for the app in your Apple developer account. Set up its signing certificate and provisioning profile in Xcode. The build command does not create or update Apple account resources automatically.
4. Build for your connected device with the local signing configuration:

```sh
MESH_IOS_DESTINATION='platform=iOS,id=YOUR-DEVICE-ID' ./scripts/ios.sh
```

Device builds use `.local/banking/ios.xcconfig` by default; set `MESH_IOS_CONFIG` to an absolute config path when using another output directory. Simulator builds retain their existing ad-hoc signing defaults. To build in Xcode's UI, apply the generated configuration as a local build configuration or pass its settings to your build; simply opening the project does not load it. Never commit a local signing configuration or credentials.

Check the generator and build argument contracts without provider calls:

```sh
python3 scripts/test-configure-banking.py
```

The app retains one SDK session while the Link sheet is open. Normal bank-app returns are handled by LinkKit; the iOS SDK does not require manual reinitialization. LinkKit 7 cannot restore an in-progress session after the app process terminates. The app records that sign-in was interrupted, opens Bank on relaunch, and offers a fresh session rather than trying to restore an old SDK object. See [Plaid's OAuth guide](https://plaid.com/docs/link/oauth/) and [LinkKit 7 termination behavior](https://github.com/plaid/plaid-link-ios/blob/master/v7-migration-guide.md#resume-after-termination).

The interrupted/expired-session screens are validated in the simulator. Actual bank app-switch return still requires a configured associated domain and physical-device Sandbox testing. The app does not claim that a simulator preview proves OAuth handoff.

The SDK is pinned to 7.2.2 and downloaded from Plaid's official release. `scripts/fetch-plaid.sh` checks the exact SHA-256 from the official [Swift package manifest](https://github.com/plaid/plaid-link-ios-spm). The dynamic framework is linked and embedded in the Xcode project. No banking client secret is packaged with it.

## Interrupted bank setup

A provider-backed link is acknowledged only after its encrypted intent is committed. Lost HTTP responses can be retried with the original UUID without exchanging the one-time public token again. Once the public token has been exchanged and its result committed, a restart resumes account verification or processor setup from the stored credentials.

There remains an unavoidable gap between Plaid consuming a one-time public token and the server committing its response. If that response is lost and Plaid rejects the token on retry, setup becomes `relink_required`. No Dwolla bank or funds transfer is created by that attempt, and the user can sign in again. An orphaned Plaid Item may need cleanup in the provider dashboard. The old intent receipt remains terminal, so a delayed retry cannot replace a newer sign-in.

The phone removes its temporary public token after the server acknowledges the intent. Access/processor tokens never reach the phone, and bank setup never travels over Bluetooth. Preview screens cannot write wallet data or call the payment server.

## Live money requirements

The next integration decision is the licensed processor/banking partner for custody, account funding and P2P wallet payouts. Plaid Auth supports a bring-your-own-processor approach. Do not enable Plaid Transfer just because its Sandbox endpoints can be called.

The live adapter must include customer/ownership verification, ACH authorization records where needed, provider idempotency, signed webhook verification, event reconciliation, availability policies, return handling, fraud controls and ongoing operations. Product eligibility and legal arrangements must be verified with the chosen provider. The code rejects production mode instead of presenting a simulated transfer as a real bank transaction.
