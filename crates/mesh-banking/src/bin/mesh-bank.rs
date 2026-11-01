//! Sandbox acceptance runner. It never creates ledger balances or enables production banking.
use mesh_banking::{
    dwolla::Dwolla,
    outbox::{Outbox, Work},
    BankOperation,
};
use std::time::Duration;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().collect();
    let usage = "Usage: mesh-bank probe | enqueue OUTBOX OPERATION.json | work OUTBOX [--once] | status OUTBOX OPERATION_UUID | events OUTBOX [CURSOR]";
    let command = arguments.get(1).ok_or(usage)?;
    if command == "probe" {
        let rail = Dwolla::from_env()?
            .ok_or("Set DWOLLA_KEY and DWOLLA_SECRET with Sandbox credentials")?;
        rail.verify_connection().await?;
        println!("Dwolla Sandbox authentication verified. No funds moved.");
        return Ok(());
    }
    let path = arguments.get(2).ok_or(usage)?;
    let mut outbox = Outbox::open(path)?;
    match command.as_str() {
        "enqueue" => {
            let file = arguments.get(3).ok_or(usage)?;
            let bytes = std::fs::read(file)?;
            if bytes.len() > 16_384 {
                return Err("Operation file is too large".into());
            }
            let operation: BankOperation = serde_json::from_slice(&bytes)?;
            let job = outbox.enqueue(&operation, mesh_core::now())?;
            println!("{} {} — sandbox only", job.operation.id, job.status);
        }
        "work" => {
            let rail = Dwolla::from_env()?
                .ok_or("Set DWOLLA_KEY and DWOLLA_SECRET with Sandbox credentials")?;
            let once = arguments.get(3).is_some_and(|arg| arg == "--once");
            loop {
                let result = outbox.run_once(&rail).await?;
                if result != Work::Idle {
                    println!("{result:?}");
                }
                if once {
                    break;
                }
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => break,
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
            }
        }
        "status" => {
            let id = Uuid::parse_str(arguments.get(3).ok_or(usage)?)?;
            let job = outbox.job(id)?.ok_or("Bank operation not found")?;
            println!("{}", serde_json::to_string_pretty(&job)?);
        }
        "events" => {
            let cursor = arguments
                .get(3)
                .map(|s| s.parse::<i64>())
                .transpose()?
                .unwrap_or(0);
            println!(
                "{}",
                serde_json::to_string_pretty(&outbox.events_after(cursor, 100)?)?
            );
        }
        _ => return Err(usage.into()),
    }
    Ok(())
}
