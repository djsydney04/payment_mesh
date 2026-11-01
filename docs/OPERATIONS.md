# Sandbox operations

[Documentation index](README.md) · [Troubleshooting](TROUBLESHOOTING.md)

This guide describes the single-server Sandbox supplied by the repository. It is not a production banking runbook. The process owns HTTP routing and a background worker; SQLite owns durable wallet accounting, setup jobs, outbox state, and reconciliation progress.

## Configuration

The server reads environment variables at startup. It does not load `.env` files automatically. Paths are relative to the working directory unless absolute. Keep secrets out of source control, command output, screenshots, and support bundles.

| Setting | Default / constraint | Purpose |
|---|---|---|
| `MESH_MODE` | If present, must be `sandbox` | Refuse accidental live mode |
| `MESH_RAIL` | `demo`; or `dwolla_sandbox` | Select and permanently pin this database's financial rail |
| `MESH_DATABASE` | `.local/mesh.sqlite` | Wallet database; provider outbox shares this file in app provider mode |
| `MESH_BIND` | `127.0.0.1:8787` | HTTP listener |
| `MESH_VAULT_KEY` | Optional 64 lowercase hex characters | 32-byte server credential-encryption key; otherwise generated beside the database |
| `PLAID_ENV` | If present, must be `sandbox` | Enforce fixed Sandbox transport |
| `PLAID_CLIENT_ID`, `PLAID_SECRET` | Unset in local demo | Plaid credentials, required together with redirect URI when linking is enabled |
| `PLAID_REDIRECT_URI` | Registered HTTPS URI | Plaid OAuth return; also configure the native associated domain |
| `DWOLLA_ENV` | If present, must be `sandbox` | Enforce fixed Sandbox transport |
| `DWOLLA_KEY`, `DWOLLA_SECRET` | Required for provider app rail | Dwolla Sandbox OAuth credentials |
| `DWOLLA_WEBHOOK_SECRET` | Optional in provider app rail; 16–1,024 bytes | Authenticate raw provider notifications |
| `MESH_BANK_OUTBOX` | Omit for normal app operation | Separate administrative runner's webhook database; see constraints below |
| `RUST_LOG` | `mesh_server=info` | Rust tracing filter |

Provider app mode requires configured Plaid and Dwolla clients. Plaid's origin is fixed to `https://sandbox.plaid.com`; Dwolla's is fixed to `https://api-sandbox.dwolla.com`. Redirects are disabled for these HTTP clients. Ordinary request timeout is 20 seconds with a 1 MiB response cap; multi-leg transfer observation has a separate bounded time budget. Credentials do not select production URLs.

In provider app mode, a webhook secret automatically uses `MESH_DATABASE`; `MESH_BANK_OUTBOX` must be omitted or use the same path representation. Do not assume two different spellings of the same filesystem path will pass the startup comparison. In demo mode, attaching webhooks for a **separate administrative acceptance runner** requires both `MESH_BANK_OUTBOX` and `DWOLLA_WEBHOOK_SECRET`. Those notifications do not turn the demo wallet into a provider-backed wallet.

Startup creates parent directories, obtains the vault key, opens/initializes the database, enforces its rail, and audits ledger consistency before serving requests. Invalid live settings, missing provider configuration, rail mismatch, or failed accounting audit cause startup failure. `GET /health` reports a running Sandbox HTTP process; it does not query bank readiness.

## Durable state and the vault key

For `MESH_DATABASE=.local/mesh.sqlite`, the default key file is `.local/mesh.sqlite.vault-key`. It is created with mode 0600. If `MESH_VAULT_KEY` is supplied, preserve that exact key in your secret storage instead. Restarting with a different key does not re-encrypt existing credentials.

The key encrypts bank/setup credential payloads using authenticated encryption and owner-specific associated data. It does not encrypt the whole SQLite file: identities, names, amounts, memos, accounting entries, and job metadata remain readable to someone with database access. Restrict access to the directory, database, and backups accordingly.

The SQLite schema uses WAL mode, foreign keys for wallet tables, and a five-second busy timeout. The database's `metadata` includes `ledger_id`, `bank_rail`, and `bank_event_cursor`. A new database has a new payment authority; pointing an existing phone at it is not recovery of the old ledger.

## Worker behavior

The in-process worker has a nominal two-second tick. Network calls run sequentially within a pass, so this is not a two-second completion promise.

In demo mode, eligible cash outs are processed after at least three seconds of age. In provider mode, each pass:

1. Applies pending outbox status events to the wallet ledger. Failure pauses new work for that pass.
2. Advances one due customer or bank setup job.
3. Advances one due transfer outbox job through the wallet-specific adapter.
4. Applies new status events to the ledger.
5. Compares one eligible wallet's provider balance/status with its local state.

Setup and transfer jobs use 90-second leases with ownership checks when saving results. A restart may wait for an old lease to expire. Unknown transfer outcomes retain their original idempotency key. The configured local safe resubmission window is 23 hours from first submission; beyond it, uncertain work requires review. A payment's own expiry can prevent a new submission earlier. A known provider transfer ID is polled rather than resubmitted, including after the original payment expires.

