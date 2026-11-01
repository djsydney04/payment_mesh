# Ledger, persistence, and reconciliation

[Documentation index](README.md) · [Payment lifecycle](PAYMENT_LIFECYCLE.md)

The authoritative balances live in SQLite. The phone's displayed amount is a cached server balance adjusted for local uncertainty. The database's internal accounting is a mirror of the selected Sandbox flow, not independently verified proof of bank reserves.

## Accounting invariants

`store::post` requires at least two entries whose signed cent amounts sum to zero. It inserts a unique journal ID, adjusts materialized account balances, and inserts the matching entry rows within its caller's transaction. A failed leg rolls back the whole transaction.

For this implementation, a positive entry increases an account's `balance_cents`; a negative entry decreases it. User accounts cannot be negative. System accounts may be negative to represent the other side of funding, clearing, holds, and receivables. Do not apply conventional debit/credit labels without specifying this sign convention.

`Store::audit` checks:

- All materialized account balances sum to zero.
- Every recorded journal's entries sum to zero.
- Every account's materialized balance matches the sum of its entries.

The server runs that audit at startup. Passing it proves those internal checks, not provider balance agreement, full journal authenticity, or regulatory/accounting approval.

## Two pinned rails

| Rail | Funding | Payment completion | Cash out |
|---|---|---|---|
| `demo` | One local faucet grant of 12,540 cents per wallet key | One local ledger transaction | Local worker moves a reserve into demo clearing |
| `dwolla_sandbox` | Owned linked test bank → provider balance | Matching provider transfer observation, then ledger credit | Provider balance → linked test bank, with reserved funds |

`metadata.bank_rail` pins the database to its rail; `metadata.ledger_id` pins signed payments to this authority. Opening a database under a different rail is rejected. An existing used unpinned database also cannot be adopted as a new provider ledger. A new database is a new ledger, not an empty replica of the same bank program.

## Worked journal entries

Amounts below are cents and illustrate independent operations.

| Event | Entries |
|---|---|
| Demo funding of $125.40 | Wallet `+12540`; `system:demo_equity -12540` |
| Demo $5 payment | Alice `-500`; Bob `+500` |
| Demo $5 cash-out reservation | Alice `-500`; `system:payout_reserve +500` |
| Demo payout processing | `system:payout_reserve -500`; `system:bank_clearing +500` |
| Provider $100 deposit processed | `system:provider_equity -10000`; Alice `+10000` |
| Provider $5 payment reserved | Alice `-500`; `system:provider_hold +500` |
| Provider payment processed | `system:provider_hold -500`; Bob `+500` |
| Provider $5 cash out reserved | Alice `-500`; `system:provider_hold +500` |
| Provider cash out processed | `system:provider_hold -500`; `system:provider_clearing +500` |
| Unprocessed reserved transfer fails | Hold `-500`; owner `+500`, subject to debt repayment |

Queuing an incoming deposit creates no spendable credit. A reserved outgoing transfer already reduced the sender's account; processing it does not debit the sender again.

## Returned deposit example

Alice deposits $100, pays Bob $80, and has $20 left when the original deposit is returned.

The return posts `system:provider_equity +10000`, Alice `-2000`, and `system:provider_loss -8000`. The journal still balances. Alice's user balance becomes zero rather than negative, and `provider_customers.debt_cents` records the $80 shortfall. Spending is blocked with the reason `debt`.

If a later $90 deposit is confirmed, its credit is split: `system:provider_equity -9000`, `system:provider_loss +8000`, Alice `+1000`. Debt becomes zero, and a debt-only block can clear. Another reason for review is not cleared merely because debt is repaid.

A credit or released reservation first repays debt through the shared `credit` helper. The code permits verified bank funding to repay a debt-only block, but it does not allow a normal peer payment into a recipient currently blocked by debt. Do not assume all frozen-wallet incoming operations are accepted.

For returned peer payments and cash outs, the code freezes affected wallets and does not invent an automatic refund. It retains the existing accounting result or reserve until the actual external disposition can be reviewed. There is no general operator-resolution command/UI yet.

## Database map

