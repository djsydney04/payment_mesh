# Rust bank-provider integration

`mesh-banking` implements the Dwolla Sandbox transport, transfer worker, durable outbox and authenticated event ingestion. The iPhone app can use the provider-backed Sandbox ledger with `MESH_RAIL=dwolla_sandbox`. Demo and provider databases are pinned to separate rails; demo balances cannot be redeemed or migrated into provider balances.

## Implemented

- OAuth application tokens with expiry, serialized refresh and credential-safe errors.
- Personal verified-customer creation and status lookup; balance funding-source discovery and balance lookup.
- Plaid processor-token exchange on the server, followed by Dwolla exchange and bank funding-source creation helpers. Each resource creation accepts its own persisted idempotency UUID.
- Exact-cent USD transfer requests with operation UUIDs used as both provider idempotency keys and correlation IDs.
- A SQLite outbox that persists the immutable operation before network effects. Independent workers use timed leases. Stale workers cannot overwrite the newer worker's receipt or status.
- Recovery after a provider accepts a transfer but the response or process is lost. Retries use the same immutable request and idempotency key.
- A 23-hour retry cutoff for unconfirmed submissions. Dwolla documents 24-hour idempotency retention; after the cutoff, an operation with no provider ID enters `needs_review` and is not resubmitted automatically. Once a provider ID is known, it can be polled beyond that window without reposting.
- Transfer observation verifies the provider transfer ID, amount, correlation ID and source/destination funding sources. It follows up to eight linked funding/funded transfers within a bounded observation timeout; a pending bank leg keeps the whole operation pending.
- Continued polling of processed operations to detect late failures. A failure observed after processing becomes a `returned` event. Consumers must apply a balanced return/reversal policy; this module does not silently credit wallet balances.
- HMAC-SHA256 verification of raw webhook bodies, durable duplicate suppression and `POST /v1/webhooks/dwolla`. Notifications wake reconciliation; their topic is never accepted as proof of financial settlement. Events for known related transfer legs also wake the original operation.

All URLs are restricted to the Dwolla Sandbox origin. Redirects are not followed with credentials. Provider response bodies are size-limited, and private provider errors are not copied into user-facing messages. There is no production-origin constructor or production switch.

## Local verification

```sh
cargo test -p mesh-banking
cargo test -p mesh-server --test bank_webhooks
```

The transport tests run a local HTTP server and check actual authorization headers, decimal request amounts, resource locations, token refresh, secure exchange steps and multi-leg responses. Worker tests cover lost responses, process death, concurrent workers, expired leases, overdue unknown submissions, conflicting observations, out-of-order webhook notifications and late failures. These tests do not prove account eligibility or successful calls to Dwolla's external Sandbox.

## External Sandbox acceptance

Create and verify your Dwolla Sandbox account and configure `DWOLLA_KEY` and `DWOLLA_SECRET` locally. Do not paste credentials into chat, commit them, or include them in operation files. `DWOLLA_ENV`, when present, must be `sandbox`.

Check authentication without moving funds:

```sh
cargo run -p mesh-banking --bin mesh-bank -- probe
```

After creating test customers/funding sources in your own Sandbox, prepare an operation JSON file with fresh IDs:

```json
{
  "id": "REPLACE_WITH_FRESH_OPERATION_UUID",
  "ledger_id": "REPLACE_WITH_LEDGER_UUID",
  "wallet": "REPLACE_WITH_64_LOWERCASE_HEX_PUBLIC_KEY",
  "kind": "cash_out",
  "source": "REPLACE_WITH_SANDBOX_BALANCE_FUNDING_SOURCE_UUID",
  "destination": "REPLACE_WITH_SANDBOX_BANK_FUNDING_SOURCE_UUID",
  "amount_cents": 500
}
```

Only use synthetic Sandbox identity/bank data. These placeholders must be replaced; the parser rejects them as written. An operator must verify ownership and the approved flow of funds before using this runner. Its local JSON input is an administrative acceptance interface, not a client-facing wallet API.

