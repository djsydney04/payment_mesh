use hmac::{Hmac, Mac};
use mesh_banking::{
    outbox::{Outbox, Work},
    *,
};
use serde_json::json;
use sha2::Sha256;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use uuid::Uuid;

fn operation() -> BankOperation {
    BankOperation {
        id: Uuid::new_v4(),
        ledger_id: Uuid::new_v4(),
        wallet: "01".repeat(32),
        kind: OperationKind::CashOut,
        source: Uuid::new_v4(),
        destination: Uuid::new_v4(),
        amount_cents: 1250,
        submit_before: None,
    }
}
#[derive(Default)]
struct FakeRail {
    transfers: Mutex<HashMap<Uuid, TransferObservation>>,
    submissions: AtomicUsize,
    timeout_after_accept: AtomicBool,
    reject: AtomicBool,
    observation_fails: AtomicBool,
    observed_amount_wrong: AtomicBool,
    status: Mutex<Option<TransferStatus>>,
}
impl PaymentRail for FakeRail {
    async fn submit(&self, op: &BankOperation) -> Result<Uuid> {
        self.submissions.fetch_add(1, Ordering::SeqCst);
        if self.reject.load(Ordering::SeqCst) {
            return Err(Error::Rejected);
        }
        let id = {
            let mut transfers = self.transfers.lock().unwrap();
            transfers
                .entry(op.id)
                .or_insert_with(|| TransferObservation {
                    id: Uuid::new_v4(),
                    correlation_id: op.id,
                    source: op.source,
                    destination: op.destination,
                    amount_cents: op.amount_cents,
                    status: TransferStatus::Pending,
                    related_transfers: vec![],
                })
                .id
        };
        if self.timeout_after_accept.swap(false, Ordering::SeqCst) {
            return Err(Error::Retryable);
        }
        Ok(id)
    }
    async fn observe(&self, id: Uuid) -> Result<TransferObservation> {
        if self.observation_fails.load(Ordering::SeqCst) {
            return Err(Error::Retryable);
        }
        let mut observed = self
            .transfers
            .lock()
            .unwrap()
            .values()
            .find(|o| o.id == id)
            .ok_or(Error::Protocol)?
            .clone();
        observed.status = self
            .status
            .lock()
            .unwrap()
            .unwrap_or(TransferStatus::Pending);
        if self.observed_amount_wrong.load(Ordering::SeqCst) {
            observed.amount_cents += 1;
        }
        Ok(observed)
    }
}

