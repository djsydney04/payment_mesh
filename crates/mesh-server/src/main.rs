use mesh_banking::{dwolla::Dwolla, outbox::Outbox};
use mesh_server::{
    bank_webhooks::BankWebhooks, plaid::Plaid, provider::Rail, store::Store, AppState,
};
use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mesh_server=info".into()),
        )
        .init();
    for variable in ["MESH_MODE", "PLAID_ENV", "DWOLLA_ENV"] {
        if let Ok(mode) = std::env::var(variable) {
            if mode != "sandbox" {
                return Err(format!(
                    "{variable} must be sandbox. Live money movement is not implemented."
                )
                .into());
            }
        }
    }
    let path = std::env::var("MESH_DATABASE").unwrap_or_else(|_| ".local/mesh.sqlite".into());
    if let Some(parent) = std::path::Path::new(&path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let key = match std::env::var("MESH_VAULT_KEY") {
        Ok(key) => mesh_core::decode32(&key)?,
        Err(_) => {
            let key_path = format!("{path}.vault-key");
            match std::fs::read_to_string(&key_path) {
                Ok(key) => mesh_core::decode32(key.trim())?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let seed = zeroize::Zeroizing::new(mesh_core::random_seed());
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(key_path)?
                        .write_all(seed.as_bytes())?;
                    mesh_core::decode32(&seed)?
                }
                Err(e) => return Err(e.into()),
            }
        }
    };
    let rail = match std::env::var("MESH_RAIL").as_deref() {
        Ok("dwolla_sandbox") => Rail::DwollaSandbox,
        Ok("demo") | Err(_) => Rail::Demo,
        _ => return Err("MESH_RAIL must be demo or dwolla_sandbox".into()),
    };
    let store = Store::open_with_rail(&path, rail)?;
    if !store.audit()? {
        return Err("Ledger audit failed; restore a consistent database before starting".into());
    }
    let mut state = AppState::new(store, key, Plaid::from_env()?);
    let provider = if rail == Rail::DwollaSandbox {
        if state.plaid.is_none() {
            return Err("Dwolla Sandbox wallets require Plaid Sandbox configuration".into());
        }
        Some(
            Dwolla::from_env()?
                .ok_or("Set DWOLLA_KEY and DWOLLA_SECRET for the bank-backed Sandbox")?,
        )
    } else {
        None
    };
    let mut outbox = if provider.is_some() {
        Some(Outbox::open(&path)?)
    } else {
        None
    };
    match (
        std::env::var("MESH_BANK_OUTBOX"),
        std::env::var("DWOLLA_WEBHOOK_SECRET"),
    ) {
        (Ok(webhook_path), Ok(secret)) => {
            if rail == Rail::DwollaSandbox
                && std::path::Path::new(&webhook_path) != std::path::Path::new(&path)
            {
                return Err("Bank-backed webhooks must use the wallet's MESH_DATABASE".into());
            }
            state =
                state.with_bank_webhooks(BankWebhooks::open(&webhook_path, secret.into_bytes())?);
        }
        (Err(_), Ok(secret)) if rail == Rail::DwollaSandbox => {
            state = state.with_bank_webhooks(BankWebhooks::open(&path, secret.into_bytes())?);
        }
        (Err(_), Err(_)) => {}
        _ => return Err("Set MESH_BANK_OUTBOX and DWOLLA_WEBHOOK_SECRET together".into()),
    }
    let worker_state = state.clone();
    let worker = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let (Some(provider), Some(outbox)) = (&provider, &mut outbox) {
                if worker_state
                    .lock()
                    .and_then(|mut s| s.apply_bank_events())
                    .is_err()
                {
                    tracing::error!(
                        "Bank ledger reconciliation failed; transfer submission paused"
                    );
                    continue;
                }
                if mesh_server::setup::process_one(&worker_state, provider)
                    .await
                    .is_err()
                {
                    tracing::error!("Bank setup will be retried");
                }
                if outbox
                    .run_once(&mesh_server::provider::WalletRail {
                        state: &worker_state,
                        provider,
                    })
                    .await
                    .is_err()
                {
                    tracing::error!("Bank transfer worker will retry");
                }
                if worker_state
                    .lock()
                    .and_then(|mut s| s.apply_bank_events())
                    .is_err()
                {
                    tracing::error!("Bank ledger reconciliation failed");
                    continue;
                }
                if mesh_server::setup::reconcile_one(&worker_state, provider)
                    .await
                    .is_err()
                {
                    tracing::error!("Provider balance reconciliation will retry");
                }
                continue;
            }
            match worker_state
                .lock()
                .and_then(|mut s| s.process_demo_payouts(mesh_core::now()))
            {
                Ok(count) if count > 0 => tracing::info!(count, "Completed demo payouts"),
                Err(_) => tracing::error!("Demo payout processing failed; will retry"),
                _ => {}
            }
        }
    });
    let bind = std::env::var("MESH_BIND").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(address=%listener.local_addr()?,"Mesh sandbox ready — test money only");
    axum::serve(listener, mesh_server::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    worker.abort();
    Ok(())
}
