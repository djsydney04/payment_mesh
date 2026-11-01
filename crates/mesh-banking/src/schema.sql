PRAGMA journal_mode=WAL;
PRAGMA busy_timeout=5000;
CREATE TABLE IF NOT EXISTS bank_jobs (
    id TEXT PRIMARY KEY,
    operation TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('queued','submitting','submitted','processed','failed','cancelled','returned','needs_review')),
    provider_transfer TEXT UNIQUE,
    first_submission_at INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0,
    uncertain INTEGER NOT NULL DEFAULT 0,
    was_processed INTEGER NOT NULL DEFAULT 0,
    lease_id TEXT,
    lease_until INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS bank_jobs_due ON bank_jobs(next_attempt_at,lease_until,status);
CREATE TABLE IF NOT EXISTS bank_transfer_links (
    operation_id TEXT NOT NULL,
    transfer_id TEXT NOT NULL,
    PRIMARY KEY(operation_id,transfer_id)
);
CREATE INDEX IF NOT EXISTS bank_transfer_links_transfer ON bank_transfer_links(transfer_id);
CREATE TABLE IF NOT EXISTS bank_status_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    operation_id TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS bank_webhooks (
    id TEXT PRIMARY KEY,
    digest TEXT NOT NULL,
    received_at INTEGER NOT NULL
);