#[test]
fn enqueue_is_durable_idempotent_and_immutable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bank.sqlite");
    let path = path.to_str().unwrap();
    let op = operation();
    {
        let mut outbox = Outbox::open(path).unwrap();
        outbox.enqueue(&op, 1000).unwrap();
        outbox.enqueue(&op, 1001).unwrap();
    }
    let mut outbox = Outbox::open(path).unwrap();
    assert_eq!(outbox.job(op.id).unwrap().unwrap().operation, op);
    let mut changed = op.clone();
    changed.amount_cents = 999;
    assert!(matches!(
        outbox.enqueue(&changed, 1002),
        Err(Error::Conflict)
    ));
    assert_eq!(outbox.events_after(0, 100).unwrap().len(), 1);
}
#[tokio::test]
async fn ambiguous_timeout_reuses_provider_key_and_does_not_duplicate_transfer() {
    let op = operation();
    let mut outbox = Outbox::open(":memory:").unwrap();
    let rail = FakeRail::default();
    outbox.enqueue(&op, 1000).unwrap();
    rail.timeout_after_accept.store(true, Ordering::SeqCst);
    assert_eq!(
        outbox.run_once_with_clock(&rail, || 1000).await.unwrap(),
        Work::Deferred(op.id)
    );
    assert_eq!(outbox.job(op.id).unwrap().unwrap().status, "submitting");
    outbox.run_once_with_clock(&rail, || 1010).await.unwrap();
    assert_eq!(rail.submissions.load(Ordering::SeqCst), 2);
    assert_eq!(rail.transfers.lock().unwrap().len(), 1);
    assert_eq!(outbox.job(op.id).unwrap().unwrap().status, "submitted");
}
#[tokio::test]
async fn unknown_submission_stops_before_provider_idempotency_expires() {
    let op = operation();
    let mut outbox = Outbox::open(":memory:").unwrap();
    let rail = FakeRail::default();
    outbox.enqueue(&op, 1000).unwrap();
    rail.timeout_after_accept.store(true, Ordering::SeqCst);
    outbox.run_once_with_clock(&rail, || 1000).await.unwrap();
    outbox
        .run_once_with_clock(&rail, || 1000 + SAFE_RESUBMIT_SECONDS)
        .await
        .unwrap();
    assert_eq!(outbox.job(op.id).unwrap().unwrap().status, "needs_review");
    assert_eq!(rail.submissions.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn a_job_that_has_never_been_submitted_can_start_after_a_long_queue_wait() {
    let op = operation();
    let mut outbox = Outbox::open(":memory:").unwrap();
    let rail = FakeRail::default();
    outbox.enqueue(&op, 1000).unwrap();
    outbox
        .run_once_with_clock(&rail, || 1000 + 100 * 86400)
        .await
        .unwrap();
    assert_eq!(outbox.job(op.id).unwrap().unwrap().status, "submitted");
    assert_eq!(rail.submissions.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn known_provider_transfer_is_polled_after_24_hours_without_resubmitting() {
    let op = operation();
    let mut outbox = Outbox::open(":memory:").unwrap();
    let rail = FakeRail::default();
    outbox.enqueue(&op, 1000).unwrap();
    outbox.run_once_with_clock(&rail, || 1000).await.unwrap();
    *rail.status.lock().unwrap() = Some(TransferStatus::Processed);
    outbox
        .run_once_with_clock(&rail, || 1000 + 10 * 86400)
        .await
        .unwrap();
    assert_eq!(outbox.job(op.id).unwrap().unwrap().status, "processed");
    assert_eq!(rail.submissions.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn observation_must_match_amount_routing_and_correlation() {
    let op = operation();
    let mut outbox = Outbox::open(":memory:").unwrap();
    let rail = FakeRail::default();
    outbox.enqueue(&op, 1000).unwrap();
    rail.observed_amount_wrong.store(true, Ordering::SeqCst);
    assert_eq!(
        outbox.run_once_with_clock(&rail, || 1000).await.unwrap(),
        Work::NeedsReview(op.id)
    );
    assert_eq!(outbox.job(op.id).unwrap().unwrap().status, "needs_review");
}
#[tokio::test]
async fn processed_transfer_keeps_reconciling_and_late_failure_becomes_returned() {
    let op = operation();
    let mut outbox = Outbox::open(":memory:").unwrap();
    let rail = FakeRail::default();
    outbox.enqueue(&op, 1000).unwrap();
    *rail.status.lock().unwrap() = Some(TransferStatus::Processed);
    outbox.run_once_with_clock(&rail, || 1000).await.unwrap();
    *rail.status.lock().unwrap() = Some(TransferStatus::Failed);
    assert_eq!(
        outbox.run_once_with_clock(&rail, || 5000).await.unwrap(),
        Work::Observed(op.id, TransferStatus::Returned)
    );
    let statuses: Vec<_> = outbox
        .events_after(0, 100)
        .unwrap()
        .iter()
        .map(|e| e.status.clone())
        .collect();
    assert_eq!(
        statuses,
        vec!["queued", "submitting", "submitted", "processed", "returned"]
    );
}
#[tokio::test]
async fn a_regression_from_processed_to_pending_is_not_accepted_as_fresh_success() {
    let op = operation();
    let mut outbox = Outbox::open(":memory:").unwrap();
    let rail = FakeRail::default();
    outbox.enqueue(&op, 1000).unwrap();
    *rail.status.lock().unwrap() = Some(TransferStatus::Processed);
    outbox.run_once_with_clock(&rail, || 1000).await.unwrap();
    *rail.status.lock().unwrap() = Some(TransferStatus::Pending);
    assert_eq!(
        outbox.run_once_with_clock(&rail, || 5000).await.unwrap(),
        Work::NeedsReview(op.id)
    );
}
#[tokio::test]
async fn a_decline_after_an_ambiguous_submission_requires_reconciliation() {
    let op = operation();
    let mut outbox = Outbox::open(":memory:").unwrap();
    let rail = FakeRail::default();
    outbox.enqueue(&op, 1000).unwrap();
    rail.timeout_after_accept.store(true, Ordering::SeqCst);
    outbox.run_once_with_clock(&rail, || 1000).await.unwrap();
    rail.reject.store(true, Ordering::SeqCst);
    assert_eq!(
        outbox.run_once_with_clock(&rail, || 1010).await.unwrap(),
        Work::NeedsReview(op.id)
    );
}
#[tokio::test]
async fn a_definite_initial_decline_is_terminal_without_retries() {
    let op = operation();
    let mut outbox = Outbox::open(":memory:").unwrap();
    let rail = FakeRail::default();
    outbox.enqueue(&op, 1000).unwrap();
    rail.reject.store(true, Ordering::SeqCst);
    assert_eq!(
        outbox.run_once_with_clock(&rail, || 1000).await.unwrap(),
        Work::Failed(op.id)
    );
    assert_eq!(
        outbox.run_once_with_clock(&rail, || 1100).await.unwrap(),
        Work::Idle
    );
}
#[tokio::test]
async fn restart_after_acceptance_keeps_provider_reference_and_never_reposts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("outbox.sqlite");
    let path = path.to_str().unwrap();
    let op = operation();
    let rail = FakeRail::default();
    {
        let mut outbox = Outbox::open(path).unwrap();
        outbox.enqueue(&op, 1000).unwrap();
        rail.observation_fails.store(true, Ordering::SeqCst);
        outbox.run_once_with_clock(&rail, || 1000).await.unwrap();
    }
    let mut outbox = Outbox::open(path).unwrap();
    rail.observation_fails.store(false, Ordering::SeqCst);
    *rail.status.lock().unwrap() = Some(TransferStatus::Processed);
    outbox.run_once_with_clock(&rail, || 1100).await.unwrap();
    assert_eq!(rail.submissions.load(Ordering::SeqCst), 1);
    assert_eq!(outbox.job(op.id).unwrap().unwrap().status, "processed");
}

struct SlowRail {
    inner: Arc<FakeRail>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl PaymentRail for SlowRail {
    async fn submit(&self, op: &BankOperation) -> Result<Uuid> {
        self.entered.notify_one();
        self.release.notified().await;
        self.inner.submit(op).await
    }
    async fn observe(&self, id: Uuid) -> Result<TransferObservation> {
        self.inner.observe(id).await
    }
}
#[tokio::test]
async fn independent_workers_cannot_claim_the_same_live_lease() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bank.sqlite");
    let path = path.to_str().unwrap();
    let op = operation();
    let mut first = Outbox::open(path).unwrap();
    let mut second = Outbox::open(path).unwrap();
    first.enqueue(&op, 1000).unwrap();
    let rail = Arc::new(SlowRail {
        inner: Arc::new(FakeRail::default()),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let rail_first = rail.clone();
    let first_worker = tokio::spawn(async move {
        first
            .run_once_with_clock(rail_first.as_ref(), || 1000)
            .await
    });
    rail.entered.notified().await;
    assert_eq!(
        second
            .run_once_with_clock(rail.as_ref(), || 1000)
            .await
            .unwrap(),
        Work::Idle
    );
    rail.release.notify_one();
    first_worker.await.unwrap().unwrap();
    assert_eq!(rail.inner.submissions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_expired_worker_cannot_overwrite_the_new_workers_provider_receipt() {
    use std::sync::atomic::AtomicI64;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bank.sqlite");
    let path = path.to_str().unwrap();
    let operation = operation();
    let mut first = Outbox::open(path).unwrap();
    let mut second = Outbox::open(path).unwrap();
    first.enqueue(&operation, 1000).unwrap();
    let rail = Arc::new(SlowRail {
        inner: Arc::new(FakeRail::default()),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let clock = Arc::new(AtomicI64::new(1000));
    let clock_first = clock.clone();
    let rail_first = rail.clone();
    let worker = tokio::spawn(async move {
        first
            .run_once_with_clock(rail_first.as_ref(), || clock_first.load(Ordering::SeqCst))
            .await
    });
    rail.entered.notified().await;
    clock.store(1100, Ordering::SeqCst);
    second
        .run_once_with_clock(rail.inner.as_ref(), || 1100)
        .await
        .unwrap();
    let before = second.job(operation.id).unwrap().unwrap();
    rail.release.notify_one();
    assert!(matches!(worker.await.unwrap(), Err(Error::LostLease)));
    let after = second.job(operation.id).unwrap().unwrap();
    assert_eq!(after.provider_transfer, before.provider_transfer);
    assert_eq!(after.status, "submitted");
    assert_eq!(rail.inner.transfers.lock().unwrap().len(), 1);
}

struct CrashRail {
    inner: Arc<FakeRail>,
    accepted: tokio::sync::Notify,
    never_release: tokio::sync::Notify,
}
impl PaymentRail for CrashRail {
    async fn submit(&self, operation: &BankOperation) -> Result<Uuid> {
        let id = self.inner.submit(operation).await?;
        self.accepted.notify_one();
        self.never_release.notified().await;
        Ok(id)
    }
    async fn observe(&self, transfer: Uuid) -> Result<TransferObservation> {
        self.inner.observe(transfer).await
    }
}
#[tokio::test]
async fn process_death_between_provider_acceptance_and_receipt_storage_recovers_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bank.sqlite");
    let path = path.to_str().unwrap();
    let operation = operation();
    let mut first = Outbox::open(path).unwrap();
    first.enqueue(&operation, 1000).unwrap();
    let rail = Arc::new(CrashRail {
        inner: Arc::new(FakeRail::default()),
        accepted: tokio::sync::Notify::new(),
        never_release: tokio::sync::Notify::new(),
    });
    let worker_rail = rail.clone();
    let worker = tokio::spawn(async move {
        first
            .run_once_with_clock(worker_rail.as_ref(), || 1000)
            .await
    });
    rail.accepted.notified().await;
    worker.abort();
    let _ = worker.await;
    let mut restored = Outbox::open(path).unwrap();
    assert!(restored
        .job(operation.id)
        .unwrap()
        .unwrap()
        .provider_transfer
        .is_none());
    restored
        .run_once_with_clock(rail.inner.as_ref(), || 1091)
        .await
        .unwrap();
    assert!(restored
        .job(operation.id)
        .unwrap()
        .unwrap()
        .provider_transfer
        .is_some());
    assert_eq!(rail.inner.transfers.lock().unwrap().len(), 1);
    assert_eq!(rail.inner.submissions.load(Ordering::SeqCst), 2);
}

fn signed_event(secret: &[u8], id: Uuid, transfer: Uuid, topic: &str) -> (Vec<u8>, String) {
    let body = serde_json::to_vec(&json!({"id":id,"topic":topic,"_links":{"resource":{"href":format!("{DWOLLA_SANDBOX}/transfers/{transfer}")}}})).unwrap();
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
    mac.update(&body);
    let signature = hex::encode(mac.finalize().into_bytes());
    (body, signature)
}
#[tokio::test]
async fn signed_webhook_is_deduplicated_and_only_wakes_authoritative_reconciliation() {
    let op = operation();
    let mut outbox = Outbox::open(":memory:").unwrap();
    let rail = FakeRail::default();
    outbox.enqueue(&op, 1000).unwrap();
    *rail.status.lock().unwrap() = Some(TransferStatus::Processed);
    outbox.run_once_with_clock(&rail, || 1000).await.unwrap();
    let transfer = outbox
        .job(op.id)
        .unwrap()
        .unwrap()
        .provider_transfer
        .unwrap();
    let secret = b"webhook-test-secret-32-characters";
    let (body, sig) = signed_event(secret, Uuid::new_v4(), transfer, "customer_transfer_failed");
    let event = webhook::verify(secret, &sig, &body).unwrap();
    assert!(outbox.receive_webhook(&event, 1001).unwrap());
    assert!(!outbox.receive_webhook(&event, 1002).unwrap());
    // The stale failure topic is ignored. GET still reports processed, so no reversal is emitted.
    outbox.run_once_with_clock(&rail, || 1003).await.unwrap();
    assert_eq!(outbox.job(op.id).unwrap().unwrap().status, "processed");
    assert_eq!(
        outbox
            .events_after(0, 100)
            .unwrap()
            .iter()
            .filter(|e| e.status == "processed")
            .count(),
        1
    );
}
#[test]
fn unauthenticated_or_tampered_webhooks_cannot_wake_jobs() {
    let secret = b"webhook-test-secret-32-characters";
    let (mut body, sig) = signed_event(
        secret,
        Uuid::new_v4(),
        Uuid::new_v4(),
        "customer_transfer_completed",
    );
    assert!(webhook::verify(b"another-valid-secret-32-characters", &sig, &body).is_err());
    body.push(b' ');
    assert!(webhook::verify(secret, &sig, &body).is_err());
    assert!(webhook::verify(secret, "bad-signature", &body).is_err());
}
#[test]
fn decimal_provider_money_never_rounds() {
    for (input, expected) in [
        ("0", 0),
        ("0.00", 0),
        ("1.2", 120),
        ("12.50", 1250),
        ("1234567.89", 123456789),
    ] {
        assert_eq!(provider_cents(input).unwrap(), expected);
    }
    for invalid in [
        "-1",
        "1.005",
        "1e2",
        "NaN",
        "1.",
        ".50",
        " 1",
        "1 ",
        "9223372036854775807.99",
    ] {
        assert!(provider_cents(invalid).is_err());
    }
    assert_eq!(decimal_amount(1250).unwrap(), "12.50");
}

#[tokio::test]
async fn expired_unsubmitted_instruction_is_cancelled_but_ambiguous_submission_needs_review() {
    let mut op = operation();
    op.submit_before = Some(1010);
    let rail = FakeRail::default();
    let mut outbox = Outbox::open(":memory:").unwrap();
    outbox.enqueue(&op, 1000).unwrap();
    outbox.run_once_with_clock(&rail, || 1010).await.unwrap();
    assert_eq!(outbox.job(op.id).unwrap().unwrap().status, "cancelled");
    assert_eq!(rail.submissions.load(Ordering::SeqCst), 0);
    op.id = Uuid::new_v4();
    outbox.enqueue(&op, 1000).unwrap();
    rail.timeout_after_accept.store(true, Ordering::SeqCst);
    outbox.run_once_with_clock(&rail, || 1000).await.unwrap();
    outbox.run_once_with_clock(&rail, || 1010).await.unwrap();
    assert_eq!(outbox.job(op.id).unwrap().unwrap().status, "needs_review");
}
#[test]
fn negative_provider_balance_does_not_relax_transfer_amount_rules() {
    assert_eq!(provider_balance_cents("-12.50").unwrap(), -1250);
    assert_eq!(provider_balance_cents("0.00").unwrap(), 0);
    for invalid in ["--1", "-", "-0.001", "+12.50", " NaN"] {
        assert!(provider_balance_cents(invalid).is_err());
    }
    assert!(provider_cents("-12.50").is_err());
    assert!(decimal_amount(-1250).is_err());
}
