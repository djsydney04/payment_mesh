use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use mesh_core::*;
use mesh_server::{router, store::Store, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

fn wallet(n: u8) -> Identity {
    Identity::from_seed(&hex::encode([n; 32])).unwrap()
}
fn setup() -> (AppState, Identity, Identity, Identity) {
    let mut store = Store::open(":memory:").unwrap();
    let a = wallet(1);
    let b = wallet(2);
    let c = wallet(3);
    for (identity, name) in [(&a, "Alice"), (&b, "Bob"), (&c, "Charlie")] {
        store
            .register(&Profile::new(identity, name, now()).unwrap())
            .unwrap();
    }
    (AppState::new(store, [8; 32], None), a, b, c)
}
async fn send_request(app: Router, request: &SignedRequest) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::post("/v1/rpc")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}
async fn call(
    state: &AppState,
    identity: &Identity,
    action: &str,
    payload: Value,
) -> (StatusCode, Value) {
    send_request(
        router(state.clone()),
        &SignedRequest::new(identity, action, payload, now()).unwrap(),
    )
    .await
}

#[tokio::test]
async fn recipient_can_settle_and_retries_do_not_move_money_twice() {
    let (state, a, b, _) = setup();
    state.lock().unwrap().fund_demo(&a.public_key()).unwrap();
    let payment = Payment::new(
        &a,
        &b.public_key(),
        1250,
        "Coffee",
        state.lock().unwrap().ledger_id,
        now(),
    )
    .unwrap();
    let (status, receipt) = call(
        &state,
        &b,
        "settle",
        serde_json::to_value(&payment).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, second) = call(
        &state,
        &a,
        "settle",
        serde_json::to_value(&payment).unwrap(),
    )
    .await;
    assert_eq!(receipt, second);
    let store = state.lock().unwrap();
    assert_eq!(
        store.snapshot(&a.public_key(), false).unwrap()["balance_cents"],
        11290
    );
    assert_eq!(
        store.snapshot(&b.public_key(), false).unwrap()["balance_cents"],
        1250
    );
    assert!(store.audit().unwrap());
}

