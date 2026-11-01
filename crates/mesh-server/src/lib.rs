//! Sandbox wallet service: signed RPC, local accounting, and provider orchestration.
//!
//! [`router`] exposes health, signed wallet RPC, and authenticated Dwolla notifications.
//! The binary configures a pinned rail, audits the store, and starts its worker.
//! [`store`] owns demo settlement and shared balanced posting; [`provider`] bridges
//! wallet reservations to the durable bank outbox; [`setup`] advances recoverable
//! customer/bank connection phases without a database lock across network calls.
//!
//! Provider HTTP acceptance is not a settlement receipt. The status-event consumer
//! applies authoritative observations and its cursor in the same accounting commit.
//! Consult `docs/API.md`, `docs/LEDGER.md`, and `docs/OPERATIONS.md` before changing
//! authorization, financial transitions, or worker recovery behavior.
pub mod bank_webhooks;
pub mod plaid;
pub mod provider;
pub mod setup;
pub mod store;

use axum::{
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use mesh_core::{seal_secret, Payment, Profile, SignedRequest};
use plaid::Plaid;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use store::Store;
use zeroize::Zeroizing;

#[derive(Debug)]
pub struct AppError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}
pub type AppResult<T> = Result<T, AppError>;
impl AppError {
    pub fn bad(code: &'static str, message: &str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message: message.into(),
        }
    }
    pub fn conflict(code: &'static str, message: &str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code,
            message: message.into(),
        }
    }
    pub fn forbidden(message: &str) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "forbidden",
            message: message.into(),
        }
    }
    pub fn internal(message: &str) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: message.into(),
        }
    }
    pub fn upstream(message: &str) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: "bank_unavailable",
            message: message.into(),
        }
    }
}
impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for AppError {}
impl From<mesh_core::Error> for AppError {
    fn from(e: mesh_core::Error) -> Self {
        Self::bad("invalid_payment", &e.to_string())
    }
}
impl From<serde_json::Error> for AppError {
    fn from(_: serde_json::Error) -> Self {
        Self::bad("invalid_payload", "This request has invalid fields")
    }
}
impl From<rusqlite::Error> for AppError {
    fn from(_: rusqlite::Error) -> Self {
        Self::internal("The ledger could not complete this request")
    }
}
impl From<mesh_banking::Error> for AppError {
    fn from(error: mesh_banking::Error) -> Self {
        match error {
            mesh_banking::Error::Conflict => {
                Self::conflict("id_conflict", "This bank request ID was already used")
            }
            mesh_banking::Error::Expired => Self::bad(
                "invalid_payment",
                "This payment expired before bank submission",
            ),
            mesh_banking::Error::Invalid(message) => Self::bad("invalid_bank_request", message),
            mesh_banking::Error::Store => Self::internal("Bank outbox unavailable"),
            _ => Self::upstream(
                "The bank provider could not complete this step; it will be retried safely",
            ),
        }
    }
}
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let message = if self.status == StatusCode::INTERNAL_SERVER_ERROR {
            "The ledger is unavailable; please try again"
        } else {
            &self.message
        };
        (
            self.status,
            Json(json!({"ok":false,"error":{"code":self.code,"message":message}})),
        )
            .into_response()
    }
}

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    pub vault_key: Arc<Zeroizing<[u8; 32]>>,
    pub plaid: Option<Plaid>,
    pub bank_webhooks: Option<Arc<bank_webhooks::BankWebhooks>>,
}
impl AppState {
    pub fn new(store: Store, vault_key: [u8; 32], plaid: Option<Plaid>) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            vault_key: Arc::new(Zeroizing::new(vault_key)),
            plaid,
            bank_webhooks: None,
        }
    }
    pub fn with_bank_webhooks(mut self, webhooks: bank_webhooks::BankWebhooks) -> Self {
        self.bank_webhooks = Some(Arc::new(webhooks));
        self
    }
    pub fn lock(&self) -> AppResult<std::sync::MutexGuard<'_, Store>> {
        self.store
            .lock()
            .map_err(|_| AppError::internal("Ledger lock unavailable"))
    }
}
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/health",
            get(|| async {
                Json(json!({"status":"ok","mode":"sandbox","version":1,"real_money_enabled":false}))
            }),
        )
        .route("/v1/rpc", post(rpc))
        .route("/v1/webhooks/dwolla", post(bank_webhooks::receive))
        .layer(DefaultBodyLimit::max(32_768))
        .with_state(state)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WithdrawalInput {
    id: String,
    amount_cents: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BankInput {
    id: uuid::Uuid,
    public_token: String,
    account_id: String,
}

async fn rpc(
    State(state): State<AppState>,
    Json(request): Json<SignedRequest>,
) -> AppResult<Json<Value>> {
    request.verify(mesh_core::now())?;
    let wallet = &request.body.public_key;
    {
        let mut store = state.lock()?;
        store.nonce(
            wallet,
            &request.body.nonce.to_string(),
            request.body.timestamp,
        )?;
        if request.body.action != "register" {
            store.exists(wallet)?;
        }
    }
    let payload = request.body.payload;
    let data = match request.body.action.as_str() {
        "register" => {
            let profile: Profile = serde_json::from_value(payload)?;
            if profile.body.public_key != *wallet {
                return Err(AppError::forbidden(
                    "This identity belongs to another wallet",
                ));
            }
            state.lock()?.register(&profile)?
        }
        "snapshot" => state.lock()?.snapshot(wallet, state.plaid.is_some())?,
        "demo_fund" => state.lock()?.fund_demo(wallet)?,
        "settle" => state
            .lock()?
            .settle(wallet, &serde_json::from_value::<Payment>(payload)?)?,
        "demo_link_bank" => {
            state.lock()?.require_demo()?;
            let encrypted =
                seal_secret(state.vault_key.as_ref(), b"{\"provider\":\"demo\"}", wallet)?;
            state
                .lock()?
                .save_bank(wallet, "Demo checking", "4242", &encrypted, "demo")?
        }
        "link_token" => {
            state.bank_setup_allowed(wallet)?;
            state
                .plaid
                .as_ref()
                .ok_or_else(|| {
                    AppError::bad(
                        "plaid_not_configured",
                        "Plaid Sandbox keys aren't configured on this server yet",
                    )
                })?
                .link_token(wallet)
                .await?
        }
        "link_bank" => {
            let input: BankInput = serde_json::from_value(payload)?;
            if state.lock()?.rail == provider::Rail::DwollaSandbox {
                state.queue_plaid_link(
                    wallet,
                    &setup::LinkInput {
                        id: input.id,
                        public_token: input.public_token,
                        account_id: input.account_id,
                    },
                )?
            } else {
                state.bank_setup_allowed(wallet)?;
                let bank = state
                    .plaid
                    .as_ref()
                    .ok_or_else(|| {
                        AppError::bad("plaid_not_configured", "Plaid Sandbox is not configured")
                    })?
                    .exchange(&input.public_token, &input.account_id)
                    .await?;
                let encrypted = seal_secret(
                    state.vault_key.as_ref(),
                    bank.credentials.as_bytes(),
                    wallet,
                )?;
                state.lock()?.save_bank(
                    wallet,
                    &bank.label,
                    &bank.mask,
                    &encrypted,
                    "plaid_sandbox",
                )?
            }
        }
        "bank_enroll" => state.enroll(wallet, serde_json::from_value(payload)?)?,
        "bank_fund" => state
            .lock()?
            .provider_fund(wallet, &serde_json::from_value(payload)?)?,
        "withdraw" => {
            let input: WithdrawalInput = serde_json::from_value(payload)?;
            state
                .lock()?
                .withdraw(wallet, &input.id, input.amount_cents)?
        }
        _ => return Err(AppError::bad("unknown_action", "Unknown request")),
    };
    Ok(Json(json!({"ok":true,"data":data})))
}
