//! Bank provider transport and durable transfer orchestration, separate from the wallet ledger.
//! No operation in this crate can contact a production banking endpoint.
//!
//! [`dwolla`] supplies the fixed-origin HTTP adapter, [`outbox`] persists transfer
//! intent before side effects and polls durable provider IDs, and [`webhook`]
//! authenticates notifications before they can wake reconciliation. The wallet
//! service separately owns reservations, recipient credits, debt, and review states.
//!
//! A successful provider submission is not proof of processing. A processed transfer
//! can later return. Stable operation IDs, fenced leases, bounded resubmission, and
//! checked observations address different failure windows; none should be replaced
//! by retrying with a new financial ID. The standalone acceptance runner does not
//! apply wallet accounting. See `docs/BANK_PROVIDER.md` and `docs/LEDGER.md`.
pub mod dwolla;
pub mod outbox;
pub mod webhook;

use serde::{Deserialize, Serialize};
use std::future::Future;
use uuid::Uuid;

pub const DWOLLA_SANDBOX: &str = "https://api-sandbox.dwolla.com";
pub const SAFE_RESUBMIT_SECONDS: i64 = 23 * 60 * 60;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Bank connection is temporarily unavailable")]
    Retryable,
    #[error("The bank provider declined this request")]
    Rejected,
    #[error("Bank provider credentials need attention")]
    Authentication,
    #[error("Bank provider returned an unexpected response")]
    Protocol,
    #[error("Invalid bank operation: {0}")]
    Invalid(&'static str),
    #[error("Bank operation ID conflicts with an existing operation")]
    Conflict,
    #[error("This worker no longer owns the operation")]
    LostLease,
    #[error("The payment instruction expired before bank submission")]
    Expired,
    #[error("Bank outbox could not complete the operation")]
    Store,
}
pub type Result<T> = std::result::Result<T, Error>;
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::Store
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::Protocol
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Funding,
    PeerPayment,
    CashOut,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BankOperation {
    /// Persist this before submitting. It is also the provider idempotency/correlation ID.
    pub id: Uuid,
    pub ledger_id: Uuid,
    pub wallet: String,
    pub kind: OperationKind,
    pub source: Uuid,
    pub destination: Uuid,
    pub amount_cents: i64,
    #[serde(default)]
    pub submit_before: Option<i64>,
}
impl BankOperation {
    pub fn validate(&self) -> Result<()> {
        if self.id.is_nil()
            || self.ledger_id.is_nil()
            || self.source.is_nil()
            || self.destination.is_nil()
            || self.source == self.destination
        {
            return Err(Error::Invalid("Invalid resource identifier"));
        }
        if self.submit_before.is_some_and(|deadline| deadline <= 0) {
            return Err(Error::Invalid("Invalid submission deadline"));
        }
        mesh_core::decode32(&self.wallet).map_err(|_| Error::Invalid("Invalid wallet"))?;
        mesh_core::validate_amount(self.amount_cents).map_err(|_| Error::Invalid("Invalid amount"))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    Pending,
    Processed,
    Failed,
    Cancelled,
    Returned,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferObservation {
    pub id: Uuid,
    pub source: Uuid,
    pub destination: Uuid,
    pub amount_cents: i64,
    pub status: TransferStatus,
    pub correlation_id: Uuid,
    /// Other provider transfer legs observed as part of the same funds movement.
    #[serde(default)]
    pub related_transfers: Vec<Uuid>,
}
impl TransferObservation {
    pub fn matches(&self, operation: &BankOperation) -> bool {
        self.correlation_id == operation.id
            && self.source == operation.source
            && self.destination == operation.destination
            && self.amount_cents == operation.amount_cents
    }
}

pub trait PaymentRail: Send + Sync {
    fn submit(&self, operation: &BankOperation) -> impl Future<Output = Result<Uuid>> + Send;
    fn observe(&self, transfer: Uuid) -> impl Future<Output = Result<TransferObservation>> + Send;
}

/// Provider amounts are decimal strings. Parsing must never round floating point dollars.
pub fn provider_cents(value: &str) -> Result<i64> {
    if value.is_empty() || value.len() > 20 || value.trim() != value {
        return Err(Error::Protocol);
    }
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 2
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || value.ends_with('.')
    {
        return Err(Error::Protocol);
    }
    let whole: i64 = whole.parse().map_err(|_| Error::Protocol)?;
    let fraction: i64 = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<i64>().map_err(|_| Error::Protocol)?
            * if fraction.len() == 1 { 10 } else { 1 }
    };
    whole
        .checked_mul(100)
        .and_then(|n| n.checked_add(fraction))
        .ok_or(Error::Protocol)
}

pub fn decimal_amount(cents: i64) -> Result<String> {
    mesh_core::validate_amount(cents).map_err(|_| Error::Invalid("Invalid amount"))?;
    Ok(format!("{}.{:02}", cents / 100, cents % 100))
}

/// Dwolla balances can be negative after an ACH return; transfer amounts cannot.
pub fn provider_balance_cents(value: &str) -> Result<i64> {
    if let Some(positive) = value.strip_prefix('-') {
        provider_cents(positive)?
            .checked_neg()
            .ok_or(Error::Protocol)
    } else {
        provider_cents(value)
    }
}
