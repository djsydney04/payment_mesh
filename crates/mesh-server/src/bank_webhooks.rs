use crate::{AppError, AppResult, AppState};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use mesh_banking::{outbox::Outbox, webhook};
use std::sync::Mutex;
use zeroize::Zeroizing;

pub struct BankWebhooks {
    outbox: Mutex<Outbox>,
    secret: Zeroizing<Vec<u8>>,
}
impl BankWebhooks {
    pub fn open(path: &str, secret: Vec<u8>) -> AppResult<Self> {
        if secret.len() < 16 || secret.len() > 1024 {
            return Err(AppError::internal(
                "Set a 16–1024 byte Dwolla webhook secret",
            ));
        }
        let outbox =
            Outbox::open(path).map_err(|_| AppError::internal("Could not open the bank outbox"))?;
        Ok(Self {
            outbox: Mutex::new(outbox),
            secret: Zeroizing::new(secret),
        })
    }
}

pub async fn receive(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<StatusCode> {
    let Some(inbox) = state.bank_webhooks else {
        return Err(AppError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "bank_webhooks_not_configured",
            message: "Bank notifications aren't configured".into(),
        });
    };
    let event = headers
        .get("X-Request-Signature-SHA-256")
        .and_then(|h| h.to_str().ok())
        .and_then(|sig| webhook::verify(&inbox.secret, sig, &body).ok())
        .ok_or_else(|| AppError {
            status: StatusCode::UNAUTHORIZED,
            code: "invalid_bank_webhook",
            message: "Bank notification could not be authenticated".into(),
        })?;
    // Persist and acknowledge promptly; the worker obtains the authoritative provider state.
    let mut outbox = inbox
        .outbox
        .lock()
        .map_err(|_| AppError::internal("Bank notification queue is unavailable"))?;
    match outbox.receive_webhook(&event, mesh_core::now()) {
        Ok(_) => Ok(StatusCode::OK),
        Err(mesh_banking::Error::Conflict) => Err(AppError::conflict(
            "webhook_conflict",
            "Bank notification ID conflicts with an earlier event",
        )),
        Err(_) => Err(AppError::internal("Bank notification could not be saved")),
    }
}
