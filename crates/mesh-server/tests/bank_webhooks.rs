use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use hmac::{Hmac, Mac};
use mesh_banking::outbox::Outbox;
use mesh_server::{bank_webhooks::BankWebhooks, router, store::Store, AppState};
use serde_json::json;
use sha2::Sha256;
use tower::ServiceExt;

const SECRET: &[u8] = b"test-webhook-secret-not-a-real-secret";
fn request(body: &[u8], signature: Option<&str>) -> Request<Body> {
    let mut request =
        Request::post("/v1/webhooks/dwolla").header("content-type", "application/json");
    if let Some(sig) = signature {
        request = request.header("X-Request-Signature-SHA-256", sig);
    }
    request.body(Body::from(body.to_vec())).unwrap()
}
fn sign(body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET).unwrap();
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}
#[tokio::test]
async fn webhook_route_authenticates_raw_bytes_persists_once_and_rejects_forgery() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bank.sqlite");
    let path = path.to_str().unwrap();
    let state = AppState::new(Store::open(":memory:").unwrap(), [1; 32], None)
        .with_bank_webhooks(BankWebhooks::open(path, SECRET.to_vec()).unwrap());
    let body = serde_json::to_vec(&json!({"id":uuid::Uuid::new_v4(),"topic":"customer_transfer_completed","_links":{"resource":{"href":format!("https://api-sandbox.dwolla.com/transfers/{}",uuid::Uuid::new_v4())}}})).unwrap();
    let signature = sign(&body);
    for _ in 0..2 {
        assert_eq!(
            router(state.clone())
                .oneshot(request(&body, Some(&signature)))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        Outbox::open(path)
            .unwrap()
            .connection
            .query_row("SELECT COUNT(*) FROM bank_webhooks", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let mut tampered = body.clone();
    tampered.push(b' ');
    assert_eq!(
        router(state.clone())
            .oneshot(request(&tampered, Some(&signature)))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        router(state.clone())
            .oneshot(request(&body, None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(state.lock().unwrap().audit().unwrap());
}
#[tokio::test]
async fn unconfigured_or_oversized_webhooks_do_not_enter_the_inbox() {
    let state = AppState::new(Store::open(":memory:").unwrap(), [1; 32], None);
    assert_eq!(
        router(state.clone())
            .oneshot(request(b"{}", None))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let body = vec![b' '; 40_000];
    assert_eq!(
        router(state)
            .oneshot(request(&body, Some(&sign(&body))))
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}
