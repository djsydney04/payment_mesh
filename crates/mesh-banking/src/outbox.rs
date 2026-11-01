//! Persist before any provider side effect. Leases fence stale workers; provider keys fence retries.
//!
//! Enqueue within the wallet's reservation transaction using
//! [`Outbox::enqueue_transaction`] when integrating money movement. Run network work
//! after commit. A known transfer is observed rather than posted again. Unknown
//! outcomes past the local safe submission window become review cases. Status
//! events are durable; their consumer must apply effects and its cursor atomically.
use crate::*;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

/// Durable intent, leases, transfer identities, and observation events in SQLite.
pub struct Outbox {
    pub connection: Connection,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub operation: BankOperation,
    pub status: String,
    pub provider_transfer: Option<Uuid>,
    pub first_submission_at: Option<i64>,
    pub was_processed: bool,
    pub attempts: i64,
    pub uncertain: bool,
}
struct Claim {
    job: Job,
    lease: String,
}
struct DueJob {
    id: String,
    status: String,
    provider: Option<String>,
    first: Option<i64>,
    attempts: i64,
}
enum ClaimResult {
    Idle,
    Review(Uuid),
    Claimed(Claim),
}
#[derive(Debug, PartialEq, Eq)]
pub enum Work {
    Idle,
    Observed(Uuid, TransferStatus),
    Deferred(Uuid),
    NeedsReview(Uuid),
    Failed(Uuid),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusEvent {
    pub sequence: i64,
    pub operation_id: Uuid,
    pub status: String,
    pub created_at: i64,
}

impl Outbox {
    pub fn initialize(connection: &Connection) -> Result<()> {
        connection.execute_batch(include_str!("schema.sql"))?;
        Ok(())
    }
    pub fn open(path: &str) -> Result<Self> {
        let connection = Connection::open(path)?;
        Self::initialize(&connection)?;
        Ok(Self { connection })
    }
    pub fn enqueue(&mut self, operation: &BankOperation, at: i64) -> Result<Job> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::enqueue_transaction(&tx, operation, at)?;
        tx.commit()?;
        self.job(operation.id)?.ok_or(Error::Store)
    }
    pub fn enqueue_transaction(
        tx: &rusqlite::Transaction<'_>,
        operation: &BankOperation,
        at: i64,
    ) -> Result<()> {
        operation.validate()?;
        let encoded = serde_json::to_string(operation)?;
        let old: Option<String> = tx
            .query_row(
                "SELECT operation FROM bank_jobs WHERE id=?",
                [operation.id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(old) = old {
            if serde_json::from_str::<BankOperation>(&old)? != *operation {
                return Err(Error::Conflict);
            }
        } else {
            if operation
                .submit_before
                .is_some_and(|deadline| deadline <= at)
            {
                return Err(Error::Expired);
            }
            tx.execute("INSERT INTO bank_jobs(id,operation,status,next_attempt_at,created_at,updated_at) VALUES (?,?,'queued',?,?,?)",params![operation.id.to_string(),encoded,at,at,at])?;
            tx.execute("INSERT INTO bank_status_events(operation_id,status,created_at) VALUES (?,'queued',?)",params![operation.id.to_string(),at])?;
        }
        Ok(())
    }
    pub fn job(&self, id: Uuid) -> Result<Option<Job>> {
        let row = self.connection.query_row("SELECT operation,status,provider_transfer,first_submission_at,was_processed,attempts,uncertain FROM bank_jobs WHERE id=?",[id.to_string()], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<i64>>(3)?,r.get::<_,bool>(4)?,r.get::<_,i64>(5)?,r.get::<_,bool>(6)?))).optional()?;
        row.map(
            |(
                operation,
                status,
                provider,
                first_submission_at,
                was_processed,
                attempts,
                uncertain,
            )| {
                Ok(Job {
                    operation: serde_json::from_str(&operation)?,
                    status,
                    provider_transfer: provider
                        .map(|p| Uuid::parse_str(&p).map_err(|_| Error::Store))
                        .transpose()?,
                    first_submission_at,
                    was_processed,
                    attempts,
                    uncertain,
                })
            },
        )
        .transpose()
    }
    fn claim(&mut self, at: i64) -> Result<ClaimResult> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = tx.query_row("SELECT id,status,provider_transfer,first_submission_at,attempts FROM bank_jobs WHERE status IN ('queued','submitting','submitted','processed') AND next_attempt_at<=? AND lease_until<=? ORDER BY next_attempt_at,created_at,id LIMIT 1",params![at,at], |r| Ok(DueJob {id:r.get(0)?,status:r.get(1)?,provider:r.get(2)?,first:r.get(3)?,attempts:r.get(4)?})).optional()?;
        let Some(DueJob {
            id,
            status,
            provider,
            first,
            attempts,
        }) = row
        else {
            return Ok(ClaimResult::Idle);
        };
        let encoded: String =
            tx.query_row("SELECT operation FROM bank_jobs WHERE id=?", [&id], |r| {
                r.get(0)
            })?;
        let operation: BankOperation = serde_json::from_str(&encoded)?;
        if provider.is_none()
            && operation
                .submit_before
                .is_some_and(|deadline| deadline <= at)
        {
            let status = if first.is_some() {
                "needs_review"
            } else {
                "cancelled"
            };
            tx.execute(
                "UPDATE bank_jobs SET status=?,updated_at=?,lease_id=NULL,lease_until=0 WHERE id=?",
                params![status, at, id],
            )?;
            tx.execute(
                "INSERT INTO bank_status_events(operation_id,status,created_at) VALUES (?,?,?)",
                params![id, status, at],
            )?;
            tx.commit()?;
            return Ok(ClaimResult::Review(
                Uuid::parse_str(&id).map_err(|_| Error::Store)?,
            ));
        }
        if provider.is_none()
            && first.is_some_and(|first| at.saturating_sub(first) >= SAFE_RESUBMIT_SECONDS)
        {
            tx.execute("UPDATE bank_jobs SET status='needs_review',updated_at=?,lease_id=NULL,lease_until=0 WHERE id=?",params![at,id])?;
            tx.execute("INSERT INTO bank_status_events(operation_id,status,created_at) VALUES (?,'needs_review',?)",params![id,at])?;
            tx.commit()?;
            return Ok(ClaimResult::Review(
                Uuid::parse_str(&id).map_err(|_| Error::Store)?,
            ));
        }
        let lease = Uuid::new_v4().to_string();
        if provider.is_none() {
            tx.execute("UPDATE bank_jobs SET status='submitting',first_submission_at=COALESCE(first_submission_at,?),attempts=attempts+1,uncertain=CASE WHEN attempts>0 THEN 1 ELSE uncertain END,lease_id=?,lease_until=?,updated_at=? WHERE id=?",params![at,lease,at.saturating_add(90),at,id])?;
            if status != "submitting" {
                tx.execute("INSERT INTO bank_status_events(operation_id,status,created_at) VALUES (?,'submitting',?)",params![id,at])?;
            }
        } else {
            tx.execute(
                "UPDATE bank_jobs SET lease_id=?,lease_until=?,updated_at=? WHERE id=?",
                params![lease, at.saturating_add(90), at, id],
            )?;
        }
        tx.commit()?;
        let id = Uuid::parse_str(&id).map_err(|_| Error::Store)?;
        let mut job = self.job(id)?.ok_or(Error::Store)?;
        // A previous attempt that lost its response remains ambiguous until a provider ID is found.
        if provider.is_none() && attempts > 0 {
            job.uncertain = true;
        }
        Ok(ClaimResult::Claimed(Claim { job, lease }))
    }
    fn submitted(&mut self, claim: &mut Claim, transfer: Uuid, at: i64) -> Result<()> {
        if transfer.is_nil() {
            return Err(Error::Protocol);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute("UPDATE bank_jobs SET provider_transfer=?,status='submitted',updated_at=? WHERE id=? AND lease_id=? AND lease_until>? AND provider_transfer IS NULL",params![transfer.to_string(),at,claim.job.operation.id.to_string(),claim.lease,at])?;
        if changed != 1 {
            return Err(Error::LostLease);
        }
        tx.execute("INSERT INTO bank_status_events(operation_id,status,created_at) VALUES (?,'submitted',?)",params![claim.job.operation.id.to_string(),at])?;
        tx.commit()?;
        claim.job.provider_transfer = Some(transfer);
        claim.job.status = "submitted".into();
        Ok(())
    }
    fn change(
        &mut self,
        claim: &Claim,
        status: &str,
        at: i64,
        delay: i64,
        uncertain: bool,
    ) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute("UPDATE bank_jobs SET status=?,uncertain=?,was_processed=CASE WHEN ?='processed' THEN 1 ELSE was_processed END,updated_at=?,next_attempt_at=?,lease_id=NULL,lease_until=0 WHERE id=? AND lease_id=? AND lease_until>?",params![status,uncertain,status,at,at.saturating_add(delay),claim.job.operation.id.to_string(),claim.lease,at])?;
        if changed != 1 {
            return Err(Error::LostLease);
        }
        if status != claim.job.status {
            tx.execute(
                "INSERT INTO bank_status_events(operation_id,status,created_at) VALUES (?,?,?)",
                params![claim.job.operation.id.to_string(), status, at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    fn observed(
        &mut self,
        claim: &Claim,
        observation: &TransferObservation,
        at: i64,
    ) -> Result<Work> {
        let id = claim.job.operation.id;
        if Some(observation.id) != claim.job.provider_transfer
            || !observation.matches(&claim.job.operation)
        {
            self.change(claim, "needs_review", at, 0, true)?;
            return Ok(Work::NeedsReview(id));
        }
        if observation.related_transfers.len() > 8
            || observation.related_transfers.iter().any(Uuid::is_nil)
        {
            self.change(claim, "needs_review", at, 0, true)?;
            return Ok(Work::NeedsReview(id));
        }
        for transfer in &observation.related_transfers {
            self.connection.execute(
                "INSERT OR IGNORE INTO bank_transfer_links(operation_id,transfer_id) VALUES (?,?)",
                params![id.to_string(), transfer.to_string()],
            )?;
        }
        let status = match observation.status {
            TransferStatus::Pending if claim.job.was_processed => "needs_review",
            TransferStatus::Pending => "submitted",
            TransferStatus::Processed => "processed",
            TransferStatus::Failed if claim.job.was_processed => "returned",
            TransferStatus::Cancelled if claim.job.was_processed => "needs_review",
            TransferStatus::Failed => "failed",
            TransferStatus::Cancelled => "cancelled",
            TransferStatus::Returned => "returned",
        };
        self.change(
            claim,
            status,
            at,
            if status == "processed" { 3600 } else { 30 },
            false,
        )?;
        if status == "needs_review" {
            Ok(Work::NeedsReview(id))
        } else {
            Ok(Work::Observed(
                id,
                if status == "returned" {
                    TransferStatus::Returned
                } else {
                    observation.status
                },
            ))
        }
    }
    /// No database transaction is held over any await. Call from one worker per connection;
    /// other connections/processes may enqueue or claim safely through SQLite leases.
    pub async fn run_once<R: PaymentRail>(&mut self, rail: &R) -> Result<Work> {
        self.run_once_with_clock(rail, mesh_core::now).await
    }
    pub async fn run_once_with_clock<R: PaymentRail, C: Fn() -> i64>(
        &mut self,
        rail: &R,
        clock: C,
    ) -> Result<Work> {
        let mut claim = match self.claim(clock())? {
            ClaimResult::Idle => return Ok(Work::Idle),
            ClaimResult::Review(id) => return Ok(Work::NeedsReview(id)),
            ClaimResult::Claimed(claim) => claim,
        };
        let id = claim.job.operation.id;
        let transfer = if let Some(transfer) = claim.job.provider_transfer {
            transfer
        } else {
            match rail.submit(&claim.job.operation).await {
                Ok(transfer) => {
                    self.submitted(&mut claim, transfer, clock())?;
                    transfer
                }
                Err(Error::Rejected | Error::Expired) => {
                    let status = if claim.job.uncertain {
                        "needs_review"
                    } else {
                        "failed"
                    };
                    self.change(&claim, status, clock(), 0, claim.job.uncertain)?;
                    return Ok(if status == "failed" {
                        Work::Failed(id)
                    } else {
                        Work::NeedsReview(id)
                    });
                }
                Err(_) => {
                    let delay = 2i64.saturating_pow(claim.job.attempts.min(8) as u32).max(5);
                    self.change(&claim, "submitting", clock(), delay, true)?;
                    return Ok(Work::Deferred(id));
                }
            }
        };
        match rail.observe(transfer).await {
            Ok(observation) => self.observed(&claim, &observation, clock()),
            Err(_) => {
                self.change(&claim, &claim.job.status, clock(), 30, claim.job.uncertain)?;
                Ok(Work::Deferred(id))
            }
        }
    }
    pub fn events_after(&self, sequence: i64, limit: usize) -> Result<Vec<StatusEvent>> {
        if limit == 0 || limit > 1000 {
            return Err(Error::Invalid("Event page size must be 1–1000"));
        }
        let rows: Vec<(i64,String,String,i64)> = self.connection.prepare("SELECT sequence,operation_id,status,created_at FROM bank_status_events WHERE sequence>? ORDER BY sequence LIMIT ?")?.query_map(params![sequence,limit as i64], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?.collect::<std::result::Result<_,_>>()?;
        rows.into_iter()
            .map(|(sequence, id, status, created_at)| {
                Ok(StatusEvent {
                    sequence,
                    operation_id: Uuid::parse_str(&id).map_err(|_| Error::Store)?,
                    status,
                    created_at,
                })
            })
            .collect()
    }
    /// Webhooks only wake reconciliation. They cannot directly change financial state.
    pub fn receive_webhook(
        &mut self,
        event: &crate::webhook::VerifiedEvent,
        at: i64,
    ) -> Result<bool> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT digest FROM bank_webhooks WHERE id=?",
                [event.id().to_string()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(digest) = existing {
            if digest != event.digest() {
                return Err(Error::Conflict);
            }
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO bank_webhooks(id,digest,received_at) VALUES (?,?,?)",
            params![event.id().to_string(), event.digest(), at],
        )?;
        if let Some(transfer) = event.transfer_id() {
            tx.execute("UPDATE bank_jobs SET next_attempt_at=? WHERE (provider_transfer=? OR id IN (SELECT operation_id FROM bank_transfer_links WHERE transfer_id=?)) AND status IN ('submitted','processed')",params![at,transfer.to_string(),transfer.to_string()])?;
        }
        tx.commit()?;
        Ok(true)
    }
}
