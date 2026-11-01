//! Recoverable Sandbox customer and bank setup. No database lock is held across a network call.
//!
//! Jobs persist encrypted inputs and stable provider keys before mutations, then
//! checkpoint resource IDs between phases. Completed/expired bank-link intent IDs
//! remain immutable receipts even if a new sign-in replaces the active setup job.
//! A consumed one-time Plaid token whose response was never saved can require a
//! fresh sign-in; that is distinct from retrying an ambiguous money movement.
//! This module also compares eligible provider balances without auto-crediting drift.
use crate::{
    plaid::LinkedBank,
    provider::{resource, Rail},
    AppError, AppResult, AppState,
};
use mesh_banking::{
    dwolla::{Dwolla, PersonalCustomer},
    SAFE_RESUBMIT_SECONDS,
};
use mesh_core::{now, open_secret, seal_secret};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentInput {
    pub id: Uuid,
    pub profile: PersonalCustomer,
    pub synthetic_test_data: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkInput {
    pub id: Uuid,
    pub public_token: String,
    pub account_id: String,
}
impl AppState {
    pub fn enroll(&self, wallet: &str, input: EnrollmentInput) -> AppResult<Value> {
        if !input.synthetic_test_data || input.id.is_nil() {
            return Err(AppError::bad(
                "test_identity_required",
                "Use a synthetic identity for this Sandbox verification",
            ));
        }
        input.profile.validate()?;
        let body = Zeroizing::new(serde_json::to_string(&input.profile)?);
        let encrypted = seal_secret(
            self.vault_key.as_ref(),
            body.as_bytes(),
            &format!("{wallet}:enroll"),
        )?;
        let mut store = self.lock()?;
        store.require_provider()?;
        let tx = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old: Option<(String, String)> = tx
            .query_row(
                "SELECT id,phase FROM provider_setup WHERE wallet=? AND kind='enroll'",
                [wallet],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, phase)) = old {
            if id != input.id.to_string() {
                return Err(AppError::conflict(
                    "setup_exists",
                    "Wallet verification has already been requested",
                ));
            }
            return Ok(json!({"id":id,"status":phase}));
        }
        tx.execute(
            "INSERT INTO provider_customers(wallet) VALUES (?)",
            [wallet],
        )?;
        tx.execute("INSERT INTO provider_setup(id,wallet,kind,phase,encrypted_payload,next_key,next_attempt,created_at) VALUES (?,?,'enroll','create_customer',?,?,?,?)",params![input.id.to_string(),wallet,encrypted,Uuid::new_v4().to_string(),now(),now()])?;
        tx.commit()?;
        Ok(json!({"id":input.id,"status":"pending"}))
    }
    /// Acknowledge only after the one-time public token is encrypted and durably queued.
    /// Repeating the same signed intent returns its original receipt without calling Plaid again.
    pub fn queue_plaid_link(&self, wallet: &str, input: &LinkInput) -> AppResult<Value> {
        crate::plaid::validate_public_token(&input.public_token)?;
        crate::plaid::validate_account_id(&input.account_id)?;
        if input.id.is_nil() {
            return Err(AppError::bad("invalid_id", "Invalid bank connection ID"));
        }
        let fingerprint = hex::encode(Sha256::digest(serde_json::to_vec(&(
            wallet,
            &input.public_token,
            &input.account_id,
        ))?));
        let mut store = self.lock()?;
        store.require_provider()?;
        let tx = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old: Option<(String, String, String)> = tx
            .query_row(
                "SELECT wallet,fingerprint,status FROM provider_link_requests WHERE id=?",
                [input.id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((owner, old, status)) = old {
            if owner != wallet || old != fingerprint {
                return Err(AppError::conflict(
                    "id_conflict",
                    "This bank connection ID was already used",
                ));
            }
            return Ok(json!({"id":input.id,"status":status}));
        }
        let customer:Option<String>=tx.query_row("SELECT customer_id FROM provider_customers WHERE wallet=? AND status='verified' AND blocked=''",[wallet],|r|r.get(0)).optional()?.flatten();
        let customer = customer.ok_or_else(|| {
            AppError::bad(
                "verification_required",
                "Verify your wallet before connecting a bank",
            )
        })?;
        let previous: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT phase,resource_id FROM provider_setup WHERE wallet=? AND kind='bank'",
                [wallet],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((phase, resource)) = previous {
            if phase != "relink_required" || resource.is_some() {
                return Err(AppError::conflict(
                    "setup_exists",
                    "A bank connection is already pending or linked",
                ));
            }
            tx.execute(
                "DELETE FROM provider_setup WHERE wallet=? AND kind='bank'",
                [wallet],
            )?;
        }
        let body=Zeroizing::new(json!({"customer":customer,"public_token":input.public_token,"account_id":input.account_id}).to_string());
        let encrypted = seal_secret(
            self.vault_key.as_ref(),
            body.as_bytes(),
            &format!("{wallet}:bank"),
        )?;
        let setup = Uuid::new_v4();
        tx.execute("INSERT INTO provider_setup(id,wallet,kind,phase,encrypted_payload,next_key,next_attempt,created_at) VALUES (?,?,'bank','plaid_exchange',?,?,?,?)",params![setup.to_string(),wallet,encrypted,Uuid::new_v4().to_string(),now(),now()])?;
        tx.execute("INSERT INTO provider_link_requests(id,wallet,fingerprint,setup_id,status,created_at,updated_at) VALUES (?,?,?,?,'plaid_exchange',?,?)",params![input.id.to_string(),wallet,fingerprint,setup.to_string(),now(),now()])?;
        tx.commit()?;
        Ok(json!({"id":input.id,"status":"plaid_exchange"}))
    }
    /// Called only with a server-validated Plaid account and its Sandbox processor token.
    pub fn queue_bank(&self, wallet: &str, bank: &LinkedBank, token: &str) -> AppResult<Value> {
        let mut store = self.lock()?;
        store.require_provider()?;
        let tx = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let customer:Option<String>=tx.query_row("SELECT customer_id FROM provider_customers WHERE wallet=? AND status='verified' AND blocked=''",[wallet],|r|r.get(0)).optional()?.flatten();
        let customer = customer.ok_or_else(|| {
            AppError::bad(
                "verification_required",
                "Verify your wallet before connecting a bank",
            )
        })?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM provider_setup WHERE wallet=? AND kind='bank')",
            [wallet],
            |r| r.get(0),
        )?;
        if exists {
            return Err(AppError::conflict(
                "setup_exists",
                "A bank connection is already pending or linked",
            ));
        }
        let body=Zeroizing::new(json!({"customer":customer,"token":token,"label":bank.label,"mask":bank.mask,"subtype":bank.subtype}).to_string());
        let encrypted = seal_secret(
            self.vault_key.as_ref(),
            body.as_bytes(),
            &format!("{wallet}:bank"),
        )?;
        tx.execute("INSERT INTO provider_setup(id,wallet,kind,phase,encrypted_payload,next_key,next_attempt,created_at) VALUES (?,?,'bank','create_exchange',?,?,?,?)",params![Uuid::new_v4().to_string(),wallet,encrypted,Uuid::new_v4().to_string(),now(),now()])?;
        tx.commit()?;
        Ok(json!({"linked":false,"status":"pending","mode":"dwolla_sandbox"}))
    }
    pub fn bank_setup_allowed(&self, wallet: &str) -> AppResult<()> {
        let store = self.lock()?;
        if store.rail == Rail::Demo {
            return Ok(());
        }
        let ready:bool=store.connection.query_row("SELECT EXISTS(SELECT 1 FROM provider_customers WHERE wallet=? AND status='verified' AND blocked='') AND NOT EXISTS(SELECT 1 FROM provider_setup WHERE wallet=? AND kind='bank' AND phase<>'relink_required')",params![wallet,wallet],|r|r.get(0))?;
        if !ready {
            return Err(AppError::bad(
                "bank_setup_unavailable",
                "Finish wallet verification, or wait for the pending bank connection",
            ));
        }
        Ok(())
    }
}
struct Claim {
    id: String,
    wallet: String,
    kind: String,
    phase: String,
    encrypted: String,
    resource: Option<String>,
    partner: Option<String>,
    next_key: String,
    lease: String,
}
fn claim(state: &AppState) -> AppResult<Option<Claim>> {
    let mut store = state.lock()?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let row:Option<(Claim,Option<i64>)>=tx.query_row("SELECT id,wallet,kind,phase,encrypted_payload,resource_id,partner_id,next_key,first_attempt FROM provider_setup WHERE phase NOT IN ('ready','failed','needs_review','relink_required') AND next_attempt<=? AND lease_until<=? ORDER BY next_attempt,created_at LIMIT 1",params![now(),now()],|r|Ok((Claim{id:r.get(0)?,wallet:r.get(1)?,kind:r.get(2)?,phase:r.get(3)?,encrypted:r.get(4)?,resource:r.get(5)?,partner:r.get(6)?,next_key:r.get(7)?,lease:String::new()},r.get(8)?))).optional()?;
    let Some((mut claim, first)) = row else {
        return Ok(None);
    };
    let writes = matches!(
        claim.phase.as_str(),
        "create_customer" | "create_exchange" | "create_bank"
    );
    if writes && first.is_some_and(|at| now().saturating_sub(at) >= SAFE_RESUBMIT_SECONDS) {
        tx.execute(
            "UPDATE provider_setup SET phase='needs_review',encrypted_payload='' WHERE id=?",
            [&claim.id],
        )?;
        tx.execute(
            "UPDATE provider_link_requests SET status='needs_review',updated_at=? WHERE setup_id=?",
            params![now(), claim.id],
        )?;
        tx.execute(
            "UPDATE provider_customers SET blocked='setup_review' WHERE wallet=?",
            [&claim.wallet],
        )?;
        tx.commit()?;
        return Ok(None);
    }
    claim.lease = Uuid::new_v4().to_string();
    tx.execute("UPDATE provider_setup SET lease_id=?,lease_until=?,first_attempt=CASE WHEN ? THEN COALESCE(first_attempt,?) ELSE first_attempt END,attempts=attempts+1 WHERE id=?",params![claim.lease,now()+90,writes,now(),claim.id])?;
    tx.commit()?;
    Ok(Some(claim))
}
struct Advance {
    phase: String,
    resource: Option<String>,
    partner: Option<String>,
    encrypted: Option<String>,
    customer_status: Option<String>,
    balance: Option<Uuid>,
    bank: Option<(Uuid, String, String)>,
}
impl Advance {
    fn next(phase: &str) -> Self {
        Self {
            phase: phase.into(),
            resource: None,
            partner: None,
            encrypted: None,
            customer_status: None,
            balance: None,
            bank: None,
        }
    }
}
trait SetupProvider: Send + Sync {
    fn create_customer(
        &self,
        profile: &PersonalCustomer,
        id: Uuid,
    ) -> impl std::future::Future<Output = mesh_banking::Result<Uuid>> + Send;
    fn customer_status(
        &self,
        id: Uuid,
    ) -> impl std::future::Future<Output = mesh_banking::Result<String>> + Send;
    fn balance_source(
        &self,
        id: Uuid,
    ) -> impl std::future::Future<Output = mesh_banking::Result<Uuid>> + Send;
    fn plaid_partner(&self)
        -> impl std::future::Future<Output = mesh_banking::Result<Uuid>> + Send;
    fn create_exchange(
        &self,
        customer: Uuid,
        partner: Uuid,
        token: &str,
        id: Uuid,
    ) -> impl std::future::Future<Output = mesh_banking::Result<Uuid>> + Send;
    fn create_bank(
        &self,
        customer: Uuid,
        exchange: Uuid,
        label: &str,
        subtype: &str,
        id: Uuid,
    ) -> impl std::future::Future<Output = mesh_banking::Result<Uuid>> + Send;
    fn verified_bank(
        &self,
        customer: Uuid,
        bank: Uuid,
    ) -> impl std::future::Future<Output = mesh_banking::Result<bool>> + Send;
}
impl SetupProvider for Dwolla {
    async fn create_customer(
        &self,
        profile: &PersonalCustomer,
        id: Uuid,
    ) -> mesh_banking::Result<Uuid> {
        Dwolla::create_customer(self, profile, id).await
    }
    async fn customer_status(&self, id: Uuid) -> mesh_banking::Result<String> {
        Ok(self.customer(id).await?.status)
    }
    async fn balance_source(&self, id: Uuid) -> mesh_banking::Result<Uuid> {
        Dwolla::balance_source(self, id).await
    }
    async fn plaid_partner(&self) -> mesh_banking::Result<Uuid> {
        Dwolla::plaid_partner(self).await
    }
    async fn create_exchange(
        &self,
        customer: Uuid,
        partner: Uuid,
        token: &str,
        id: Uuid,
    ) -> mesh_banking::Result<Uuid> {
        Dwolla::create_exchange(self, customer, partner, token, id).await
    }
    async fn create_bank(
        &self,
        customer: Uuid,
        exchange: Uuid,
        label: &str,
        subtype: &str,
        id: Uuid,
    ) -> mesh_banking::Result<Uuid> {
        Dwolla::create_bank(self, customer, exchange, label, subtype, id).await
    }
    async fn verified_bank(&self, customer: Uuid, bank: Uuid) -> mesh_banking::Result<bool> {
        Dwolla::verified_bank(self, customer, bank).await
    }
}
async fn advance<P: SetupProvider>(
    state: &AppState,
    provider: &P,
    c: &Claim,
) -> AppResult<Advance> {
    let plaintext = if c.encrypted.is_empty() {
        Zeroizing::new(Vec::new())
    } else {
        open_secret(
            state.vault_key.as_ref(),
            &c.encrypted,
            &format!("{}:{}", c.wallet, c.kind),
        )?
    };
    let mut data: Value = if plaintext.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&plaintext)?
    };
    match c.phase.as_str() {
        "plaid_exchange" => {
            let plaid = state
                .plaid
                .as_ref()
                .ok_or_else(|| AppError::internal("Plaid is not configured"))?;
            let credentials = plaid
                .exchange_public_token(data["public_token"].as_str().unwrap_or_default())
                .await?;
            data.as_object_mut()
                .ok_or_else(|| AppError::internal("Bank setup missing"))?
                .remove("public_token");
            data["credentials"] = json!(credentials.as_str());
            encrypted_advance(state, c, "plaid_account", &data)
        }
        "plaid_account" => {
            let plaid = state
                .plaid
                .as_ref()
                .ok_or_else(|| AppError::internal("Plaid is not configured"))?;
            let bank = plaid
                .account(
                    data["credentials"].as_str().unwrap_or_default(),
                    data["account_id"].as_str().unwrap_or_default(),
                )
                .await?;
            data["label"] = json!(bank.label);
            data["mask"] = json!(bank.mask);
            data["subtype"] = json!(bank.subtype);
            data["credentials"] = json!(bank.credentials.as_str());
            encrypted_advance(state, c, "plaid_processor", &data)
        }
        "plaid_processor" => {
            let plaid = state
                .plaid
                .as_ref()
                .ok_or_else(|| AppError::internal("Plaid is not configured"))?;
            let bank = LinkedBank {
                label: data["label"].as_str().unwrap_or_default().into(),
                mask: data["mask"].as_str().unwrap_or_default().into(),
                subtype: data["subtype"].as_str().unwrap_or_default().into(),
                credentials: Zeroizing::new(
                    data["credentials"].as_str().unwrap_or_default().into(),
                ),
            };
            let token = plaid.dwolla_processor_token(&bank).await?;
            data["token"] = json!(token.as_str());
            data.as_object_mut()
                .ok_or_else(|| AppError::internal("Bank setup missing"))?
                .remove("credentials");
            data.as_object_mut()
                .ok_or_else(|| AppError::internal("Bank setup missing"))?
                .remove("account_id");
            encrypted_advance(state, c, "create_exchange", &data)
        }
        "create_customer" => {
            let profile: PersonalCustomer = serde_json::from_value(data)?;
            let id = provider.create_customer(&profile, resource(&c.id)?).await?;
            let mut a = Advance::next("verify_customer");
            a.resource = Some(id.to_string());
            a.encrypted = Some(String::new());
            a.customer_status = Some("pending".into());
            Ok(a)
        }
        "verify_customer" => {
            let customer = resource(c.resource.as_deref().unwrap_or_default())?;
            let status = provider.customer_status(customer).await?;
            let mut a = Advance::next("verify_customer");
            a.customer_status = Some(status.clone());
            if status == "verified" {
                a.balance = Some(provider.balance_source(customer).await?);
                a.phase = "ready".into();
            }
            if matches!(
                status.as_str(),
                "suspended" | "deactivated" | "retry" | "document" | "kba"
            ) {
                a.phase = "needs_review".into();
            }
            Ok(a)
        }
        "create_exchange" => {
            // Persist the selected partner before creating anything, so every retry has identical bytes.
            if c.partner.is_none() {
                let mut a = Advance::next("create_exchange");
                a.partner = Some(provider.plaid_partner().await?.to_string());
                return Ok(a);
            }
            let customer = resource(data["customer"].as_str().unwrap_or_default())?;
            let token = data["token"]
                .as_str()
                .ok_or_else(|| AppError::internal("Processor token missing"))?;
            let id = provider
                .create_exchange(
                    customer,
                    resource(c.partner.as_deref().unwrap_or_default())?,
                    token,
                    resource(&c.id)?,
                )
                .await?;
            data.as_object_mut()
                .ok_or_else(|| AppError::internal("Bank setup missing"))?
                .remove("token");
            let body = Zeroizing::new(data.to_string());
            let mut a = Advance::next("create_bank");
            a.resource = Some(id.to_string());
            a.encrypted = Some(seal_secret(
                state.vault_key.as_ref(),
                body.as_bytes(),
                &format!("{}:bank", c.wallet),
            )?);
            Ok(a)
        }
        "create_bank" => {
            let id = provider
                .create_bank(
                    resource(data["customer"].as_str().unwrap_or_default())?,
                    resource(c.resource.as_deref().unwrap_or_default())?,
                    data["label"].as_str().unwrap_or_default(),
                    data["subtype"].as_str().unwrap_or_default(),
                    resource(&c.next_key)?,
                )
                .await?;
            let mut a = Advance::next("verify_bank");
            a.resource = Some(id.to_string());
            Ok(a)
        }
        "verify_bank" => {
            let id = resource(c.resource.as_deref().unwrap_or_default())?;
            let verified = provider
                .verified_bank(resource(data["customer"].as_str().unwrap_or_default())?, id)
                .await?;
            let mut a = Advance::next(if verified { "ready" } else { "needs_review" });
            if verified {
                a.bank = Some((
                    id,
                    data["label"].as_str().unwrap_or("Bank").into(),
                    data["mask"].as_str().unwrap_or("••••").into(),
                ));
                a.encrypted = Some(String::new());
            }
            Ok(a)
        }
        _ => Err(AppError::internal("Unknown bank setup phase")),
    }
}
fn encrypted_advance(state: &AppState, c: &Claim, phase: &str, data: &Value) -> AppResult<Advance> {
    let body = Zeroizing::new(data.to_string());
    let mut a = Advance::next(phase);
    a.encrypted = Some(seal_secret(
        state.vault_key.as_ref(),
        body.as_bytes(),
        &format!("{}:{}", c.wallet, c.kind),
    )?);
    Ok(a)
}
pub async fn process_one(state: &AppState, provider: &Dwolla) -> AppResult<bool> {
    process_using(state, provider).await
}
async fn process_using<P: SetupProvider>(state: &AppState, provider: &P) -> AppResult<bool> {
    let Some(c) = claim(state)? else {
        return Ok(false);
    };
    let result = advance(state, provider, &c).await;
    persist(state, c, result)
}
fn persist(state: &AppState, c: Claim, result: AppResult<Advance>) -> AppResult<bool> {
    let mut store = state.lock()?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let owned: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM provider_setup WHERE id=? AND lease_id=? AND lease_until>?)",
        params![c.id, c.lease, now()],
        |r| r.get(0),
    )?;
    if !owned {
        return Ok(false);
    }
    match result {
        Ok(a) => {
            let change_phase = a.phase != c.phase;
            tx.execute(
                "UPDATE provider_link_requests SET status=?,updated_at=? WHERE setup_id=?",
                params![a.phase, now(), c.id],
            )?;
            tx.execute("UPDATE provider_setup SET phase=?,resource_id=COALESCE(?,resource_id),partner_id=COALESCE(?,partner_id),encrypted_payload=COALESCE(?,encrypted_payload),first_attempt=CASE WHEN ? THEN NULL ELSE first_attempt END,attempts=CASE WHEN ? THEN 0 ELSE attempts END,next_attempt=?,lease_id=NULL,lease_until=0 WHERE id=?",params![a.phase,a.resource,a.partner,a.encrypted,change_phase,change_phase,now()+if change_phase || a.partner.is_some() {0}else{30},c.id])?;
            if let Some(status) = a.customer_status {
                tx.execute("UPDATE provider_customers SET customer_id=COALESCE(?,customer_id),status=? WHERE wallet=?",params![a.resource.or(c.resource),status,c.wallet])?;
            }
            if let Some(balance) = a.balance {
                tx.execute(
                    "UPDATE provider_customers SET balance_source=? WHERE wallet=?",
                    params![balance.to_string(), c.wallet],
                )?;
            }
            if let Some((id, label, mask)) = a.bank {
                tx.execute(
                    "INSERT INTO provider_banks(wallet,source,label,mask) VALUES (?,?,?,?)",
                    params![c.wallet, id.to_string(), label, mask],
                )?;
            }
            if a.phase == "needs_review" {
                tx.execute(
                    "UPDATE provider_customers SET blocked='setup_review' WHERE wallet=?",
                    [&c.wallet],
                )?;
            }
        }
        Err(error)
            if error.code == "plaid_relink_required"
                && c.phase.starts_with("plaid_")
                && c.resource.is_none() =>
        {
            // No Dwolla resource or transfer was created. A fresh sign-in is safe; stale workers are fenced.
            tx.execute("UPDATE provider_setup SET phase='relink_required',encrypted_payload='',lease_id=NULL,lease_until=0 WHERE id=?",[&c.id])?;
            tx.execute("UPDATE provider_link_requests SET status='relink_required',updated_at=? WHERE setup_id=?",params![now(),c.id])?;
        }
        Err(_) => {
            // A setup failure has no wallet side effect. Unknown POST outcomes keep their original key.
            tx.execute(
                "UPDATE provider_setup SET next_attempt=?,lease_id=NULL,lease_until=0 WHERE id=?",
                params![now() + 30, c.id],
            )?;
        }
    }
    tx.commit()?;
    Ok(true)
}

