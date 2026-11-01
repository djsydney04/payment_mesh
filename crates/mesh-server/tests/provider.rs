use mesh_banking::{
    outbox::Outbox, BankOperation, PaymentRail, TransferObservation, TransferStatus,
};
use mesh_core::{now, Identity, Payment, Profile, SignedRequest};
use mesh_server::{
    provider::{FundingInput, Rail},
    store::Store,
    AppState,
};
use rusqlite::params;
use serde_json::{json, Value};
use std::sync::Mutex;
use uuid::Uuid;

struct FakeRail {
    operations: Mutex<Vec<BankOperation>>,
    status: Mutex<TransferStatus>,
}
impl Default for FakeRail {
    fn default() -> Self {
        Self {
            operations: Mutex::new(vec![]),
            status: Mutex::new(TransferStatus::Processed),
        }
    }
}
impl PaymentRail for FakeRail {
    async fn submit(&self, op: &BankOperation) -> mesh_banking::Result<Uuid> {
        let mut ops = self.operations.lock().unwrap();
        if !ops.iter().any(|o| o.id == op.id) {
            ops.push(op.clone());
        }
        Ok(op.id)
    }
    async fn observe(&self, id: Uuid) -> mesh_banking::Result<TransferObservation> {
        let ops = self.operations.lock().unwrap();
        let o = ops.iter().find(|o| o.id == id).unwrap();
        Ok(TransferObservation {
            id,
            source: o.source,
            destination: o.destination,
            amount_cents: o.amount_cents,
            status: *self.status.lock().unwrap(),
            correlation_id: o.id,
            related_transfers: vec![],
        })
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    path: String,
    store: Store,
    rail: FakeRail,
    a: Identity,
    b: Identity,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("wallet.sqlite")
            .to_str()
            .unwrap()
            .to_owned();
        let mut store = Store::open_with_rail(&path, Rail::DwollaSandbox).unwrap();
        let a = Identity::from_seed(&"01".repeat(32)).unwrap();
        let b = Identity::from_seed(&"02".repeat(32)).unwrap();
        for (who, name) in [(&a, "Alice"), (&b, "Bob")] {
            store
                .register(&Profile::new(who, name, now()).unwrap())
                .unwrap();
            store.connection.execute("INSERT INTO provider_customers(wallet,customer_id,balance_source,status) VALUES (?,?,?,'verified')",params![who.public_key(),Uuid::new_v4().to_string(),Uuid::new_v4().to_string()]).unwrap();
            store.connection.execute("INSERT INTO provider_banks(wallet,source,label,mask) VALUES (?,?,'Sandbox checking','1234')",params![who.public_key(),Uuid::new_v4().to_string()]).unwrap();
        }
        Self {
            _dir: dir,
            path,
            store,
            rail: FakeRail::default(),
            a,
            b,
        }
    }
    fn fund(&mut self, amount: i64) -> Uuid {
        let id = Uuid::new_v4();
        self.store
            .provider_fund(
                &self.a.public_key(),
                &FundingInput {
                    id,
                    amount_cents: amount,
                    authorize_test_debit: true,
                },
            )
            .unwrap();
        id
    }
    fn payment(&self, amount: i64) -> Payment {
        Payment::new(
            &self.a,
            &self.b.public_key(),
            amount,
            "Coffee",
            self.store.ledger_id,
            now(),
        )
        .unwrap()
    }
    fn balance(&self, who: &Identity) -> i64 {
        self.store.snapshot(&who.public_key(), false).unwrap()["balance_cents"]
            .as_i64()
            .unwrap()
    }
    async fn work(&mut self) {
        let mut outbox = Outbox::open(&self.path).unwrap();
        outbox.run_once(&self.rail).await.unwrap();
        self.store.apply_bank_events().unwrap();
        assert!(self.store.audit().unwrap());
    }
    fn due(&self, id: Uuid) {
        self.store
            .connection
            .execute(
                "UPDATE bank_jobs SET next_attempt_at=0 WHERE id=?",
                [id.to_string()],
            )
            .unwrap();
    }
}
#[tokio::test]
async fn bank_funding_is_not_available_until_provider_processed_and_events_are_idempotent() {
    let mut f = Fixture::new();
    let id = f.fund(10000);
    assert_eq!(f.balance(&f.a), 0);
    *f.rail.status.lock().unwrap() = TransferStatus::Pending;
    f.work().await;
    assert_eq!(f.balance(&f.a), 0);
    assert_eq!(
        f.store.snapshot(&f.a.public_key(), false).unwrap()["fundings"][0]["status"],
        "submitted"
    );
    *f.rail.status.lock().unwrap() = TransferStatus::Processed;
    f.due(id);
    f.work().await;
    assert_eq!(f.balance(&f.a), 10000);
    assert_eq!(f.store.apply_bank_events().unwrap(), 0);
    f.due(id);
    f.work().await;
    assert_eq!(f.balance(&f.a), 10000);
    assert_eq!(f.rail.operations.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn payment_reserves_once_and_recipient_can_retry_after_restart() {
    let mut f = Fixture::new();
    f.fund(10000);
    f.work().await;
    let p = f.payment(1250);
    let first = f.store.settle(&f.b.public_key(), &p).unwrap();
    assert_eq!(first["status"], "queued");
    assert_eq!(f.balance(&f.a), 8750);
    assert_eq!(f.balance(&f.b), 0);
    f.store = Store::open_with_rail(&f.path, Rail::DwollaSandbox).unwrap();
    assert_eq!(f.store.settle(&f.a.public_key(), &p).unwrap(), first);
    assert_eq!(f.balance(&f.a), 8750);
    let snap = f.store.snapshot(&f.a.public_key(), false).unwrap();
    assert_eq!(snap["payments"][0]["status"], "queued");
    assert_eq!(snap["reserved_payment_ids"][0], p.body.id.to_string());
    f.work().await;
    assert_eq!(f.balance(&f.b), 1250);
    assert_eq!(
        f.store.settle(&f.b.public_key(), &p).unwrap()["status"],
        "settled"
    );
    assert!(f.store.audit().unwrap());
}
#[tokio::test]
async fn atomic_reservation_rolls_back_when_outbox_or_request_insert_fails() {
    let mut f = Fixture::new();
    f.fund(10000);
    f.work().await;
    let p = f.payment(2500);
    f.store.connection.execute_batch("CREATE TRIGGER injected_failure BEFORE INSERT ON provider_requests BEGIN SELECT RAISE(ABORT,'simulated crash'); END;").unwrap();
    assert!(f.store.settle(&f.a.public_key(), &p).is_err());
    assert_eq!(f.balance(&f.a), 10000);
    assert!(Outbox::open(&f.path)
        .unwrap()
        .job(p.body.id)
        .unwrap()
        .is_none());
    assert!(f.store.audit().unwrap());
}
#[tokio::test]
async fn failed_cashout_refunds_once_and_cannot_be_rebound_to_another_amount_or_owner() {
    let mut f = Fixture::new();
    f.fund(5000);
    f.work().await;
    let id = Uuid::new_v4();
    f.store
        .provider_cashout(&f.a.public_key(), id, 2000)
        .unwrap();
    assert_eq!(f.balance(&f.a), 3000);
    assert!(f
        .store
        .provider_cashout(&f.a.public_key(), id, 2100)
        .is_err());
    assert!(f
        .store
        .provider_cashout(&f.b.public_key(), id, 2000)
        .is_err());
    *f.rail.status.lock().unwrap() = TransferStatus::Failed;
    f.work().await;
    assert_eq!(f.balance(&f.a), 5000);
    f.store.apply_bank_events().unwrap();
    assert_eq!(f.balance(&f.a), 5000);
}
#[tokio::test]
async fn returned_deposit_removes_unspent_funds_records_spent_shortfall_and_new_funding_repays_debt(
) {
    let mut f = Fixture::new();
    let deposit = f.fund(10000);
    f.work().await;
    let p = f.payment(8000);
    f.store.settle(&f.a.public_key(), &p).unwrap();
    f.work().await;
    *f.rail.status.lock().unwrap() = TransferStatus::Failed;
    f.due(deposit);
    f.work().await;
    assert_eq!(f.balance(&f.a), 0);
    assert_eq!(f.balance(&f.b), 8000);
    let s = f.store.snapshot(&f.a.public_key(), false).unwrap();
    assert_eq!(s["banking"]["debt_cents"], 8000);
    assert_eq!(s["banking"]["blocked"], "debt");
    assert!(f.store.settle(&f.a.public_key(), &f.payment(1)).is_err());
    let next = f.fund(9000);
    *f.rail.status.lock().unwrap() = TransferStatus::Processed;
    f.work().await;
    assert_eq!(f.balance(&f.a), 1000);
    let s = f.store.snapshot(&f.a.public_key(), false).unwrap();
    assert_eq!(s["banking"]["debt_cents"], 0);
    assert_eq!(s["banking"]["blocked"], "");
    f.due(next);
    f.work().await;
    assert_eq!(f.balance(&f.a), 1000);
}
#[tokio::test]
async fn observation_mismatch_keeps_reservation_and_freezes_wallets() {
    let mut f = Fixture::new();
    f.fund(1000);
    f.work().await;
    let p = f.payment(500);
    f.store.settle(&f.a.public_key(), &p).unwrap();
    struct WrongRail;
    impl PaymentRail for WrongRail {
        async fn submit(&self, o: &BankOperation) -> mesh_banking::Result<Uuid> {
            Ok(o.id)
        }
        async fn observe(&self, id: Uuid) -> mesh_banking::Result<TransferObservation> {
            Ok(TransferObservation {
                id,
                source: Uuid::new_v4(),
                destination: Uuid::new_v4(),
                amount_cents: 500,
                status: TransferStatus::Processed,
                correlation_id: id,
                related_transfers: vec![],
            })
        }
    }
    Outbox::open(&f.path)
        .unwrap()
        .run_once(&WrongRail)
        .await
        .unwrap();
    f.store.apply_bank_events().unwrap();
    assert_eq!(f.balance(&f.a), 500);
    assert_eq!(f.balance(&f.b), 0);
    assert_eq!(
        f.store.snapshot(&f.a.public_key(), false).unwrap()["banking"]["blocked"],
        "transfer_review"
    );
    assert!(f.store.audit().unwrap());
}
#[test]
fn payment_rail_is_pinned_and_demo_funds_never_enter_provider_ledger() {
    let f = Fixture::new();
    assert!(Store::open(&f.path).is_err());
    assert!(f.store.require_demo().is_err());
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("demo.sqlite");
    let path = path.to_str().unwrap();
    let mut demo = Store::open(path).unwrap();
    demo.register(&Profile::new(&f.a, "A", now()).unwrap())
        .unwrap();
    demo.fund_demo(&f.a.public_key()).unwrap();
    drop(demo);
    assert!(Store::open_with_rail(path, Rail::DwollaSandbox).is_err());
}
#[tokio::test]
async fn signed_api_blocks_demo_funding_and_records_explicit_bank_authorization() {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let f = Fixture::new();
    let identity = f.a;
    let state = AppState::new(f.store, [3; 32], None);
    for (action, payload, expected) in [
        ("demo_fund", json!({}), 403),
        (
            "bank_fund",
            json!({"id":Uuid::new_v4(),"amount_cents":500,"authorize_test_debit":false}),
            400,
        ),
        (
            "bank_fund",
            json!({"id":Uuid::new_v4(),"amount_cents":500,"authorize_test_debit":true}),
            200,
        ),
    ] {
        let signed = SignedRequest::new(&identity, action, payload, now()).unwrap();
        let response = mesh_server::router(state.clone())
            .oneshot(
                Request::post("/v1/rpc")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&signed).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(!value.to_string().contains("provider_transfer"));
    }
    assert_eq!(
        state
            .lock()
            .unwrap()
            .snapshot(&identity.public_key(), false)
            .unwrap()["balance_cents"],
        0
    );
}

#[tokio::test]
async fn provider_decline_stays_terminal_after_later_funding() {
    let mut f = Fixture::new();
    let p = f.payment(500);
    assert_eq!(
        f.store.settle(&f.a.public_key(), &p).unwrap_err().code,
        "insufficient_funds"
    );
    f.fund(1000);
    f.work().await;
    assert_eq!(
        f.store.settle(&f.b.public_key(), &p).unwrap_err().code,
        "insufficient_funds"
    );
    assert_eq!(f.balance(&f.a), 1000);
    assert_eq!(f.balance(&f.b), 0);
}
#[tokio::test]
async fn applying_a_provider_event_rolls_back_accounting_and_cursor_together() {
    let mut f = Fixture::new();
    f.fund(1000);
    f.work().await;
    let p = f.payment(500);
    f.store.settle(&f.a.public_key(), &p).unwrap();
    Outbox::open(&f.path)
        .unwrap()
        .run_once(&f.rail)
        .await
        .unwrap();
    let cursor: String = f
        .store
        .connection
        .query_row(
            "SELECT value FROM metadata WHERE key='bank_event_cursor'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    f.store.connection.execute_batch("CREATE TRIGGER fail_credit BEFORE UPDATE OF balance_cents ON accounts WHEN NEW.name='Bob' BEGIN SELECT RAISE(ABORT,'test crash'); END;").unwrap();
    assert!(f.store.apply_bank_events().is_err());
    assert_eq!(f.balance(&f.b), 0);
    let after: String = f
        .store
        .connection
        .query_row(
            "SELECT value FROM metadata WHERE key='bank_event_cursor'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cursor, after);
    assert!(f.store.audit().unwrap());
    f.store
        .connection
        .execute_batch("DROP TRIGGER fail_credit")
        .unwrap();
    f.store.apply_bank_events().unwrap();
    assert_eq!(f.balance(&f.b), 500);
    assert!(f.store.audit().unwrap());
}
#[tokio::test]
async fn expired_unsubmitted_payment_releases_reservation_exactly_once() {
    let mut f = Fixture::new();
    f.fund(1000);
    f.work().await;
    let p = f.payment(500);
    f.store.settle(&f.a.public_key(), &p).unwrap();
    Outbox::open(&f.path)
        .unwrap()
        .run_once_with_clock(&f.rail, || p.body.expires_at)
        .await
        .unwrap();
    f.store.apply_bank_events().unwrap();
    assert_eq!(f.balance(&f.a), 1000);
    assert_eq!(f.balance(&f.b), 0);
    assert_eq!(
        f.store.settle(&f.b.public_key(), &p).unwrap()["status"],
        "cancelled"
    );
    assert!(f.store.audit().unwrap());
}
#[tokio::test]
async fn returned_cashout_stays_under_review_without_an_unproven_refund() {
    let mut f = Fixture::new();
    f.fund(1000);
    f.work().await;
    let id = Uuid::new_v4();
    f.store
        .provider_cashout(&f.a.public_key(), id, 500)
        .unwrap();
    f.work().await;
    *f.rail.status.lock().unwrap() = TransferStatus::Failed;
    f.due(id);
    f.work().await;
    assert_eq!(f.balance(&f.a), 500);
    assert_eq!(
        f.store.snapshot(&f.a.public_key(), false).unwrap()["banking"]["blocked"],
        "returned_transfer"
    );
    assert!(f.store.audit().unwrap());
}
#[tokio::test]
async fn independent_database_connections_cannot_reserve_the_same_money_twice() {
    let mut f = Fixture::new();
    f.fund(1000);
    f.work().await;
    let p = f.payment(750);
    let q = f.payment(750);
    let path = f.path.clone();
    let a = f.a.public_key();
    let b = f.b.public_key();
    let first = std::thread::spawn(move || {
        let mut s = Store::open_with_rail(&path, Rail::DwollaSandbox).unwrap();
        s.settle(&a, &p).is_ok()
    });
    let path = f.path.clone();
    let second = std::thread::spawn(move || {
        let mut s = Store::open_with_rail(&path, Rail::DwollaSandbox).unwrap();
        s.settle(&b, &q).is_ok()
    });
    assert_eq!(
        usize::from(first.join().unwrap()) + usize::from(second.join().unwrap()),
        1
    );
    assert_eq!(f.balance(&f.a), 250);
    assert_eq!(f.balance(&f.b), 0);
    assert!(f.store.audit().unwrap());
}