Pending transfers normally poll after 30 seconds; processed transfers continue periodic observation, normally after an hour, to detect later returns. Authenticated notifications wake known jobs sooner. Retry backoff and time spent in external requests also affect timing. Notifications alone never create spendable credit.

Periodic balance comparison selects eligible verified wallets with no active transfer requests and a sufficiently old check (five minutes). It rechecks local state before storing the observation. A mismatch records `balance_review`; it does not mint an adjustment. For event/cursor transaction details, read [ledger and storage](LEDGER.md).

## Read-only inspection

Prefer the wallet's signed snapshot for its visible state. For local diagnosis, SQLite's read-only mode avoids accidental financial mutations. The examples inspect metadata and job summaries without selecting encrypted credentials, public tokens, full operation payloads, or customer identity data:

```sh
sqlite3 -readonly .local/mesh.sqlite
```

Inside SQLite:

```sql
.headers on
.mode column
SELECT key, value FROM metadata ORDER BY key;
SELECT status, COUNT(*) AS jobs FROM bank_jobs GROUP BY status;
SELECT kind, phase, attempts, next_attempt, lease_until
  FROM provider_setup ORDER BY created_at;
SELECT kind, status, applied, reversed, COUNT(*) AS requests
  FROM provider_requests GROUP BY kind, status, applied, reversed;
SELECT blocked, COUNT(*) AS wallets, SUM(debt_cents) AS debt_cents
  FROM provider_customers GROUP BY blocked;
SELECT MAX(sequence) AS newest_event FROM bank_status_events;
```

Setup/provider tables are meaningful on the provider rail; the common database initializer also creates the outbox tables. A status count alone cannot distinguish external delay from a stuck application transition. Compare the outbox job, wallet request, event sequence, and saved cursor together.

These consistency queries should report zero total and no discrepancy rows:

```sql
SELECT COALESCE(SUM(balance_cents), 0) AS total FROM accounts;
SELECT journal_id, SUM(amount_cents) AS net
  FROM entries GROUP BY journal_id HAVING SUM(amount_cents) != 0;
SELECT a.id, a.balance_cents, COALESCE(SUM(e.amount_cents), 0) AS from_entries
  FROM accounts a LEFT JOIN entries e ON e.account_id = a.id
  GROUP BY a.id HAVING a.balance_cents != COALESCE(SUM(e.amount_cents), 0);
```

They help diagnose the local ledger; they do not prove external reserves, account ownership, or absence of unauthorized transfers. Startup also runs the application's audit. Do not repair a mismatch by directly changing balances, flags, operation IDs, or the event cursor.

## Backup and restore

Use SQLite's online backup mechanism for a consistent copy, for example `.backup '/absolute/private/path/mesh-backup.sqlite'` inside the read-only session. Alternatively, stop the process cleanly and preserve a complete consistent database state. Copying only the main file while WAL writes are active can lose committed changes.

Preserve the corresponding vault key separately with restricted access and retain a record of the selected rail and provider Sandbox account. Protect the backup as financial and credential data. Keep phone identities/journals through their normal local storage; the server backup cannot recreate a lost wallet seed.

Restore first into an isolated inspection location. Check SQLite integrity, ledger consistency, matching key availability, and rail metadata before any provider worker starts. Do not run a second submission worker against an old restored copy while the original remains active. An old backup can omit provider transfers that already exist externally: stable IDs help only for operations actually represented in the restored data. This prototype does not provide automated disaster-recovery reconciliation, restore orchestration, or credential-key rotation. Provider-connected restore needs a reviewed reconciliation procedure before submissions resume.

## Review states and operational limits

| State / reason | Intended response |
|---|---|
| `relink_required` before Dwolla bank creation | User starts a fresh bank sign-in; preserve the old intent receipt |
| Deposit return and `debt` | Inspect the return; future permitted funding repays debt before available credit |
| `returned_transfer` | Hold spending; establish the disposition of a peer payment or cash out |
| `transfer_review` / `needs_review` | Preserve reservations and immutable transfer evidence; resolve ambiguity |
| `balance_review` | Compare external and local histories before any adjustment |
| `customer_review` | Check the provider's customer status and required resolution |

There is no general operator resolution API/UI, audited adjustment workflow, or supported SQL recipe for clearing these restrictions. Keep the original evidence and develop a tested resolution path; deleting a block string is not a financial resolution. See [lifecycle recovery](PAYMENT_LIFECYCLE.md) and [troubleshooting](TROUBLESHOOTING.md).

The standalone `mesh-bank` acceptance runner inspects/tests the banking adapter, not the app's wallet accounting bridge. Use its documented commands only against a dedicated acceptance outbox, as described in [BANK_PROVIDER.md](BANK_PROVIDER.md). Do not enqueue arbitrary operations into the app's live Sandbox database: unowned outbox events stop wallet reconciliation.