```sh
cargo run -p mesh-banking --bin mesh-bank -- enqueue .local/banking.sqlite .local/bank-operation.json
cargo run -p mesh-banking --bin mesh-bank -- work .local/banking.sqlite --once
cargo run -p mesh-banking --bin mesh-bank -- status .local/banking.sqlite OPERATION_UUID
cargo run -p mesh-banking --bin mesh-bank -- events .local/banking.sqlite 0
```

`work` without `--once` runs until interrupted. The provider's Sandbox bank transfers may remain pending until processing is simulated through Dwolla's supported Sandbox controls. Do not replace real provider responses with local success flags.

For webhook testing, start the server with the same `MESH_BANK_OUTBOX` file and a 16–1024-byte `DWOLLA_WEBHOOK_SECRET`. Create a Sandbox webhook subscription with that same secret and a valid public HTTPS URL ending in `/v1/webhooks/dwolla`. The application does not automatically subscribe an external endpoint. Polling remains available while a public URL is not configured.

## Configuration preflight

Create separate [Plaid Sandbox](https://dashboard.plaid.com/) and [Dwolla Sandbox](https://accounts-sandbox.dwolla.com/) applications. Cloudflare can provide the public HTTPS hostname and host the Apple association file; it does not replace either banking provider account. In Plaid, configure Auth, the Dwolla processor integration, and the exact HTTPS redirect URI used by the app. Complete the associated-domain setup in [BANKING.md](BANKING.md). Keep API secrets on the server, never in the iOS build or repository.

Export the settings from `.env.example` into the server process using your local secret manager or environment. The server does **not** read `.env` files automatically. Set `MESH_RAIL=dwolla_sandbox` and an explicit `MESH_DATABASE` path for a fresh wallet database, plus all five Plaid/Dwolla credential and redirect settings.

```sh
cargo run -p mesh-server -- --check-banking
```

This local preflight reports missing variable names, rejects non-Sandbox modes and invalid provider configuration, and validates optional webhook wiring. It makes no external requests and never opens or creates a database or vault key. It cannot determine whether a database already belongs to the demo rail, whether a redirect is registered with Plaid, or whether the phone's Universal Link works. Normal server startup still validates the stored rail.

Once configured, explicitly check external Sandbox access:

```sh
cargo run -p mesh-server -- --probe-banking
```

The probe authenticates with Dwolla and asks Plaid for an Auth Link token using a synthetic readiness user. The token is discarded without being printed. It creates no customers, linked bank accounts, or transfers. Authentication and Link token creation do not establish Dwolla Balance eligibility or prove the processor exchange and payment journey; complete the app flow below. A failed command exits nonzero and leaves the wallet database untouched.

## App Sandbox setup

1. Create Dwolla Balance and Plaid Sandbox accounts. Enable Plaid Auth and its Dwolla processor integration for your Sandbox application. Configure the native Plaid redirect/associated-domain setup in [BANKING.md](BANKING.md).
2. Set `MESH_RAIL=dwolla_sandbox`, `MESH_DATABASE=.local/dwolla-wallet.sqlite`, `DWOLLA_KEY`, `DWOLLA_SECRET`, `PLAID_CLIENT_ID`, `PLAID_SECRET` and `PLAID_REDIRECT_URI` in the server environment. Use a fresh database. The server refuses to reopen an existing database under a different rail.
3. Run the configuration preflight and optional Sandbox probe above, then run `cargo run -p mesh-server`. The bank transfer outbox uses the wallet database, so a reservation and its provider instruction commit atomically. Do not enqueue administrative runner operations into this database.
4. Open the app, connect to this server, and open Bank. Choose **Create my test wallet**. It submits a synthetic test identity rather than collecting real identity data. Customer creation is durable, uses a stable idempotency key, and deletes the encrypted identity payload after recording the provider customer ID. Verification/document exceptions enter review; no spending is allowed until verified.
5. Connect a test checking/savings account with Plaid. The server verifies the selected account, creates a processor token, then durably creates a Dwolla secure exchange and bank funding source. It verifies that the bank belongs to this customer and is verified. The processor token is encrypted while needed and discarded after the exchange is recorded. The provider path retains the Plaid access token encrypted only until the processor token is recorded, then discards it. The client intent is committed before token exchange, and each setup stage resumes after restart. Expired sign-ins offer a fresh Link session before any Dwolla resource exists; see [interruption behavior](BANKING.md#interrupted-bank-setup).
6. Choose **Add money**, enter an amount, and confirm the one-time test bank debit. The app saves the transfer UUID before the request. Available balance stays unchanged while the deposit is pending. Use Dwolla's supported Sandbox processing controls to advance bank transfers.
7. Verify a second test wallet on the same server. Send a nearby payment. The sender's money is reserved immediately, and the recipient is credited only after the provider reports processing. Cash outs use the linked bank source and retain the same ID across retries.
8. Test lost responses, server/app restart, declines, returns and delayed webhook delivery using your own Sandbox account before considering external acceptance complete.

If `DWOLLA_WEBHOOK_SECRET` is set, `/v1/webhooks/dwolla` authenticates notifications into this same wallet database. Omit `MESH_BANK_OUTBOX`, or set it to exactly `MESH_DATABASE`. Without a webhook endpoint, the worker polls. Webhook topics only wake a provider lookup and cannot directly credit balances.

## Accounting and recovery

The ledger consumes the durable outbox status log transactionally with its accounting entries and cursor. Repeated events cannot debit or credit twice. Funding credits come from a provider equity account, payments move sender → hold → recipient, and cash outs move sender → hold → provider clearing. Failed/cancelled operations release reservations once. New incoming credits repay recorded ACH-return debt before becoming spendable.

A deposit returned after processing removes the unspent amount. A spent shortfall is recorded as debt in a receivable account, and spending is frozen until restored. Ambiguous outcomes and returned peer/cash-out transfers retain reservations or balances under review until the actual provider disposition is reconciled; they do not generate an automatic refund based solely on a return notification. There is no operator refund/review-resolution UI yet. Resolve these cases with provider evidence and a reviewed balanced recovery procedure, never by editing a wallet balance.

Current provider balances are compared periodically when a wallet has no active transfer. A discrepancy freezes spending and never creates a compensating credit. Negative provider balances are parsed exactly. Each new provider submission rechecks local restrictions and bank verification; known transfers continue to be observed while frozen.

Offline payment expiry is also a submission deadline. An expired instruction that was never submitted is cancelled and its reservation released. An unknown prior submission at expiry enters review. A known provider transfer continues to be observed after the instruction expires, without being resubmitted.

## Remaining external acceptance and production work

Local mock-provider and ledger tests pass, but no external Dwolla/Plaid credentials are configured here. Actual provider customer creation, Link handoff, exchange eligibility, bank funding, P2P transfers, cash outs and return scenarios must still be verified against the external Sandbox. Physical iPhone Bluetooth testing is also outstanding.

This is a Sandbox integration. Production requires an approved P2P/custody operating model and contracts, real identity and consent flows, return reserves, fraud/AML operations, operational recovery tools, enrollment throttling, managed secrets, account recovery, distribution and independent security review. Production API origins remain disabled.

An unresolved operation requires provider-side reconciliation; creating another UUID is not a safe recovery procedure. Terminate any stale workers before manual recovery. The lease and retry cutoff do not extend Dwolla's contractual idempotency guarantee or establish exactly-once execution under arbitrary process suspension. Production needs the provider-approved operating model and acceptance tests.

## Primary references

- [Dwolla idempotency retention](https://developers.dwolla.com/docs/api-reference/api-fundamentals/idempotency-key)
- [Transfer retrieval](https://developers.dwolla.com/docs/api-reference/transfers/retrieve-a-transfer)
- [Correlation IDs and linked transfer legs](https://www.dwolla.com/updates/correlationids-streamline-transfer-webhoook-processing/)
- [Plaid secure exchange](https://developers.dwolla.com/docs/secure-exchange/plaid)
- [Webhook authentication and delivery](https://developers.dwolla.com/docs/working-with-webhooks)
- [Sandbox behavior](https://developers.dwolla.com/docs/testing)
