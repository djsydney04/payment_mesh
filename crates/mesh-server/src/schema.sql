PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS accounts (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    system INTEGER NOT NULL DEFAULT 0 CHECK(system IN (0,1)),
    balance_cents INTEGER NOT NULL DEFAULT 0 CHECK(system = 1 OR balance_cents >= 0),
    profile TEXT,
    demo_funded INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS journal (
    id TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS entries (
    journal_id TEXT NOT NULL REFERENCES journal(id),
    account_id TEXT NOT NULL REFERENCES accounts(id),
    amount_cents INTEGER NOT NULL,
    PRIMARY KEY(journal_id, account_id)
);
CREATE TABLE IF NOT EXISTS payments (
    id TEXT PRIMARY KEY,
    sender TEXT NOT NULL REFERENCES accounts(id),
    recipient TEXT NOT NULL REFERENCES accounts(id),
    amount_cents INTEGER NOT NULL CHECK(amount_cents > 0),
    memo TEXT NOT NULL,
    payment TEXT NOT NULL,
    settled_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS payments_sender ON payments(sender,settled_at);
CREATE INDEX IF NOT EXISTS payments_recipient ON payments(recipient,settled_at);
CREATE TABLE IF NOT EXISTS rejected_payments (
    id TEXT PRIMARY KEY,
    payment TEXT NOT NULL,
    reason TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS nonces (
    wallet TEXT NOT NULL,
    nonce TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    PRIMARY KEY(wallet,nonce)
);
CREATE INDEX IF NOT EXISTS nonces_timestamp ON nonces(timestamp);
CREATE TABLE IF NOT EXISTS banks (
    wallet TEXT PRIMARY KEY REFERENCES accounts(id),
    label TEXT NOT NULL,
    mask TEXT NOT NULL,
    encrypted_credentials TEXT NOT NULL,
    mode TEXT NOT NULL CHECK(mode IN ('demo','plaid_sandbox'))
);
CREATE TABLE IF NOT EXISTS withdrawals (
    id TEXT PRIMARY KEY,
    wallet TEXT NOT NULL REFERENCES accounts(id),
    amount_cents INTEGER NOT NULL CHECK(amount_cents > 0),
    status TEXT NOT NULL CHECK(status IN ('queued','submitted','settled','failed')),
    bank_label TEXT NOT NULL,
    bank_mask TEXT NOT NULL,
    bank_credentials TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    provider_reference TEXT
);
CREATE INDEX IF NOT EXISTS withdrawals_wallet ON withdrawals(wallet,created_at);
INSERT OR IGNORE INTO accounts(id,name,system) VALUES
    ('system:demo_equity','Demo funding',1),
    ('system:payout_reserve','Reserved payouts',1),
    ('system:bank_clearing','Demo bank clearing',1);
