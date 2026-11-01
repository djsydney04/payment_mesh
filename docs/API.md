# Signed HTTP API

[Documentation index](README.md) · [Wire signatures](PROTOCOL.md#signed-structures)

The public server has three routes. Wallet actions share one signed RPC endpoint. There is no bearer-token login and no unsigned balance-changing route.

| Method and path | Authentication | Purpose |
|---|---|---|
| `GET /health` | None | Process-level Sandbox health response |
| `POST /v1/rpc` | Ed25519-signed request | Register, read a wallet, submit payments, and manage bank actions |
| `POST /v1/webhooks/dwolla` | HMAC over raw request bytes | Persist provider notifications for later reconciliation |

Sources: [`lib.rs`](../crates/mesh-server/src/lib.rs), [`bank_webhooks.rs`](../crates/mesh-server/src/bank_webhooks.rs).

## Request envelope

The following is a shape illustration, **not a signed request you can submit**:

```json
{
  "body": {
    "version": 1,
    "public_key": "<64 lowercase hexadecimal characters>",
    "timestamp": 1792620000,
    "nonce": "<fresh UUID>",
    "action": "snapshot",
    "payload": {}
  },
  "signature": "<Ed25519 signature in hexadecimal>"
}
```

`timestamp` is Unix time in seconds, within 120 seconds of the server clock. Every RPC attempt needs a fresh nonce. The server persists and consumes the nonce before dispatching the action, including action attempts that subsequently fail. Do not resend the identical signed HTTP envelope after a timeout.

Action IDs have a different purpose: retain the same payment, withdrawal, funding, or bank-link intent UUID when retrying that operation. A new RPC nonce avoids request replay; a stable operation ID avoids repeated financial effects.

The signature covers `meshpay/rpc/v1`, a zero byte, and compact serialization of the typed body. Use `SignedRequest::new` or the Rust FFI `request` operation. JSON formatting and another language's map ordering are not a substitute for the typed signing contract.

### Constructing requests in Rust

This example describes the existing API; it does not perform a transfer:

```rust
use mesh_core::{now, random_seed, Identity, Profile, SignedRequest};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let identity = Identity::from_seed(&random_seed())?;
    let profile = Profile::new(&identity, "Alice", now())?;
    let register = SignedRequest::new(
        &identity, "register", serde_json::to_value(profile)?, now(),
    )?;
    // POST serde_json::to_vec(&register)? to /v1/rpc with application/json.
    // Once registered, sign a separate snapshot request with a fresh nonce:
    let snapshot = SignedRequest::new(&identity, "snapshot", json!({}), now())?;
    assert_ne!(register.body.nonce, snapshot.body.nonce);
    Ok(())
}
```

For a runnable network example, use `./scripts/demo.sh` and read [`mesh-cli`](../crates/mesh-cli/src/main.rs). A real client must preserve its seed securely instead of creating a new identity on each run.

## Common response format

```json
{"ok":true,"data":{"id":"<operation UUID>","status":"queued"}}
```

```json
{"ok":false,"error":{"code":"insufficient_funds","message":"Your available balance is too low"}}
```

HTTP success can mean “durably accepted and pending.” Inspect `data.status`; only a payment status of `settled` means its recipient credit has been applied. `processed` is an internal provider state, mapped to `settled` in wallet receipts and snapshots.

The request-body limit is 32,768 bytes. Invalid JSON, missing typed fields, and oversized bodies can be rejected by Axum before the application error formatter runs. Clients must tolerate a non-JSON framework error. Rust-core validation errors are surfaced with the application code `invalid_payment`, including some amount and expiry errors.

## Wallet and payment actions

Except for `register`, the caller must already have a wallet account in the database.

| Action | Payload | Result and restrictions |
|---|---|---|
| `register` | A complete signed `Profile` | `registered`, `mode`; the profile key must equal the signed caller key |
| `snapshot` | `{}` | Current wallet snapshot for the caller only |
| `settle` | A complete signed `Payment` | Current operation receipt; caller must be the payment's sender or recipient |
| `withdraw` | `{"id":"<UUID>","amount_cents":500}` | Reserve/queue $5 to the caller's linked bank, or return the existing result |
| `demo_fund` | `{}` | One $125.40 grant per wallet key in demo mode; repeated calls do not grant more |
| `demo_link_bank` | `{}` | Attach the local `Demo checking` fixture in demo mode |

`settle` takes the signed payment itself, not a mesh envelope or BLE fragment. The recipient can submit the instruction without possessing the sender's private key because the inner payment already carries the sender's signature.

The server checks the stored ledger UUID. Reusing an operation ID for different immutable fields is a conflict. A saved insufficient-funds rejection cannot be revived by funding the wallet and retrying that same payment; the user must explicitly create a new payment.

## Bank setup and funding actions

| Action | Payload | Result and restrictions |
|---|---|---|
| `link_token` | `{}` | `link_token`, `mode`; requires configured Plaid and, on the provider rail, eligible wallet setup |
| `link_bank` | `id`, `public_token`, `account_id` | Provider rail returns intent `id` and current setup `status`; default demo rail performs Plaid linking directly and returns bank display fields |
| `bank_enroll` | `id`, `profile`, `synthetic_test_data: true` | Queue synthetic customer setup on the provider rail; returns `id`, `status` |
| `bank_fund` | `id`, `amount_cents`, `authorize_test_debit: true` | Queue one authorized test-bank debit on the provider rail; returns a receipt, without immediately crediting available money |

All transfer IDs must be non-nil UUIDs. Amounts are integer cents in the range 1–100,000. Clients never supply a Dwolla customer, bank source, balance source, or transfer URL through these actions. The server selects resources bound to the authenticated wallet.

Enrollment `profile` uses **camelCase** because it follows `PersonalCustomer` serialization. The surrounding RPC payload uses the field names shown above. This is a synthetic example, not real personal information:

```json
{
  "id": "11111111-1111-4111-8111-111111111111",
  "synthetic_test_data": true,
  "profile": {
    "firstName": "John",
    "lastName": "Doe",
    "email": "mesh-example@example.com",
    "address1": "99-99 33rd St",
    "city": "Some City",
    "state": "NY",
    "postalCode": "11101",
    "dateOfBirth": "1970-01-01",
    "ssn": "1234"
  }
}
```

Generate a fresh ID for your own attempt. Do not submit real identity data to this Sandbox prototype. Current setup and retention details are in [BANKING.md](BANKING.md) and [BANK_PROVIDER.md](BANK_PROVIDER.md).

A provider-backed `link_bank` response is issued only after encrypted intent persistence. Repeating the same intent and inputs returns its current status. An expired pre-Dwolla sign-in becomes `relink_required`; a fresh user sign-in gets a fresh intent ID. A late retry of the old ID cannot replace the newer attempt.

## Snapshot contract

The common top-level fields are:

| Field | Meaning |
|---|---|
| `ledger_id` | UUID identifying this database's payment authority |
| `public_key`, `name` | Caller wallet identity and current display name |
| `balance_cents`, `currency` | Server materialized available account balance; currency is `USD` |
| `mode`, `bank_rail` | `sandbox`, and either `demo` or `dwolla_sandbox` |
| `plaid_configured` | Client configuration exists; not proof that an external Plaid call succeeded |
| `bank` | Display `label`, `mask`, `mode`, or null |
| `payments` | Up to 200 recent wallet-related payment rows |
| `withdrawals` | Up to 200 relevant recent cash-out rows |

Provider snapshots also contain `banking`, `fundings`, and `reserved_payment_ids`. `banking` includes verification `status`, blocking reason `blocked`, `debt_cents`, and `bank_status`. It starts with `status: not_started` before enrollment. An empty `blocked` string means no local block reason is recorded; it is not independent proof of provider eligibility.

A payment row includes `id`, `sender`, `recipient`, `amount_cents`, `memo`, `settled_at`, `sender_name`, `recipient_name`, and `status`. On provider snapshots, the historical field name `settled_at` holds the request's creation timestamp, including pending rows. Do not use that timestamp as evidence of settlement; read `status`.

Withdrawal/funding rows include `id`, `amount_cents`, `status`, `bank_label`, `bank_mask`, `created_at`; provider rows additionally include `kind`. The arrays are recent history, not complete exports, and there is no pagination API yet. Provider history limits are applied before splitting the recent bank-operation rows into funding and withdrawal arrays.

`reserved_payment_ids` identifies active outgoing payment reservations already represented in `balance_cents`. The iPhone excludes those IDs from an additional local subtraction. It is independent of the visible activity page. See [the ledger guide](LEDGER.md#client-available-balance).

## Error handling and retry policy

| Code or condition | Interpretation | Client response |
|---|---|---|
| `replayed_request` | RPC nonce was already consumed | Sign a fresh request; retain the action's operation ID |
| `insufficient_funds` on a payment | Terminal declined instruction | Show failure; a new payment requires a new explicit user decision |
| `id_conflict` | Operation ID and immutable input disagree | Preserve original intent; do not silently create another financial ID |
| `wrong_ledger` | Instruction belongs to another database | Return to the original ledger; do not rewrite and re-sign automatically |
| `verification_required`, `bank_required` | Required setup is incomplete | Complete setup and refresh the snapshot |
| `wallet_review` | Spending/setup restriction requires attention | Show current state; do not bypass a freeze |
| `too_many_pending` | Provider wallet already has 128 active requests | Wait for existing work to resolve |
| `plaid_relink_required` | User bank sign-in must be restarted | Start a fresh Link flow, not a new money transfer |
| `bank_unavailable`, timeout, unreadable response | Outcome may be uncertain | Keep the saved intent and retry/reconcile with the same action ID |

Application errors generally use 400 for invalid input/setup, 403 for forbidden access, 409 for conflicts/restrictions, 500 for internal availability, and 502 for provider availability. Do not rely on HTTP status alone to decide that funds did or did not move.

## Provider notification route

`POST /v1/webhooks/dwolla` checks `X-Request-Signature-SHA-256` against the exact raw body. A valid new event or identical duplicate returns 200; bad/missing authentication returns 401, a conflicting reused event ID returns 409, and an unconfigured route returns 503. The same global body limit applies.

The route records the event digest and wakes known transfer reconciliation. It never credits money based on a topic string. Use the provider's subscription tools for genuine Sandbox notification tests; the wallet's signed RPC envelope is not webhook authentication.