Schemas are applied by `Store::open_with_rail`: the [wallet schema](../crates/mesh-server/src/schema.sql), [bank outbox schema](../crates/mesh-banking/src/schema.sql), and [provider schema](../crates/mesh-server/src/provider.sql). Tables can exist in a demo database even though their provider workflows are inactive.

| Tables | Purpose |
|---|---|
| `metadata` | Ledger ID, rail pin, consumed bank-event cursor |
| `accounts`, `journal`, `entries` | Materialized balances and balanced journal history |
| `payments`, `rejected_payments` | Completed payment records and durable insufficient-funds tombstones |
| `nonces` | RPC replay protection; old rows are pruned during new requests |
| `banks`, `withdrawals` | Default-rail linked-bank credentials and local payout queue |
| `provider_customers`, `provider_banks` | Wallet-bound customer/balance/bank IDs, verification, debt, block reason |
| `provider_setup` | Encrypted staged customer/bank setup, stable creation keys, lease and retry timing |
| `provider_link_requests` | Immutable Link-intent receipts retained when an expired sign-in is replaced |
| `provider_requests` | Wallet-owned funding/payment/cash-out intent and applied/reversed flags |
| `bank_jobs` | Immutable provider operation, known external transfer, uncertainty, lease, next attempt |
| `bank_status_events` | Ordered outbox status log consumed by wallet accounting |
| `bank_transfer_links` | Related external legs that can wake an original transfer |
| `bank_webhooks` | Authenticated notification IDs and body digests for deduplication |

The current schema mechanism is additive `CREATE TABLE IF NOT EXISTS` plus guarded metadata initialization. It is not a general versioned migration framework. A future field/constraint migration needs explicit planning and restore testing; editing a schema file alone does not alter existing tables.

## Outbox and event transactions

A wallet reservation, `provider_requests` row, initial `bank_jobs` row, and initial status event commit together. The worker uses a separate connection to the **same file**, so it can make network calls without holding the wallet transaction or synchronous application mutex.

A claim has a random lease token and 90-second expiry. Persisting the provider ID or observation requires the current token and an unexpired lease. The external idempotency/correlation ID comes from the persisted operation UUID. Repeating a submission must retain the exact operation fields.

`apply_bank_events` reads up to 500 ordered events after `metadata.bank_event_cursor`. It checks the operation against its wallet request, applies each financial effect at most once, updates request state, and advances the cursor in one immediate transaction. A failure rolls back both entries and cursor. An unowned operation or inconsistent transition is an error, not permission to manufacture a wallet credit.

Do not enqueue administrative `mesh-bank` operations into the application database. They have no matching wallet request/reservation and stop safe event application.

## Reconciliation and restrictions

The worker nominally ticks every two seconds. A pass applies events, advances one setup job, runs one transfer job, reapplies events, and checks an eligible provider balance. Network calls and scheduling affect actual latency.

Pending observations are ordinarily retried after 30 seconds; processed transfers continue to be observed approximately hourly for late failures. Unknown submissions use backoff and stop at the 23-hour cutoff. Known transfers are polled rather than resubmitted. These are code timings, not bank-settlement promises.

A provider balance comparison selects an eligible verified wallet without active transfer requests and checks it after its recorded check is at least five minutes old. The expected provider balance is local user balance minus recorded debt. Before saving the result, the code rechecks local state under a write transaction. Drift freezes spending with `balance_review`; it never posts a compensating credit. Provider customer restrictions can also freeze a wallet. Known transfers continue to be observed even while new submissions are restricted.

## Client available balance

[`WalletModel.availableCents`](../ios/MeshPay/WalletModel.swift) combines these sources:

1. Start from cached `snapshot.balance_cents`.
2. Exclude active `reserved_payment_ids` from local outgoing reservation subtraction; the server has already reserved those funds.
3. Ask Rust to subtract distinct, unexpired, outgoing pending payments from the remaining local journal.
4. Subtract an uncertain cash-out attempt if the snapshot does not yet show its ID.
5. Clamp at zero. An ineligible provider wallet displays zero available for spending.

Example: the server snapshot is $100 after reserving a $25 payment. If that payment ID is in `reserved_payment_ids`, the client displays $100, not $75. An additional unsynced $10 payment would reduce the local display to $90.

Pending incoming payments and deposits never add to this calculation. These client checks improve the experience; the server still serializes balance checks against competing signed instructions.
