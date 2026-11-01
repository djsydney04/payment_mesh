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

Set `com.apple.developer.associated-domains` in `ios/MeshPay/MeshPay.entitlements` to `applinks:YOUR-DOMAIN`. Serve `https://YOUR-DOMAIN/.well-known/apple-app-site-association` over HTTPS with no redirect:

```json
{
  "applinks": {
    "details": [{
      "appIDs": ["YOUR_TEAM_ID.app.mesh.sandbox"],
      "components": [{"/": "/plaid/*"}]
    }]
  }
}
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
