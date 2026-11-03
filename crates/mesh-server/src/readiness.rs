use mesh_banking::dwolla::Dwolla;
use mesh_server::plaid::Plaid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Serve,
    CheckBanking,
    ProbeBanking,
}
impl Command {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, &'static str> {
        let args: Vec<_> = args.into_iter().collect();
        match args.as_slice() {
            [] => Ok(Self::Serve),
            [arg] if arg == "--check-banking" => Ok(Self::CheckBanking),
            [arg] if arg == "--probe-banking" => Ok(Self::ProbeBanking),
            _ => Err("Usage: mesh-server [--check-banking | --probe-banking]"),
        }
    }
}

#[derive(Default, Debug)]
pub struct ReadinessReport {
    pub missing: Vec<&'static str>,
    pub invalid: Vec<&'static str>,
}
impl ReadinessReport {
    pub fn inspect(get: impl Fn(&str) -> Option<String>) -> Self {
        let mut report = Self::default();
        for name in [
            "MESH_RAIL",
            "MESH_DATABASE",
            "PLAID_CLIENT_ID",
            "PLAID_SECRET",
            "PLAID_REDIRECT_URI",
            "DWOLLA_KEY",
            "DWOLLA_SECRET",
        ] {
            if get(name).is_none_or(|value| value.trim().is_empty()) {
                report.missing.push(name);
            }
        }
        for name in ["MESH_MODE", "PLAID_ENV", "DWOLLA_ENV"] {
            if get(name).is_some_and(|value| value != "sandbox") {
                report.invalid.push(name);
            }
        }
        if get("MESH_RAIL").is_some_and(|value| !value.is_empty() && value != "dwolla_sandbox") {
            report.invalid.push("MESH_RAIL");
        }
        if let Some(secret) = get("DWOLLA_WEBHOOK_SECRET") {
            if !(16..=1024).contains(&secret.len()) {
                report.invalid.push("DWOLLA_WEBHOOK_SECRET");
            }
        }
        if let Some(outbox) = get("MESH_BANK_OUTBOX") {
            if get("MESH_DATABASE").as_deref() != Some(outbox.as_str()) {
                report.invalid.push("MESH_BANK_OUTBOX");
            }
            if get("DWOLLA_WEBHOOK_SECRET").is_none() {
                report.missing.push("DWOLLA_WEBHOOK_SECRET");
            }
        }
        report
    }
    fn validate(&self) -> Result<(), String> {
        if self.missing.is_empty() && self.invalid.is_empty() {
            return Ok(());
        }
        let mut errors = Vec::new();
        if !self.missing.is_empty() {
            errors.push(format!("Missing settings: {}", self.missing.join(", ")));
        }
        if !self.invalid.is_empty() {
            errors.push(format!(
                "Invalid Sandbox settings: {}",
                self.invalid.join(", ")
            ));
        }
        Err(errors.join("; "))
    }
}

pub async fn run(probe: bool) -> Result<(), Box<dyn std::error::Error>> {
    ReadinessReport::inspect(|name| std::env::var(name).ok()).validate()?;
    let plaid = Plaid::from_env()?.ok_or("Plaid Sandbox settings are required")?;
    let dwolla = Dwolla::from_env()?.ok_or("Dwolla Sandbox settings are required")?;
    println!(
        "Sandbox banking configuration is valid. No database or vault key was opened or created."
    );
    if probe {
        dwolla
            .verify_connection()
            .await
            .map_err(|error| format!("Dwolla Sandbox probe failed: {error}"))?;
        println!("Dwolla Sandbox authentication succeeded.");
        plaid
            .link_token("mesh-readiness-check")
            .await
            .map_err(|error| format!("Plaid Sandbox probe failed: {error}"))?;
        println!("Plaid Sandbox Auth Link token creation succeeded; token discarded.");
        println!("No customers, bank links, or transfers were created. Complete the app acceptance flow next.");
    } else {
        println!(
            "No provider requests were made. Use --probe-banking to check external Sandbox access."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_commands_cannot_accidentally_start_server_or_probe() {
        assert_eq!(Command::parse([]), Ok(Command::Serve));
        assert_eq!(
            Command::parse(["--check-banking".into()]),
            Ok(Command::CheckBanking)
        );
        assert_eq!(
            Command::parse(["--probe-banking".into()]),
            Ok(Command::ProbeBanking)
        );
        assert!(Command::parse(["--check-banking".into(), "--probe-banking".into()]).is_err());
        assert!(Command::parse(["--probe-bankng".into()]).is_err());
    }
    #[test]
    fn reports_missing_names_and_rejects_live_settings_without_echoing_values() {
        let report = ReadinessReport::inspect(|name| match name {
            "PLAID_SECRET" => Some("private-secret".into()),
            "DWOLLA_KEY" => Some("  ".into()),
            "PLAID_ENV" => Some("private-production-value".into()),
            _ => None,
        });
        let message = report.validate().unwrap_err();
        assert!(report.missing.contains(&"DWOLLA_KEY"));
        assert!(!report.missing.contains(&"PLAID_SECRET"));
        assert_eq!(report.invalid, ["PLAID_ENV"]);
        assert!(!message.contains("private"));
    }
    #[test]
    fn bank_outbox_must_match_wallet_database_and_have_valid_webhook_secret() {
        let report = ReadinessReport::inspect(|name| {
            Some(
                match name {
                    "MESH_MODE" | "PLAID_ENV" | "DWOLLA_ENV" => "sandbox",
                    "MESH_RAIL" => "dwolla_sandbox",
                    "MESH_DATABASE" => "wallet.sqlite",
                    "MESH_BANK_OUTBOX" => "other.sqlite",
                    "DWOLLA_WEBHOOK_SECRET" => "short",
                    _ => "configured",
                }
                .into(),
            )
        });
        assert!(report.missing.is_empty());
        assert_eq!(
            report.invalid,
            ["DWOLLA_WEBHOOK_SECRET", "MESH_BANK_OUTBOX"]
        );
    }
}
