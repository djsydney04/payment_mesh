use crate::*;
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub struct VerifiedEvent {
    id: Uuid,
    digest: String,
    transfer_id: Option<Uuid>,
}
impl VerifiedEvent {
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn transfer_id(&self) -> Option<Uuid> {
        self.transfer_id
    }
}

/// Authenticate the raw request body, not a parsed/reserialized JSON object.
/// The constructor is private so the outbox cannot accidentally accept unverified events.
pub fn verify(secret: &[u8], signature: &str, body: &[u8]) -> Result<VerifiedEvent> {
    if secret.len() < 16 || body.len() > 32_768 || signature.len() != 64 {
        return Err(Error::Invalid("Invalid bank webhook"));
    }
    let signature = hex::decode(signature).map_err(|_| Error::Invalid("Invalid bank webhook"))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret)
        .map_err(|_| Error::Invalid("Invalid bank webhook"))?;
    mac.update(body);
    mac.verify_slice(&signature)
        .map_err(|_| Error::Invalid("Invalid bank webhook"))?;
    let value: Value = serde_json::from_slice(body)?;
    let id = Uuid::parse_str(value["id"].as_str().ok_or(Error::Protocol)?)
        .map_err(|_| Error::Protocol)?;
    if id.is_nil() || value["topic"].as_str().is_none_or(|s| s.len() > 128) {
        return Err(Error::Protocol);
    }
    let transfer_id = if let Some(resource) = value["_links"]["resource"]["href"].as_str() {
        let url = reqwest::Url::parse(resource).map_err(|_| Error::Protocol)?;
        let expected = reqwest::Url::parse(DWOLLA_SANDBOX).map_err(|_| Error::Protocol)?;
        if url.origin() != expected.origin()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Protocol);
        }
        let parts: Vec<_> = url.path_segments().ok_or(Error::Protocol)?.collect();
        if parts.len() == 2 && parts[0] == "transfers" {
            let id = Uuid::parse_str(parts[1]).map_err(|_| Error::Protocol)?;
            if id.is_nil() {
                return Err(Error::Protocol);
            }
            Some(id)
        } else {
            None
        }
    } else {
        None
    };
    Ok(VerifiedEvent {
        id,
        digest: hex::encode(Sha256::digest(body)),
        transfer_id,
    })
}