#[tokio::test]
async fn payment_cannot_replay_on_a_different_ledger() {
    let (first, a, b, _) = setup();
    let (second, _, _, _) = setup();
    first.lock().unwrap().fund_demo(&a.public_key()).unwrap();
    second.lock().unwrap().fund_demo(&a.public_key()).unwrap();
    let payment = Payment::new(
        &a,
        &b.public_key(),
        500,
        "",
        first.lock().unwrap().ledger_id,
        now(),
    )
    .unwrap();
    let (status, result) = call(
        &second,
        &a,
        "settle",
        serde_json::to_value(&payment).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(result["error"]["code"], "wrong_ledger");
    assert_eq!(
        second
            .lock()
            .unwrap()
            .snapshot(&a.public_key(), false)
            .unwrap()["balance_cents"],
        12540
    );
    assert_eq!(
        call(&first, &a, "settle", serde_json::to_value(payment).unwrap())
            .await
            .0,
        StatusCode::OK
    );
}
#[tokio::test]
async fn concurrent_double_spend_has_one_winner_and_no_negative_balance() {
    let (state, a, b, c) = setup();
    state.lock().unwrap().fund_demo(&a.public_key()).unwrap();
    let p = Payment::new(
        &a,
        &b.public_key(),
        10000,
        "",
        state.lock().unwrap().ledger_id,
        now(),
    )
    .unwrap();
    let q = Payment::new(
        &a,
        &c.public_key(),
        10000,
        "",
        state.lock().unwrap().ledger_id,
        now(),
    )
    .unwrap();
    let (r1, r2) = tokio::join!(
        call(&state, &a, "settle", serde_json::to_value(p).unwrap()),
        call(&state, &a, "settle", serde_json::to_value(q).unwrap())
    );
    assert_eq!(
        [r1.0, r2.0]
            .iter()
            .filter(|&&s| s == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        [r1.0, r2.0]
            .iter()
            .filter(|&&s| s == StatusCode::CONFLICT)
            .count(),
        1
    );
    let store = state.lock().unwrap();
    assert_eq!(
        store.snapshot(&a.public_key(), false).unwrap()["balance_cents"],
        2540
    );
    assert!(store.audit().unwrap());
}
#[tokio::test]
async fn declined_payment_cannot_be_revived_when_sender_is_funded_later() {
    let (state, a, b, _) = setup();
    let payment = Payment::new(
        &a,
        &b.public_key(),
        1000,
        "",
        state.lock().unwrap().ledger_id,
        now(),
    )
    .unwrap();
    assert_eq!(
        call(
            &state,
            &b,
            "settle",
            serde_json::to_value(&payment).unwrap()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    state.lock().unwrap().fund_demo(&a.public_key()).unwrap();
    assert_eq!(
        call(
            &state,
            &b,
            "settle",
            serde_json::to_value(&payment).unwrap()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        state
            .lock()
            .unwrap()
            .snapshot(&a.public_key(), false)
            .unwrap()["balance_cents"],
        12540
    );
}
#[tokio::test]
async fn unrelated_wallet_cannot_submit_or_rename_someone_else() {
    let (state, a, b, c) = setup();
    let payment = Payment::new(
        &a,
        &b.public_key(),
        100,
        "",
        state.lock().unwrap().ledger_id,
        now(),
    )
    .unwrap();
    assert_eq!(
        call(&state, &c, "settle", serde_json::to_value(payment).unwrap())
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let profile = Profile::new(&a, "Hijacked", now()).unwrap();
    assert_eq!(
        call(
            &state,
            &c,
            "register",
            serde_json::to_value(profile).unwrap()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        state
            .lock()
            .unwrap()
            .snapshot(&a.public_key(), false)
            .unwrap()["name"],
        "Alice"
    );
}
#[tokio::test]
async fn rpc_replay_expiry_and_signature_tampering_are_rejected() {
    let (state, a, _, _) = setup();
    let request = SignedRequest::new(&a, "snapshot", json!({}), now()).unwrap();
    assert_eq!(
        send_request(router(state.clone()), &request).await.0,
        StatusCode::OK
    );
    let (status, result) = send_request(router(state.clone()), &request).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(result["error"]["code"], "replayed_request");
    let mut modified = request;
    modified.body.action = "demo_fund".into();
    assert_eq!(
        send_request(router(state.clone()), &modified).await.0,
        StatusCode::BAD_REQUEST
    );
    let old = SignedRequest::new(&a, "snapshot", json!({}), now() - 121).unwrap();
    assert_eq!(
        send_request(router(state), &old).await.0,
        StatusCode::BAD_REQUEST
    );
}
#[tokio::test]
async fn withdrawal_is_reserved_once_and_settles_once() {
    let (state, a, _, _) = setup();
    {
        let mut store = state.lock().unwrap();
        store.fund_demo(&a.public_key()).unwrap();
        store
            .save_bank(&a.public_key(), "Test bank", "1234", "encrypted", "demo")
            .unwrap();
    }
    let id = uuid::Uuid::new_v4();
    let payload = json!({"id":id,"amount_cents":5000});
    assert_eq!(
        call(&state, &a, "withdraw", payload.clone()).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(&state, &a, "withdraw", payload).await.0,
        StatusCode::OK
    );
    let mut store = state.lock().unwrap();
    assert_eq!(
        store.snapshot(&a.public_key(), false).unwrap()["balance_cents"],
        7540
    );
    assert_eq!(store.process_demo_payouts(now() + 5).unwrap(), 1);
    assert_eq!(store.process_demo_payouts(now() + 6).unwrap(), 0);
    assert_eq!(
        store.snapshot(&a.public_key(), false).unwrap()["withdrawals"][0]["status"],
        "settled"
    );
    assert!(store.audit().unwrap());
}
#[tokio::test]
async fn cashout_cannot_use_money_already_sent_or_another_wallets_id() {
    let (state, a, b, _) = setup();
    {
        let mut store = state.lock().unwrap();
        store.fund_demo(&a.public_key()).unwrap();
        store
            .save_bank(&a.public_key(), "Bank", "1234", "encrypted", "demo")
            .unwrap();
        let payment = Payment::new(&a, &b.public_key(), 10000, "", store.ledger_id, now()).unwrap();
        store.settle(&a.public_key(), &payment).unwrap();
    }
    let id = uuid::Uuid::new_v4();
    assert_eq!(
        call(&state, &a, "withdraw", json!({"id":id,"amount_cents":5000}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&state, &a, "withdraw", json!({"id":id,"amount_cents":2000}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&state, &b, "withdraw", json!({"id":id,"amount_cents":2000}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert!(state.lock().unwrap().audit().unwrap());
}
#[tokio::test]
async fn bank_credentials_never_appear_in_wallet_snapshot() {
    let (state, a, _, _) = setup();
    assert_eq!(
        call(&state, &a, "demo_link_bank", json!({})).await.0,
        StatusCode::OK
    );
    let (_, snapshot) = call(&state, &a, "snapshot", json!({})).await;
    assert_eq!(snapshot["data"]["bank"]["mask"], "4242");
    assert!(!snapshot.to_string().contains("encrypted_credentials"));
    assert!(!snapshot.to_string().contains("access_token"));
    let token: String = state
        .lock()
        .unwrap()
        .connection
        .query_row(
            "SELECT encrypted_credentials FROM banks WHERE wallet=?",
            [a.public_key()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!token.contains("provider"));
    assert!(open_secret(&[8; 32], &token, &a.public_key()).is_ok());
    assert_eq!(
        call(&state, &a, "link_token", json!({})).await.1["error"]["code"],
        "plaid_not_configured"
    );
}
#[test]
fn database_restart_preserves_balances_replays_and_payout_jobs() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("ledger.sqlite");
    let path = file.to_str().unwrap();
    let a = wallet(1);
    let b = wallet(2);
    let payment = Payment::new(
        &a,
        &b.public_key(),
        1000,
        "",
        Store::open(path).unwrap().ledger_id,
        now(),
    )
    .unwrap();
    let withdrawal_id = uuid::Uuid::new_v4().to_string();
    {
        let mut store = Store::open(path).unwrap();
        for (who, name) in [(&a, "Alice"), (&b, "Bob")] {
            store
                .register(&Profile::new(who, name, now()).unwrap())
                .unwrap();
        }
        store.fund_demo(&a.public_key()).unwrap();
        store.settle(&a.public_key(), &payment).unwrap();
        store.nonce(&a.public_key(), "used-nonce", now()).unwrap();
        store
            .save_bank(&a.public_key(), "Bank", "1111", "encrypted", "demo")
            .unwrap();
        store
            .withdraw(&a.public_key(), &withdrawal_id, 2000)
            .unwrap();
    }
    let mut store = Store::open(path).unwrap();
    store.fund_demo(&a.public_key()).unwrap();
    store.settle(&b.public_key(), &payment).unwrap();
    assert!(store.nonce(&a.public_key(), "used-nonce", now()).is_err());
    assert_eq!(store.process_demo_payouts(now() + 5).unwrap(), 1);
    assert_eq!(
        store.snapshot(&a.public_key(), false).unwrap()["balance_cents"],
        9540
    );
    assert!(store.audit().unwrap());
}
#[test]
fn transaction_rolls_back_all_entries_when_one_leg_fails() {
    let (state, a, b, _) = setup();
    let mut store = state.lock().unwrap();
    store.fund_demo(&a.public_key()).unwrap();
    store.connection.execute_batch("CREATE TRIGGER fail_recipient BEFORE UPDATE OF balance_cents ON accounts WHEN NEW.name='Bob' BEGIN SELECT RAISE(ABORT,'test crash'); END;").unwrap();
    let payment = Payment::new(&a, &b.public_key(), 1000, "", store.ledger_id, now()).unwrap();
    assert!(store.settle(&a.public_key(), &payment).is_err());
    assert_eq!(
        store.snapshot(&a.public_key(), false).unwrap()["balance_cents"],
        12540
    );
    assert!(store.audit().unwrap());
}
#[tokio::test]
async fn schema_rejects_unsigned_oversize_and_unknown_fields() {
    let (state, a, _, _) = setup();
    let response = router(state.clone())
        .oneshot(
            Request::post("/v1/rpc")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let request = SignedRequest::new(
        &a,
        "snapshot",
        json!({"oversized":"x".repeat(40000)}),
        now(),
    )
    .unwrap();
    let response = router(state)
        .oneshot(
            Request::post("/v1/rpc")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
