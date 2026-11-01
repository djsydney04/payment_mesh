CREATE TABLE IF NOT EXISTS provider_customers (
    wallet TEXT PRIMARY KEY REFERENCES accounts(id),
    customer_id TEXT UNIQUE,
    balance_source TEXT UNIQUE,
    status TEXT NOT NULL DEFAULT 'pending',
    blocked TEXT NOT NULL DEFAULT '',
    debt_cents INTEGER NOT NULL DEFAULT 0 CHECK(debt_cents>=0),
    checked_at INTEGER,
    observed_balance INTEGER
);
CREATE TABLE IF NOT EXISTS provider_banks (
    wallet TEXT PRIMARY KEY REFERENCES accounts(id),
    source TEXT NOT NULL UNIQUE,
    label TEXT NOT NULL,
    mask TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS provider_setup (
    id TEXT PRIMARY KEY,
    wallet TEXT NOT NULL REFERENCES accounts(id),
    kind TEXT NOT NULL CHECK(kind IN ('enroll','bank')),
    phase TEXT NOT NULL,
    encrypted_payload TEXT NOT NULL,
    resource_id TEXT,
    partner_id TEXT,
    next_key TEXT NOT NULL,
    first_attempt INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt INTEGER NOT NULL,
    lease_id TEXT,
    lease_until INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    UNIQUE(wallet,kind)
);
CREATE TABLE IF NOT EXISTS provider_requests (
    id TEXT PRIMARY KEY REFERENCES bank_jobs(id),
    wallet TEXT NOT NULL REFERENCES accounts(id),
    recipient TEXT REFERENCES accounts(id),
    kind TEXT NOT NULL CHECK(kind IN ('funding','peer_payment','cash_out')),
    amount_cents INTEGER NOT NULL CHECK(amount_cents>0),
    payment TEXT,
    memo TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'queued',
    applied INTEGER NOT NULL DEFAULT 0 CHECK(applied IN (0,1)),
    reversed INTEGER NOT NULL DEFAULT 0 CHECK(reversed IN (0,1)),
    bank_label TEXT NOT NULL DEFAULT '',
    bank_mask TEXT NOT NULL DEFAULT '',
    authorization TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS provider_requests_wallet ON provider_requests(wallet,created_at);
INSERT OR IGNORE INTO accounts(id,name,system) VALUES
    ('system:provider_hold','Bank transfer reservations',1),
    ('system:provider_equity','Provider confirmed funds',1),
    ('system:provider_clearing','Provider cash outs',1),
    ('system:provider_loss','Returned funds receivable',1);
INSERT OR IGNORE INTO metadata(key,value) VALUES ('bank_event_cursor','0');
-- Completed/expired Link attempts remain as immutable receipts even after a new sign-in.
CREATE TABLE IF NOT EXISTS provider_link_requests (
    id TEXT PRIMARY KEY,
    wallet TEXT NOT NULL REFERENCES accounts(id),
    fingerprint TEXT NOT NULL,
    setup_id TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