/// Compare only wallets with no active transfer, after ledger events have been consumed.
/// A discrepancy freezes spending; it can never mint a compensating wallet credit.
pub async fn reconcile_one(state: &AppState, provider: &Dwolla) -> AppResult<()> {
    let candidate: Option<(String, String, String, i64)> = {
        let store = state.lock()?;
        store.connection.query_row("SELECT c.wallet,c.customer_id,c.balance_source,a.balance_cents-c.debt_cents FROM provider_customers c JOIN accounts a ON a.id=c.wallet WHERE c.status='verified' AND c.balance_source IS NOT NULL AND COALESCE(c.checked_at,0)<? AND NOT EXISTS(SELECT 1 FROM provider_requests r WHERE (r.wallet=c.wallet OR r.recipient=c.wallet) AND r.status IN ('queued','submitting','submitted','needs_review')) ORDER BY COALESCE(c.checked_at,0) LIMIT 1",[now()-300],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?
    };
    if let Some((wallet, customer, source, expected)) = candidate {
        let status = provider.customer(resource(&customer)?).await?.status;
        if status != "verified" {
            state.lock()?.connection.execute("UPDATE provider_customers SET status=?,blocked='customer_review',checked_at=? WHERE wallet=?",params![status,now(),wallet])?;
            return Ok(());
        }
        let observed = provider.available_balance(resource(&source)?).await?;
        let mut store = state.lock()?;
        let tx = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Recheck under the write transaction: transfers may have been queued while the GET ran.
        let current:Option<i64>=tx.query_row("SELECT a.balance_cents-c.debt_cents FROM provider_customers c JOIN accounts a ON a.id=c.wallet WHERE c.wallet=? AND NOT EXISTS(SELECT 1 FROM provider_requests r WHERE (r.wallet=c.wallet OR r.recipient=c.wallet) AND r.status IN ('queued','submitting','submitted','needs_review'))",[&wallet],|r|r.get(0)).optional()?;
        if current == Some(expected) {
            tx.execute("UPDATE provider_customers SET checked_at=?,observed_balance=?,blocked=CASE WHEN ?<>? THEN 'balance_review' ELSE blocked END WHERE wallet=?",params![now(),observed,expected,observed,wallet])?;
        }
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use mesh_core::{Identity, Profile};
    use std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicBool, Ordering},
            Mutex,
        },
    };
    struct Fake {
        customer: Uuid,
        balance: Uuid,
        partner: Uuid,
        exchange: Uuid,
        bank: Uuid,
        keys: Mutex<HashMap<Uuid, String>>,
        owned: AtomicBool,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                customer: Uuid::new_v4(),
                balance: Uuid::new_v4(),
                partner: Uuid::new_v4(),
                exchange: Uuid::new_v4(),
                bank: Uuid::new_v4(),
                keys: Mutex::new(HashMap::new()),
                owned: AtomicBool::new(true),
            }
        }
    }
    impl Fake {
        fn key(&self, id: Uuid, body: String) {
            let mut keys = self.keys.lock().unwrap();
            if let Some(old) = keys.insert(id, body.clone()) {
                assert_eq!(old, body);
            }
        }
    }
    impl SetupProvider for Fake {
        async fn create_customer(
            &self,
            p: &PersonalCustomer,
            id: Uuid,
        ) -> mesh_banking::Result<Uuid> {
            self.key(id, serde_json::to_string(p).unwrap());
            Ok(self.customer)
        }
        async fn customer_status(&self, id: Uuid) -> mesh_banking::Result<String> {
            assert_eq!(id, self.customer);
            Ok("verified".into())
        }
        async fn balance_source(&self, id: Uuid) -> mesh_banking::Result<Uuid> {
            assert_eq!(id, self.customer);
            Ok(self.balance)
        }
        async fn plaid_partner(&self) -> mesh_banking::Result<Uuid> {
            Ok(self.partner)
        }
        async fn create_exchange(
            &self,
            c: Uuid,
            p: Uuid,
            t: &str,
            id: Uuid,
        ) -> mesh_banking::Result<Uuid> {
            assert_eq!(c, self.customer);
            assert_eq!(p, self.partner);
            assert_eq!(t, "processor-sandbox-private");
            self.key(id, format!("{c}:{p}:{t}"));
            Ok(self.exchange)
        }
        async fn create_bank(
            &self,
            c: Uuid,
            e: Uuid,
            l: &str,
            s: &str,
            id: Uuid,
        ) -> mesh_banking::Result<Uuid> {
            assert_eq!(c, self.customer);
            assert_eq!(e, self.exchange);
            self.key(id, format!("{c}:{e}:{l}:{s}"));
            Ok(self.bank)
        }
        async fn verified_bank(&self, c: Uuid, b: Uuid) -> mesh_banking::Result<bool> {
            assert_eq!(c, self.customer);
            assert_eq!(b, self.bank);
            Ok(self.owned.load(Ordering::SeqCst))
        }
    }
    fn fixture() -> (AppState, String, Uuid) {
        let mut store = Store::open_with_rail(":memory:", Rail::DwollaSandbox).unwrap();
        let identity = Identity::from_seed(&"01".repeat(32)).unwrap();
        let wallet = identity.public_key();
        store
            .register(&Profile::new(&identity, "A", now()).unwrap())
            .unwrap();
        let state = AppState::new(store, [4; 32], None);
        let id = Uuid::new_v4();
        let input:EnrollmentInput=serde_json::from_value(json!({"id":id,"synthetic_test_data":true,"profile":{"firstName":"John","lastName":"Doe","email":"test@example.com","address1":"1 Test St","city":"Test","state":"NY","postalCode":"11101","dateOfBirth":"1970-01-01","ssn":"1234"}})).unwrap();
        state.enroll(&wallet, input).unwrap();
        (state, wallet, id)
    }
    fn due(state: &AppState) {
        state
            .lock()
            .unwrap()
            .connection
            .execute("UPDATE provider_setup SET next_attempt=0,lease_until=0", [])
            .unwrap();
    }
    fn stored_payload(state: &AppState, kind: &str) -> String {
        state
            .lock()
            .unwrap()
            .connection
            .query_row(
                "SELECT encrypted_payload FROM provider_setup WHERE kind=?",
                [kind],
                |r| r.get(0),
            )
            .unwrap()
    }
    #[tokio::test]
    async fn customer_and_plaid_bank_setup_complete_and_discard_sensitive_payloads() {
        let (state, wallet, _) = fixture();
        let fake = Fake::default();
        let encrypted = stored_payload(&state, "enroll");
        assert!(!encrypted.contains("1234"));
        assert!(!encrypted.contains("John"));
        process_using(&state, &fake).await.unwrap();
        assert_eq!(stored_payload(&state, "enroll"), "");
        process_using(&state, &fake).await.unwrap();
        assert_eq!(
            state.lock().unwrap().snapshot(&wallet, true).unwrap()["banking"]["status"],
            "verified"
        );
        let bank = LinkedBank {
            label: "Sandbox checking".into(),
            mask: "4567".into(),
            subtype: "checking".into(),
            credentials: Zeroizing::new("private-plaid-access-token".into()),
        };
        state
            .queue_bank(&wallet, &bank, "processor-sandbox-private")
            .unwrap();
        assert!(!stored_payload(&state, "bank").contains("processor-sandbox-private"));
        for _ in 0..4 {
            due(&state);
            process_using(&state, &fake).await.unwrap();
        }
        assert_eq!(stored_payload(&state, "bank"), "");
        let snapshot = state.lock().unwrap().snapshot(&wallet, true).unwrap();
        assert_eq!(snapshot["bank"]["mask"], "4567");
        assert_eq!(snapshot["banking"]["bank_status"], "ready");
        assert!(!snapshot.to_string().contains("processor-sandbox"));
        assert_eq!(fake.keys.lock().unwrap().len(), 3);
    }
    #[tokio::test]
    async fn process_death_after_customer_creation_reuses_key_and_fences_stale_worker() {
        let (state, wallet, _) = fixture();
        let fake = Fake::default();
        let stale = claim(&state).unwrap().unwrap();
        let accepted = advance(&state, &fake, &stale).await.unwrap();
        due(&state);
        let newer = claim(&state).unwrap().unwrap();
        let retry = advance(&state, &fake, &newer).await.unwrap();
        assert!(!persist(&state, stale, Ok(accepted)).unwrap());
        assert!(persist(&state, newer, Ok(retry)).unwrap());
        process_using(&state, &fake).await.unwrap();
        assert_eq!(fake.keys.lock().unwrap().len(), 1);
        assert_eq!(
            state.lock().unwrap().snapshot(&wallet, false).unwrap()["banking"]["status"],
            "verified"
        );
    }
    #[test]
    fn unknown_setup_stops_before_idempotency_key_expires() {
        let (state, wallet, _) = fixture();
        let _ = claim(&state).unwrap().unwrap();
        state
            .lock()
            .unwrap()
            .connection
            .execute(
                "UPDATE provider_setup SET lease_until=0,first_attempt=?",
                [now() - SAFE_RESUBMIT_SECONDS],
            )
            .unwrap();
        assert!(claim(&state).unwrap().is_none());
        assert_eq!(stored_payload(&state, "enroll"), "");
        assert_eq!(
            state.lock().unwrap().snapshot(&wallet, false).unwrap()["banking"]["blocked"],
            "setup_review"
        );
    }
    #[tokio::test]
    async fn bank_is_never_bound_without_verified_customer_membership() {
        let (state, wallet, _) = fixture();
        let fake = Fake::default();
        process_using(&state, &fake).await.unwrap();
        process_using(&state, &fake).await.unwrap();
        state
            .queue_bank(
                &wallet,
                &LinkedBank {
                    label: "Bank".into(),
                    mask: "1234".into(),
                    subtype: "checking".into(),
                    credentials: Zeroizing::new(String::new()),
                },
                "processor-sandbox-private",
            )
            .unwrap();
        fake.owned.store(false, Ordering::SeqCst);
        for _ in 0..4 {
            due(&state);
            process_using(&state, &fake).await.unwrap();
        }
        let snapshot = state.lock().unwrap().snapshot(&wallet, false).unwrap();
        assert!(snapshot["bank"].is_null());
        assert_eq!(snapshot["banking"]["bank_status"], "needs_review");
        assert_eq!(snapshot["banking"]["blocked"], "setup_review");
    }
    fn ready_fixture(path: &str, fake: &Fake) -> (AppState, String) {
        let mut store = Store::open_with_rail(path, Rail::DwollaSandbox).unwrap();
        let identity = Identity::from_seed(&"01".repeat(32)).unwrap();
        let wallet = identity.public_key();
        store
            .register(&Profile::new(&identity, "A", now()).unwrap())
            .unwrap();
        store.connection.execute("INSERT INTO provider_customers(wallet,customer_id,balance_source,status) VALUES (?,?,?,'verified')",params![wallet,fake.customer.to_string(),fake.balance.to_string()]).unwrap();
        (AppState::new(store, [4; 32], None), wallet)
    }
    fn link_input() -> LinkInput {
        LinkInput {
            id: Uuid::new_v4(),
            public_token: "public-sandbox-original".into(),
            account_id: "selected".into(),
        }
    }
    fn setup_phase(state: &AppState) -> String {
        state
            .lock()
            .unwrap()
            .connection
            .query_row(
                "SELECT phase FROM provider_setup WHERE kind='bank'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    }
    #[tokio::test]
    async fn plaid_handoff_survives_restart_without_reexchanging_consumed_public_token() {
        use crate::plaid::test_support::mock;
        use reqwest::StatusCode;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wallet.sqlite");
        let path = path.to_str().unwrap();
        let fake = Fake::default();
        let (mut state, wallet) = ready_fixture(path, &fake);
        let mock = mock().await;
        state.plaid = Some(mock.client.clone());
        let input = link_input();
        let first = state.queue_plaid_link(&wallet, &input).unwrap();
        assert_eq!(first["id"], input.id.to_string());
        assert_eq!(first, state.queue_plaid_link(&wallet, &input).unwrap());
        assert!(mock.requests().is_empty());
        let mut changed = link_input();
        changed.id = input.id;
        changed.account_id = "different".into();
        assert_eq!(
            state.queue_plaid_link(&wallet, &changed).unwrap_err().code,
            "id_conflict"
        );
        mock.reply(
            StatusCode::OK,
            json!({"access_token":"access-sandbox-private","item_id":"item-123"}),
        );
        process_using(&state, &fake).await.unwrap();
        assert_eq!(setup_phase(&state), "plaid_account");
        let payload = open_secret(
            &[4; 32],
            &stored_payload(&state, "bank"),
            &format!("{wallet}:bank"),
        )
        .unwrap();
        let payload = String::from_utf8_lossy(&payload);
        assert!(!payload.contains("public-sandbox-original"));
        assert!(payload.contains("access-sandbox-private"));
        mock.reply(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error_code":"INTERNAL_SERVER_ERROR"}),
        );
        process_using(&state, &fake).await.unwrap();
        assert_eq!(setup_phase(&state), "plaid_account");
        drop(state);
        let state = AppState::new(
            Store::open_with_rail(path, Rail::DwollaSandbox).unwrap(),
            [4; 32],
            Some(mock.client.clone()),
        );
        mock.reply(StatusCode::OK,json!({"accounts":[{"account_id":"selected","type":"depository","subtype":"checking","name":"Verified checking","mask":"9876"}]}));
        due(&state);
        process_using(&state, &fake).await.unwrap();
        mock.reply(
            StatusCode::OK,
            json!({"processor_token":"processor-sandbox-private"}),
        );
        due(&state);
        process_using(&state, &fake).await.unwrap();
        let payload = open_secret(
            &[4; 32],
            &stored_payload(&state, "bank"),
            &format!("{wallet}:bank"),
        )
        .unwrap();
        let payload = String::from_utf8_lossy(&payload);
        assert!(!payload.contains("access-sandbox"));
        assert!(!payload.contains("credentials"));
        for _ in 0..4 {
            due(&state);
            process_using(&state, &fake).await.unwrap();
        }
        assert_eq!(stored_payload(&state, "bank"), "");
        let snapshot = state.lock().unwrap().snapshot(&wallet, true).unwrap();
        assert_eq!(snapshot["bank"]["mask"], "9876");
        assert_eq!(snapshot["banking"]["bank_status"], "ready");
        assert_eq!(
            state.queue_plaid_link(&wallet, &input).unwrap()["status"],
            "ready"
        );
        assert_eq!(
            mock.requests()
                .iter()
                .map(|r| r.0.as_str())
                .collect::<Vec<_>>(),
            [
                "/item/public_token/exchange",
                "/accounts/get",
                "/accounts/get",
                "/processor/token/create"
            ]
        );
    }
    #[tokio::test]
    async fn expired_link_can_be_replaced_and_old_retries_cannot_replace_the_new_attempt() {
        use crate::plaid::test_support::mock;
        use reqwest::StatusCode;
        let fake = Fake::default();
        let (mut state, wallet) = ready_fixture(":memory:", &fake);
        let mock = mock().await;
        state.plaid = Some(mock.client.clone());
        let old = link_input();
        state.queue_plaid_link(&wallet, &old).unwrap();
        mock.reply(
            StatusCode::BAD_REQUEST,
            json!({"error_code":"INVALID_PUBLIC_TOKEN"}),
        );
        process_using(&state, &fake).await.unwrap();
        assert_eq!(setup_phase(&state), "relink_required");
        assert_eq!(stored_payload(&state, "bank"), "");
        assert!(state.bank_setup_allowed(&wallet).is_ok());
        let fresh = link_input();
        state.queue_plaid_link(&wallet, &fresh).unwrap();
        assert_eq!(
            state.queue_plaid_link(&wallet, &old).unwrap()["status"],
            "relink_required"
        );
        assert_eq!(setup_phase(&state), "plaid_exchange");
        assert_eq!(
            state.queue_plaid_link(&wallet, &fresh).unwrap()["status"],
            "plaid_exchange"
        );
        assert_eq!(
            state.lock().unwrap().snapshot(&wallet, true).unwrap()["banking"]["blocked"],
            ""
        );
        assert!(fake.keys.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn lost_exchange_result_never_creates_a_bank_and_stale_worker_cannot_restore_it() {
        use crate::plaid::test_support::mock;
        use reqwest::StatusCode;
        let fake = Fake::default();
        let (mut state, wallet) = ready_fixture(":memory:", &fake);
        let mock = mock().await;
        state.plaid = Some(mock.client.clone());
        state.queue_plaid_link(&wallet, &link_input()).unwrap();
        mock.reply(
            StatusCode::OK,
            json!({"access_token":"access-sandbox-private","item_id":"orphan-item"}),
        );
        let stale = claim(&state).unwrap().unwrap();
        let lost = advance(&state, &fake, &stale).await.unwrap();
        due(&state);
        mock.reply(
            StatusCode::BAD_REQUEST,
            json!({"error_code":"INVALID_PUBLIC_TOKEN"}),
        );
        process_using(&state, &fake).await.unwrap();
        assert!(!persist(&state, stale, Ok(lost)).unwrap());
        assert_eq!(setup_phase(&state), "relink_required");
        assert!(fake.keys.lock().unwrap().is_empty());
        assert!(state.lock().unwrap().snapshot(&wallet, true).unwrap()["bank"].is_null());
    }
    #[tokio::test]
    async fn signed_link_rpc_returns_a_durable_receipt_before_provider_network_work() {
        use axum::{body::Body, http::Request};
        use http_body_util::BodyExt;
        use mesh_core::SignedRequest;
        use tower::ServiceExt;
        let fake = Fake::default();
        let (state, wallet) = ready_fixture(":memory:", &fake);
        let input = link_input();
        let identity = Identity::from_seed(&"01".repeat(32)).unwrap();
        let request = SignedRequest::new(
            &identity,
            "link_bank",
            json!({"id":input.id,"public_token":input.public_token,"account_id":input.account_id}),
            now(),
        )
        .unwrap();
        let response = crate::router(state.clone())
            .oneshot(
                Request::post("/v1/rpc")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["data"]["id"], input.id.to_string());
        assert!(!value.to_string().contains("public-sandbox"));
        assert_eq!(
            state.lock().unwrap().snapshot(&wallet, true).unwrap()["banking"]["bank_status"],
            "plaid_exchange"
        );
    }
}
