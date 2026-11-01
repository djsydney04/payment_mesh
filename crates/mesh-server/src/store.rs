//! SQLite wallet ledger and the local demo rail.
//!
//! Amounts are signed integer cents in journal entries, with every posting summing
//! to zero. User accounts must remain nonnegative; system accounts carry offsets.
//! A receipt is durable only with its accounting transaction. Provider reservations
//! and event application extend [`Store`] in the `provider` module.
//!
//! Do not call external banking APIs inside a posting transaction. Schema definitions
//! live in `schema.sql` and `provider.sql`; worked entries are in `docs/LEDGER.md`.
use crate::{AppError, AppResult};
use mesh_core::{now, Payment, Profile};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::{json, Value};

/// One SQLite connection and its persistent ledger/rail identity.
///
/// Public connection access is for internal integration and tests, not an operator
/// balance-adjustment API. The application serializes store use through its mutex.
pub struct Store {
    pub connection: Connection,
    pub ledger_id: uuid::Uuid,
    pub rail: crate::provider::Rail,
}
impl Store {
    /// Open the demo rail, rejecting a database already pinned to another rail.
    pub fn open(path: &str) -> AppResult<Self> {
        Self::open_with_rail(path, crate::provider::Rail::Demo)
    }
    pub(crate) fn open_base(path: &str) -> AppResult<Self> {
        let connection = Connection::open(path)?;
        connection.execute_batch(include_str!("schema.sql"))?;
        connection.execute(
            "INSERT OR IGNORE INTO metadata(key,value) VALUES ('ledger_id',?)",
            [uuid::Uuid::new_v4().to_string()],
        )?;
        let id: String = connection.query_row(
            "SELECT value FROM metadata WHERE key='ledger_id'",
            [],
            |r| r.get(0),
        )?;
        let ledger_id = uuid::Uuid::parse_str(&id)
            .map_err(|_| AppError::internal("Invalid ledger identity"))?;
        Ok(Self {
            connection,
            ledger_id,
            rail: crate::provider::Rail::Demo,
        })
    }
    pub fn nonce(&mut self, wallet: &str, nonce: &str, timestamp: i64) -> AppResult<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM nonces WHERE timestamp < ?", [now() - 240])?;
        let changed = tx.execute(
            "INSERT OR IGNORE INTO nonces(wallet,nonce,timestamp) VALUES (?,?,?)",
            params![wallet, nonce, timestamp],
        )?;
        if changed == 0 {
            return Err(AppError::conflict(
                "replayed_request",
                "This request was already used",
            ));
        }
        tx.commit()?;
        Ok(())
    }
    pub fn register(&mut self, profile: &Profile) -> AppResult<Value> {
        profile.verify(now())?;
        self.connection.execute("INSERT INTO accounts(id,name,profile) VALUES (?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,profile=excluded.profile", params![profile.body.public_key, profile.body.name, serde_json::to_string(profile)?])?;
        Ok(json!({"registered":true,"mode":"sandbox"}))
    }
    pub fn exists(&self, wallet: &str) -> AppResult<()> {
        let exists: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=? AND system=0)",
            [wallet],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(AppError::bad("not_registered", "Set up your wallet first"));
        }
        Ok(())
    }
    pub fn fund_demo(&mut self, wallet: &str) -> AppResult<Value> {
        self.require_demo()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let funded: i64 = tx.query_row(
            "SELECT demo_funded FROM accounts WHERE id=?",
            [wallet],
            |r| r.get(0),
        )?;
        if funded == 0 {
            post(
                &tx,
                &format!("faucet:{wallet}"),
                &[(wallet, 12540), ("system:demo_equity", -12540)],
            )?;
            tx.execute("UPDATE accounts SET demo_funded=1 WHERE id=?", [wallet])?;
        }
        tx.commit()?;
        Ok(json!({"funded":true,"test_money":true}))
    }
    pub fn settle(&mut self, wallet: &str, payment: &Payment) -> AppResult<Value> {
        if self.rail == crate::provider::Rail::DwollaSandbox {
            return self.provider_payment(wallet, payment);
        }
        let b = &payment.body;
        if b.ledger_id != self.ledger_id {
            return Err(AppError::bad(
                "wrong_ledger",
                "This payment belongs to a different ledger",
            ));
        }
        if wallet != b.sender && wallet != b.recipient {
            return Err(AppError::forbidden(
                "Only the sender or recipient may submit this payment",
            ));
        }
        // Return an existing receipt even after expiry. The entire immutable instruction must match.
        let encoded = serde_json::to_string(payment)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<(String, i64)> = tx
            .query_row(
                "SELECT payment,settled_at FROM payments WHERE id=?",
                [b.id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((old, at)) = existing {
            if old != encoded {
                return Err(AppError::conflict(
                    "id_conflict",
                    "Payment ID was already used",
                ));
            }
            return Ok(json!({"id":b.id,"status":"settled","settled_at":at}));
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
                return Err(AppError::conflict(
                    "id_conflict",
                    "Payment ID was already used",
                ));
            }
            return Err(AppError::conflict(
                "insufficient_funds",
                "This payment was declined. Start a new payment to try again",
            ));
        }
        let recipient_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=? AND system=0)",
            [&b.recipient],
            |r| r.get(0),
        )?;
        if !recipient_exists {
            return Err(AppError::bad(
                "recipient_not_registered",
                "Your friend needs to connect once to set up their wallet",
            ));
        }
        let balance: i64 = tx
            .query_row(
                "SELECT balance_cents FROM accounts WHERE id=?",
                [&b.sender],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| {
                AppError::bad("sender_not_registered", "The sender needs to finish setup")
            })?;
        if balance < b.amount_cents {
            tx.execute("INSERT INTO rejected_payments(id,payment,reason,created_at) VALUES (?,?,'insufficient_funds',?)", params![b.id.to_string(),encoded,now()])?;
            tx.commit()?;
            return Err(AppError::conflict(
                "insufficient_funds",
                "There isn't enough available money for this payment",
            ));
        }
        post(
            &tx,
            &format!("payment:{}", b.id),
            &[(&b.sender, -b.amount_cents), (&b.recipient, b.amount_cents)],
        )?;
        let at = now();
        tx.execute("INSERT INTO payments(id,sender,recipient,amount_cents,memo,payment,settled_at) VALUES (?,?,?,?,?,?,?)", params![b.id.to_string(),b.sender,b.recipient,b.amount_cents,b.memo,encoded,at])?;
        tx.commit()?;
        Ok(json!({"id":b.id,"status":"settled","settled_at":at}))
    }
    pub fn save_bank(
        &mut self,
        wallet: &str,
        label: &str,
        mask: &str,
        credentials: &str,
        mode: &str,
    ) -> AppResult<Value> {
        self.require_demo()?;
        self.connection.execute("INSERT INTO banks(wallet,label,mask,encrypted_credentials,mode) VALUES (?,?,?,?,?) ON CONFLICT(wallet) DO UPDATE SET label=excluded.label,mask=excluded.mask,encrypted_credentials=excluded.encrypted_credentials,mode=excluded.mode", params![wallet,label,mask,credentials,mode])?;
        Ok(json!({"linked":true,"label":label,"mask":mask,"mode":mode}))
    }
    pub fn withdraw(&mut self, wallet: &str, id: &str, amount: i64) -> AppResult<Value> {
        mesh_core::validate_amount(amount)?;
        let id = uuid::Uuid::parse_str(id)
            .map_err(|_| AppError::bad("invalid_id", "Invalid withdrawal ID"))?;
        if id.is_nil() {
            return Err(AppError::bad("invalid_id", "Invalid withdrawal ID"));
        }
        if self.rail == crate::provider::Rail::DwollaSandbox {
            return self.provider_cashout(wallet, id, amount);
        }
        let id = id.to_string();
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old: Option<(String, i64, String)> = tx
            .query_row(
                "SELECT wallet,amount_cents,status FROM withdrawals WHERE id=?",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((owner, cents, status)) = old {
            if owner != wallet || cents != amount {
                return Err(AppError::conflict(
                    "id_conflict",
                    "Withdrawal ID was already used",
                ));
            }
            return Ok(json!({"id":id,"status":status,"test_money":true}));
        }
        let bank: Option<(String, String, String)> = tx
            .query_row(
                "SELECT label,mask,encrypted_credentials FROM banks WHERE wallet=?",
                [wallet],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (label, mask, credentials) =
            bank.ok_or_else(|| AppError::bad("bank_required", "Connect a bank account first"))?;
        let balance: i64 = tx.query_row(
            "SELECT balance_cents FROM accounts WHERE id=?",
            [wallet],
            |r| r.get(0),
        )?;
        if balance < amount {
            return Err(AppError::conflict(
                "insufficient_funds",
                "Your available balance is too low",
            ));
        }
        post(
            &tx,
            &format!("withdrawal:{id}:reserve"),
            &[(wallet, -amount), ("system:payout_reserve", amount)],
        )?;
        tx.execute("INSERT INTO withdrawals(id,wallet,amount_cents,status,bank_label,bank_mask,bank_credentials,created_at,updated_at) VALUES (?,?,?,'queued',?,?,?,?,?)", params![id,wallet,amount,label,mask,credentials,now(),now()])?;
        tx.commit()?;
        Ok(json!({"id":id,"status":"queued","test_money":true}))
    }
    /// Demo adapter is deterministic and atomic: process after restart without double debit.
    /// A live adapter must instead persist provider idempotency keys and reconcile webhooks.
    pub fn process_demo_payouts(&mut self, at: i64) -> AppResult<usize> {
        self.require_demo()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let pending: Vec<(String, i64)> = tx
            .prepare(
                "SELECT id,amount_cents FROM withdrawals WHERE status='queued' AND created_at<=?",
            )?
            .query_map([at - 3], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        for (id, cents) in &pending {
            post(
                &tx,
                &format!("withdrawal:{id}:settle"),
                &[
                    ("system:payout_reserve", -*cents),
                    ("system:bank_clearing", *cents),
                ],
            )?;
            tx.execute("UPDATE withdrawals SET status='settled',updated_at=?,provider_reference=? WHERE id=? AND status='queued'", params![at,format!("demo_{id}"),id])?;
        }
        tx.commit()?;
        Ok(pending.len())
    }
    pub fn snapshot(&self, wallet: &str, plaid_configured: bool) -> AppResult<Value> {
        let (name, balance): (String, i64) = self.connection.query_row(
            "SELECT name,balance_cents FROM accounts WHERE id=?",
            [wallet],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut statement = self.connection.prepare("SELECT p.id,p.sender,p.recipient,p.amount_cents,p.memo,p.settled_at,s.name,r.name FROM payments p JOIN accounts s ON s.id=p.sender JOIN accounts r ON r.id=p.recipient WHERE p.sender=? OR p.recipient=? ORDER BY p.settled_at DESC,p.rowid DESC LIMIT 200")?;
        let payments: Vec<Value> = statement.query_map(params![wallet,wallet], |r| Ok(json!({"id":r.get::<_,String>(0)?,"sender":r.get::<_,String>(1)?,"recipient":r.get::<_,String>(2)?,"amount_cents":r.get::<_,i64>(3)?,"memo":r.get::<_,String>(4)?,"settled_at":r.get::<_,i64>(5)?,"sender_name":r.get::<_,String>(6)?,"recipient_name":r.get::<_,String>(7)?,"status":"settled"})))?.collect::<std::result::Result<_,_>>()?;
        let bank: Option<Value> = self.connection.query_row("SELECT label,mask,mode FROM banks WHERE wallet=?", [wallet], |r| Ok(json!({"label":r.get::<_,String>(0)?,"mask":r.get::<_,String>(1)?,"mode":r.get::<_,String>(2)?}))).optional()?;
        let withdrawals: Vec<Value> = self.connection.prepare("SELECT id,amount_cents,status,bank_label,bank_mask,created_at FROM withdrawals WHERE wallet=? ORDER BY created_at DESC LIMIT 200")?.query_map([wallet], |r| Ok(json!({"id":r.get::<_,String>(0)?,"amount_cents":r.get::<_,i64>(1)?,"status":r.get::<_,String>(2)?,"bank_label":r.get::<_,String>(3)?,"bank_mask":r.get::<_,String>(4)?,"created_at":r.get::<_,i64>(5)?})))?.collect::<std::result::Result<_,_>>()?;
        let snapshot = json!({"ledger_id":self.ledger_id,"public_key":wallet,"name":name,"balance_cents":balance,"currency":"USD","mode":"sandbox","plaid_configured":plaid_configured,"bank":bank,"payments":payments,"withdrawals":withdrawals,"bank_rail":self.rail.label()});
        if self.rail == crate::provider::Rail::DwollaSandbox {
            self.provider_snapshot(wallet, snapshot)
        } else {
            Ok(snapshot)
        }
    }
    /// Check conservation, balanced journals, and materialized balances against entries.
    ///
    /// This checks local consistency, not external provider reserves or authorization.
    pub fn audit(&self) -> AppResult<bool> {
        let total: i64 = self.connection.query_row(
            "SELECT COALESCE(SUM(balance_cents),0) FROM accounts",
            [],
            |r| r.get(0),
        )?;
        let unbalanced: i64 = self.connection.query_row("SELECT COUNT(*) FROM (SELECT journal_id FROM entries GROUP BY journal_id HAVING SUM(amount_cents)<>0)", [], |r| r.get(0))?;
        let drift: i64 = self.connection.query_row("SELECT COUNT(*) FROM accounts a WHERE balance_cents <> (SELECT COALESCE(SUM(amount_cents),0) FROM entries e WHERE e.account_id=a.id)", [], |r| r.get(0))?;
        Ok(total == 0 && unbalanced == 0 && drift == 0)
    }
}

pub(crate) fn post(tx: &Transaction<'_>, id: &str, entries: &[(&str, i64)]) -> AppResult<()> {
    let total = entries
        .iter()
        .try_fold(0i64, |sum, (_, n)| sum.checked_add(*n))
        .ok_or_else(|| AppError::internal("Ledger amount overflow"))?;
    if entries.len() < 2 || total != 0 {
        return Err(AppError::internal("Unbalanced ledger entry"));
    }
    tx.execute(
        "INSERT INTO journal(id,created_at) VALUES (?,?)",
        params![id, now()],
    )?;
    for (wallet, amount) in entries {
        if tx.execute(
            "UPDATE accounts SET balance_cents=balance_cents+? WHERE id=?",
            params![amount, wallet],
        )? != 1
        {
            return Err(AppError::internal("Ledger account missing"));
        }
        tx.execute(
            "INSERT INTO entries(journal_id,account_id,amount_cents) VALUES (?,?,?)",
            params![id, wallet, amount],
        )?;
    }
    Ok(())
}
