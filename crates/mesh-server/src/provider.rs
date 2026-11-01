//! Bank-backed accounting. Reserving funds and queuing the corresponding transfer are one commit.
//!
//! Outbox status events drive credit, refunds, return debt, and review restrictions.
//! Applying an event commits accounting flags and the consumed cursor together.
//! Never infer success from a notification topic or clear a review block as a
//! substitute for establishing the transfer's financial disposition. See
//! `docs/LEDGER.md` for exact signs and `docs/PAYMENT_LIFECYCLE.md` for recovery.
use crate::{
    store::{post, Store},
    AppError, AppResult,
};
use mesh_banking::{outbox::Outbox, BankOperation, OperationKind};
use mesh_core::{now, Payment};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rail {
    Demo,
    DwollaSandbox,
}
impl Rail {
    pub fn label(self) -> &'static str {
        match self {
            Self::Demo => "demo",
            Self::DwollaSandbox => "dwolla_sandbox",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FundingInput {
    pub id: Uuid,
    pub amount_cents: i64,
    pub authorize_test_debit: bool,
}

impl Store {
    pub fn open_with_rail(path: &str, rail: Rail) -> AppResult<Self> {
        let mut store = Self::open_base(path)?;
        Outbox::initialize(&store.connection)?;
        store
            .connection
            .execute_batch(include_str!("provider.sql"))?;
        let tx = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved: Option<String> = tx
            .query_row(
                "SELECT value FROM metadata WHERE key='bank_rail'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(saved) = saved {
            if saved != rail.label() {
                return Err(AppError::internal(
                    "This database belongs to a different payment rail. Use a separate database.",
                ));
            }
        } else {
            let used: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM journal UNION ALL SELECT 1 FROM bank_jobs)",
                [],
                |r| r.get(0),
            )?;
            if used && rail != Rail::Demo {
                return Err(AppError::internal(
                    "Demo funds cannot be converted into bank-backed funds. Use a new database.",
                ));
            }
            tx.execute(
                "INSERT INTO metadata(key,value) VALUES ('bank_rail',?)",
                [rail.label()],
            )?;
        }
        tx.commit()?;
        store.rail = rail;
        Ok(store)
    }
    pub fn require_demo(&self) -> AppResult<()> {
        if self.rail != Rail::Demo {
            return Err(AppError::forbidden(
                "Demo funding is disabled on this bank-backed ledger",
            ));
        }
        Ok(())
    }
    pub fn require_provider(&self) -> AppResult<()> {
        if self.rail != Rail::DwollaSandbox {
            return Err(AppError::bad(
                "provider_required",
                "Connect this app to the Dwolla Sandbox server first",
            ));
        }
        Ok(())
    }
    pub fn provider_payment(&mut self, caller: &str, payment: &Payment) -> AppResult<Value> {
        self.require_provider()?;
        let b = &payment.body;
        if b.ledger_id != self.ledger_id {
            return Err(AppError::bad(
                "wrong_ledger",
                "This payment belongs to a different ledger",
            ));
        }
        if caller != b.sender && caller != b.recipient {
            return Err(AppError::forbidden(
                "Only the sender or recipient may submit this payment",
            ));
        }
        let encoded = serde_json::to_string(payment)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(receipt) = existing(
            &tx,
            b.id,
            &b.sender,
            OperationKind::PeerPayment,
            b.amount_cents,
            Some(&encoded),
        )? {
            return Ok(receipt);
        }
        payment.verify(now())?;
        let rejected: Option<String> = tx
            .query_row(
                "SELECT payment FROM rejected_payments WHERE id=?",
                [b.id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(old) = rejected {
            if old != encoded {
                return Err(id_conflict());
            }
            return Err(AppError::conflict(
                "insufficient_funds",
                "This payment was declined. Create a new payment to try again",
            ));
        }
        limit_pending(&tx, &b.sender)?;
        let source = ready_balance(&tx, &b.sender, true)?;
        let destination = ready_balance(&tx, &b.recipient, true)?;
        if balance(&tx, &b.sender)? < b.amount_cents {
            tx.execute(
                "INSERT INTO rejected_payments VALUES (?,?,'insufficient_funds',?)",
                params![b.id.to_string(), encoded, now()],
            )?;
            tx.commit()?;
            return Err(AppError::conflict(
                "insufficient_funds",
                "There isn't enough available money for this payment",
            ));
        }
        let operation = BankOperation {
            id: b.id,
            ledger_id: self.ledger_id,
            wallet: b.sender.clone(),
            kind: OperationKind::PeerPayment,
            source,
            destination,
            amount_cents: b.amount_cents,
            submit_before: Some(b.expires_at),
        };
        Outbox::enqueue_transaction(&tx, &operation, now())?;
        post(
            &tx,
            &format!("bank:{}:reserve", b.id),
            &[
                (&b.sender, -b.amount_cents),
                ("system:provider_hold", b.amount_cents),
            ],
        )?;
        tx.execute("INSERT INTO provider_requests(id,wallet,recipient,kind,amount_cents,payment,memo,authorization,created_at,updated_at) VALUES (?,?,?,'peer_payment',?,?,?,?,?,?)",params![b.id.to_string(),b.sender,b.recipient,b.amount_cents,encoded,b.memo,"signed_payment_v1",now(),now()])?;
        tx.commit()?;
        Ok(receipt(b.id, "queued"))
    }
    pub fn provider_fund(&mut self, wallet: &str, input: &FundingInput) -> AppResult<Value> {
        self.require_provider()?;
        if !input.authorize_test_debit {
            return Err(AppError::bad(
                "authorization_required",
                "Confirm this one-time test bank debit",
            ));
        }
        self.provider_bank_transfer(wallet, input.id, input.amount_cents, OperationKind::Funding)
    }
    pub fn provider_cashout(&mut self, wallet: &str, id: Uuid, amount: i64) -> AppResult<Value> {
        self.require_provider()?;
        self.provider_bank_transfer(wallet, id, amount, OperationKind::CashOut)
    }
    fn provider_bank_transfer(
        &mut self,
        wallet: &str,
        id: Uuid,
        amount: i64,
        kind: OperationKind,
    ) -> AppResult<Value> {
        mesh_core::validate_amount(amount)?;
        if id.is_nil() {
            return Err(AppError::bad("invalid_id", "Invalid transfer ID"));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(receipt) = existing(&tx, id, wallet, kind, amount, None)? {
            return Ok(receipt);
        }
        limit_pending(&tx, wallet)?;
        let own_balance = ready_balance(&tx, wallet, kind == OperationKind::CashOut)?;
        let (bank, label, mask): (String, String, String) = tx
            .query_row(
                "SELECT source,label,mask FROM provider_banks WHERE wallet=?",
                [wallet],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or_else(|| {
                AppError::bad("bank_required", "Connect a verified bank account first")
            })?;
        let bank = resource(&bank)?;
        let (source, destination) = if kind == OperationKind::Funding {
            (bank, own_balance)
        } else {
            (own_balance, bank)
        };
        if kind == OperationKind::CashOut && balance(&tx, wallet)? < amount {
            return Err(AppError::conflict(
                "insufficient_funds",
                "Your available balance is too low",
            ));
        }
        let operation = BankOperation {
            id,
            ledger_id: self.ledger_id,
            wallet: wallet.into(),
            kind,
            source,
            destination,
            amount_cents: amount,
            submit_before: None,
        };
        Outbox::enqueue_transaction(&tx, &operation, now())?;
        if kind == OperationKind::CashOut {
            post(
                &tx,
                &format!("bank:{id}:reserve"),
                &[(wallet, -amount), ("system:provider_hold", amount)],
            )?;
        }
        tx.execute("INSERT INTO provider_requests(id,wallet,kind,amount_cents,bank_label,bank_mask,authorization,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?)",params![id.to_string(),wallet,kind_label(kind),amount,label,mask,if kind==OperationKind::Funding {"one_time_sandbox_debit_v1"} else {"signed_cashout_v1"},now(),now()])?;
        tx.commit()?;
        Ok(receipt(id, "queued"))
    }
    /// Consume an ordered event log in the same transaction as the resulting accounting entries.
    /// Unknown jobs are an operational error, never a source of wallet credit.
    pub fn apply_bank_events(&mut self) -> AppResult<usize> {
        self.require_provider()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cursor: i64 = tx.query_row(
            "SELECT CAST(value AS INTEGER) FROM metadata WHERE key='bank_event_cursor'",
            [],
            |r| r.get(0),
        )?;
        let events:Vec<(i64,String,String,i64)>=tx.prepare("SELECT sequence,operation_id,status,created_at FROM bank_status_events WHERE sequence>? ORDER BY sequence LIMIT 500")?.query_map([cursor],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?.collect::<Result<_,_>>()?;
        for (sequence, id, status, at) in &events {
            let row=tx.query_row("SELECT wallet,recipient,kind,amount_cents,applied,reversed,payment,memo,status FROM provider_requests WHERE id=?",[id],|r|Ok(RequestRow {wallet:r.get(0)?,recipient:r.get(1)?,kind:r.get(2)?,amount:r.get(3)?,applied:r.get(4)?,reversed:r.get(5)?,payment:r.get(6)?,memo:r.get(7)?,status:r.get(8)?})).optional()?.ok_or_else(||AppError::internal("Unowned bank operation in wallet database"))?;
            let op: String =
                tx.query_row("SELECT operation FROM bank_jobs WHERE id=?", [id], |r| {
                    r.get(0)
                })?;
            let op: BankOperation = serde_json::from_str(&op)?;
            if op.ledger_id != self.ledger_id
                || op.id.to_string() != *id
                || op.wallet != row.wallet
                || op.amount_cents != row.amount
                || kind_label(op.kind) != row.kind
            {
                return Err(AppError::internal(
                    "Bank operation does not match its wallet reservation",
                ));
            }
            let terminal = matches!(
                row.status.as_str(),
                "failed" | "cancelled" | "returned" | "needs_review"
            );
            if terminal && row.status != *status {
                return Err(AppError::internal(
                    "Bank event regressed from a terminal state",
                ));
            }
            match status.as_str() {
                "processed" if !row.applied => {
                    if row.reversed {
                        return Err(AppError::internal("Cannot credit a reversed transfer"));
                    }
                    match op.kind {
                        OperationKind::Funding => credit(
                            &tx,
                            &row.wallet,
                            "system:provider_equity",
                            row.amount,
                            &format!("bank:{id}:processed"),
                        )?,
                        OperationKind::PeerPayment => {
                            let recipient = row
                                .recipient
                                .as_deref()
                                .ok_or_else(|| AppError::internal("Payment recipient missing"))?;
                            credit(
                                &tx,
                                recipient,
                                "system:provider_hold",
                                row.amount,
                                &format!("bank:{id}:processed"),
                            )?;
                            tx.execute("INSERT INTO payments(id,sender,recipient,amount_cents,memo,payment,settled_at) VALUES (?,?,?,?,?,?,?)",params![id,row.wallet,recipient,row.amount,row.memo,row.payment,at])?;
                        }
                        OperationKind::CashOut => post(
                            &tx,
                            &format!("bank:{id}:processed"),
                            &[
                                ("system:provider_hold", -row.amount),
                                ("system:provider_clearing", row.amount),
                            ],
                        )?,
                    }
                    tx.execute("UPDATE provider_requests SET applied=1 WHERE id=?", [id])?;
                }
                "failed" | "cancelled" if !row.applied && !row.reversed => {
                    if op.kind != OperationKind::Funding {
                        credit(
                            &tx,
                            &row.wallet,
                            "system:provider_hold",
                            row.amount,
                            &format!("bank:{id}:refund"),
                        )?;
                    }
                    tx.execute("UPDATE provider_requests SET reversed=1 WHERE id=?", [id])?;
                }
                "returned" if !row.reversed => {
                    if op.kind == OperationKind::Funding && row.applied {
                        let available = balance(&tx, &row.wallet)?.min(row.amount);
                        let shortfall = row.amount - available;
                        let mut entries = vec![("system:provider_equity", row.amount)];
                        if available > 0 {
                            entries.push((&row.wallet, -available));
                        }
                        if shortfall > 0 {
                            entries.push(("system:provider_loss", -shortfall));
                        }
                        post(&tx, &format!("bank:{id}:return"), &entries)?;
                        tx.execute("UPDATE provider_customers SET debt_cents=debt_cents+?,blocked=CASE WHEN blocked IN ('','debt') THEN CASE WHEN debt_cents+?>0 THEN 'debt' ELSE '' END ELSE blocked END WHERE wallet=?",params![shortfall,shortfall,row.wallet])?;
                    } else if op.kind == OperationKind::Funding {
                        // No credit was issued for this returned deposit.
                    } else {
                        // Returned peer/cash-out legs need provider balance reconciliation before any refund.
                        freeze(&tx, &row.wallet, "returned_transfer")?;
                        if let Some(recipient) = &row.recipient {
                            freeze(&tx, recipient, "returned_transfer")?;
                        }
                    }
                    tx.execute("UPDATE provider_requests SET reversed=1 WHERE id=?", [id])?;
                }
                "needs_review" => {
                    freeze(&tx, &row.wallet, "transfer_review")?;
                    if let Some(recipient) = &row.recipient {
                        freeze(&tx, recipient, "transfer_review")?;
                    }
                }
                "queued" | "submitting" | "submitted" | "processed" | "failed" | "cancelled"
                | "returned" => {}
                _ => return Err(AppError::internal("Unknown bank event")),
            }
            tx.execute(
                "UPDATE provider_requests SET status=?,updated_at=? WHERE id=?",
                params![status, at, id],
            )?;
            tx.execute(
                "UPDATE metadata SET value=? WHERE key='bank_event_cursor'",
                [sequence.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(events.len())
    }
    pub fn provider_snapshot(&self, wallet: &str, mut base: Value) -> AppResult<Value> {
        base["bank_rail"] = json!(self.rail.label());
        let banking:Option<Value>=self.connection.query_row("SELECT status,blocked,debt_cents FROM provider_customers WHERE wallet=?",[wallet],|r|Ok(json!({"status":r.get::<_,String>(0)?,"blocked":r.get::<_,String>(1)?,"debt_cents":r.get::<_,i64>(2)?}))).optional()?;
        base["banking"] =
            banking.unwrap_or(json!({"status":"not_started","blocked":"","debt_cents":0}));
        base["bank"]=self.connection.query_row("SELECT label,mask FROM provider_banks WHERE wallet=?",[wallet],|r|Ok(json!({"label":r.get::<_,String>(0)?,"mask":r.get::<_,String>(1)?,"mode":"dwolla_sandbox"}))).optional()?.unwrap_or(Value::Null);
        let setup: Option<String> = self
            .connection
            .query_row(
                "SELECT phase FROM provider_setup WHERE wallet=? AND kind='bank'",
                [wallet],
                |r| r.get(0),
            )
            .optional()?;
        base["banking"]["bank_status"] = json!(setup.unwrap_or_else(|| "not_started".into()));
        let rows:Vec<Value>=self.connection.prepare("SELECT r.id,r.wallet,r.recipient,r.amount_cents,r.memo,r.created_at,s.name,d.name,r.status FROM provider_requests r JOIN accounts s ON s.id=r.wallet JOIN accounts d ON d.id=r.recipient WHERE r.kind='peer_payment' AND (r.wallet=? OR r.recipient=?) ORDER BY r.created_at DESC,r.rowid DESC LIMIT 200")?.query_map(params![wallet,wallet],|r|{ let status:String=r.get(8)?; Ok(json!({"id":r.get::<_,String>(0)?,"sender":r.get::<_,String>(1)?,"recipient":r.get::<_,String>(2)?,"amount_cents":r.get::<_,i64>(3)?,"memo":r.get::<_,String>(4)?,"settled_at":r.get::<_,i64>(5)?,"sender_name":r.get::<_,String>(6)?,"recipient_name":r.get::<_,String>(7)?,"status":if status=="processed" {"settled"} else {&status}})) })?.collect::<Result<_,_>>()?;
        base["payments"] = json!(rows);
        let rows:Vec<Value>=self.connection.prepare("SELECT id,amount_cents,status,bank_label,bank_mask,created_at,kind FROM provider_requests WHERE wallet=? AND kind<>'peer_payment' ORDER BY created_at DESC,rowid DESC LIMIT 200")?.query_map([wallet],|r| {let status:String=r.get(2)?;Ok(json!({"id":r.get::<_,String>(0)?,"amount_cents":r.get::<_,i64>(1)?,"status":if status=="processed" {"settled"} else {&status},"bank_label":r.get::<_,String>(3)?,"bank_mask":r.get::<_,String>(4)?,"created_at":r.get::<_,i64>(5)?,"kind":r.get::<_,String>(6)?}))})?.collect::<Result<_,_>>()?;
        base["withdrawals"] = json!(rows
            .iter()
            .filter(|r| r["kind"] == "cash_out")
            .collect::<Vec<_>>());
        base["fundings"] = json!(rows
            .iter()
            .filter(|r| r["kind"] == "funding")
            .collect::<Vec<_>>());
        // Active reservations are bounded independently of the visible activity page.
        let reserved: Vec<String> = self
            .connection
            .prepare("SELECT id FROM provider_requests WHERE wallet=? AND kind='peer_payment' AND status IN ('queued','submitting','submitted','needs_review')")?
            .query_map([wallet], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        base["reserved_payment_ids"] = json!(reserved);
        Ok(base)
    }
}
struct RequestRow {
    wallet: String,
    recipient: Option<String>,
    kind: String,
    amount: i64,
    applied: bool,
    reversed: bool,
    payment: Option<String>,
    memo: String,
    status: String,
}
fn existing(
    tx: &Transaction<'_>,
    id: Uuid,
    wallet: &str,
    kind: OperationKind,
    amount: i64,
    payment: Option<&str>,
) -> AppResult<Option<Value>> {
    let row: Option<(String, String, i64, Option<String>, String)> = tx
        .query_row(
            "SELECT wallet,kind,amount_cents,payment,status FROM provider_requests WHERE id=?",
            [id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    row.map(|(owner, k, cents, old, status)| {
        if owner != wallet || k != kind_label(kind) || cents != amount || old.as_deref() != payment
        {
            return Err(id_conflict());
        }
        Ok(receipt(
            id,
            if status == "processed" {
                "settled"
            } else {
                &status
            },
        ))
    })
    .transpose()
}
fn kind_label(kind: OperationKind) -> &'static str {
    match kind {
        OperationKind::Funding => "funding",
        OperationKind::PeerPayment => "peer_payment",
        OperationKind::CashOut => "cash_out",
    }
}
fn receipt(id: Uuid, status: &str) -> Value {
    json!({"id":id,"status":status,"test_money":true})
}
fn id_conflict() -> AppError {
    AppError::conflict("id_conflict", "This transfer ID was already used")
}
pub(crate) fn resource(id: &str) -> AppResult<Uuid> {
    Uuid::parse_str(id)
        .ok()
        .filter(|id| !id.is_nil())
        .ok_or_else(|| AppError::internal("Invalid stored provider resource"))
}
fn balance(tx: &Transaction<'_>, wallet: &str) -> AppResult<i64> {
    Ok(tx.query_row(
        "SELECT balance_cents FROM accounts WHERE id=?",
        [wallet],
        |r| r.get(0),
    )?)
}
fn ready_balance(tx: &Transaction<'_>, wallet: &str, spending: bool) -> AppResult<Uuid> {
    let row:Option<(Option<String>,String,String,i64)>=tx.query_row("SELECT balance_source,status,blocked,debt_cents FROM provider_customers WHERE wallet=?",[wallet],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let (source, status, blocked, debt) = row.ok_or_else(|| {
        AppError::bad(
            "verification_required",
            "Both friends need to finish wallet verification",
        )
    })?;
    if status != "verified" || source.is_none() {
        return Err(AppError::bad(
            "verification_required",
            "Wallet verification is still pending",
        ));
    }
    if (!blocked.is_empty() && (spending || blocked != "debt")) || (spending && debt > 0) {
        return Err(AppError::conflict(
            "wallet_review",
            "This wallet needs a bank review before moving money",
        ));
    }
    resource(source.as_deref().unwrap_or_default())
}
fn freeze(tx: &Transaction<'_>, wallet: &str, reason: &str) -> AppResult<()> {
    tx.execute(
        "UPDATE provider_customers SET blocked=? WHERE wallet=?",
        params![reason, wallet],
    )?;
    Ok(())
}
/// A new credit first restores any money owed after an ACH return.
fn credit(
    tx: &Transaction<'_>,
    wallet: &str,
    source: &str,
    amount: i64,
    id: &str,
) -> AppResult<()> {
    let debt: i64 = tx.query_row(
        "SELECT debt_cents FROM provider_customers WHERE wallet=?",
        [wallet],
        |r| r.get(0),
    )?;
    let repaid = amount.min(debt);
    let mut entries = vec![(source, -amount)];
    if repaid > 0 {
        entries.push(("system:provider_loss", repaid));
    }
    if amount > repaid {
        entries.push((wallet, amount - repaid));
    }
    post(tx, id, &entries)?;
    tx.execute("UPDATE provider_customers SET debt_cents=debt_cents-?,blocked=CASE WHEN blocked='debt' AND debt_cents-?=0 THEN '' ELSE blocked END WHERE wallet=?",params![repaid,repaid,wallet])?;
    Ok(())
}

/// Recheck current wallet restrictions immediately before any new provider submission.
/// Known transfers are always observed, including when the wallet is frozen.
pub struct WalletRail<'a> {
    pub state: &'a crate::AppState,
    pub provider: &'a mesh_banking::dwolla::Dwolla,
}
impl mesh_banking::PaymentRail for WalletRail<'_> {
    async fn submit(&self, op: &BankOperation) -> mesh_banking::Result<Uuid> {
        let bank_check = {
            let store = self.state.lock().map_err(|_| mesh_banking::Error::Store)?;
            let account:Option<(String,String,String,String)>=store.connection.query_row("SELECT customer_id,balance_source,status,blocked FROM provider_customers WHERE wallet=?",[&op.wallet],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            let (customer, own, status, blocked) = account.ok_or(mesh_banking::Error::Protocol)?;
            if status != "verified"
                || (!blocked.is_empty()
                    && !(op.kind == OperationKind::Funding && blocked == "debt"))
            {
                return Err(mesh_banking::Error::Rejected);
            }
            let own = Uuid::parse_str(&own).map_err(|_| mesh_banking::Error::Protocol)?;
            match op.kind {
                OperationKind::PeerPayment => {
                    let destination:Option<String>=store.connection.query_row("SELECT c.balance_source FROM provider_requests r JOIN provider_customers c ON c.wallet=r.recipient WHERE r.id=? AND c.status='verified' AND c.blocked='' AND c.debt_cents=0",[op.id.to_string()],|r|r.get(0)).optional()?;
                    if destination.as_deref() != Some(op.destination.to_string().as_str())
                        || op.source != own
                    {
                        return Err(mesh_banking::Error::Rejected);
                    }
                    None
                }
                OperationKind::Funding | OperationKind::CashOut => {
                    let bank: String = store.connection.query_row(
                        "SELECT source FROM provider_banks WHERE wallet=?",
                        [&op.wallet],
                        |r| r.get(0),
                    )?;
                    let bank = Uuid::parse_str(&bank).map_err(|_| mesh_banking::Error::Protocol)?;
                    let matches = if op.kind == OperationKind::Funding {
                        op.source == bank && op.destination == own
                    } else {
                        op.source == own && op.destination == bank
                    };
                    if !matches {
                        return Err(mesh_banking::Error::Protocol);
                    }
                    Some((
                        Uuid::parse_str(&customer).map_err(|_| mesh_banking::Error::Protocol)?,
                        bank,
                    ))
                }
            }
        };
        if let Some((customer, bank)) = bank_check {
            if !self.provider.verified_bank(customer, bank).await? {
                return Err(mesh_banking::Error::Rejected);
            }
        }
        self.provider.submit(op).await
    }
    async fn observe(&self, id: Uuid) -> mesh_banking::Result<mesh_banking::TransferObservation> {
        self.provider.observe(id).await
    }
}

fn limit_pending(tx: &Transaction<'_>, wallet: &str) -> AppResult<()> {
    let active:i64=tx.query_row("SELECT COUNT(*) FROM provider_requests WHERE wallet=? AND status IN ('queued','submitting','submitted','needs_review')",[wallet],|r|r.get(0))?;
    if active >= 128 {
        return Err(AppError::conflict(
            "too_many_pending",
            "Wait for your pending bank transfers before starting another",
        ));
    }
    Ok(())
}
