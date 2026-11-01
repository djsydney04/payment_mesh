//! MeshPay v1: signed payment instructions, confidential Bluetooth packets, and framing.
//!
//! A delivered instruction is never proof of funds or final settlement.
//! This crate has no network client or authoritative balance store. A server must
//! authenticate the submitter, check the instruction's ledger and available funds,
//! and commit its accounting result. Client-side validation is only one layer.
//!
//! # Send an instruction through the packet format
//!
//! The example generates disposable identities and simulates radio frames in memory.
//! A real wallet must preserve its seed securely and obtain `ledger_id` from its
//! server snapshot rather than generating a replacement authority.
//!
//! ```
//! use mesh_core::{framing, parse_cents, random_seed, Identity, MeshPacket, Payment, Profile};
//! use uuid::Uuid;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let at = 1_700_000_000; // An explicit clock keeps this example reproducible.
//! let alice = Identity::from_seed(&random_seed())?;
//! let bob = Identity::from_seed(&random_seed())?;
//! let bob_profile = Profile::new(&bob, "Bob", at)?;
//! let example_ledger = Uuid::new_v4();
//! let payment = Payment::new(
//!     &alice, &bob.public_key(), parse_cents("5.00")?, "Coffee", example_ledger, at,
//! )?;
//! let packet = MeshPacket::seal(&alice, &bob_profile, &payment, at)?;
//! let frames = framing::fragment(&packet, 180)?;
//!
//! let mut receiver = framing::Reassembler::default();
//! let mut delivered = None;
//! for frame in frames.iter().rev() { // Reassembly accepts out-of-order fragments.
//!     if let Some(packet) = receiver.push("alice-radio", frame, at)? {
//!         delivered = Some(packet);
//!     }
//! }
//! let received = delivered.expect("all frames delivered").open(&bob, at)?;
//! assert_eq!(received, payment);
//! assert_eq!(received.body.amount_cents, 500);
//! // Save the instruction before submitting it to its ledger. No money moved here.
//! # Ok(())
//! # }
//! ```
//!
//! # Compatibility
//!
//! Signatures cover a domain, a NUL separator, and compact typed JSON serialization.
//! Field order and domain strings are protocol contracts; signing arbitrary raw JSON
//! bytes is not interchangeable. Unknown fields in signed wire models are rejected.
//! See the repository's `docs/PROTOCOL.md` for the exact v1 wire specification and
//! `docs/API.md` for the separate signed HTTP request contract.
pub mod framing;
pub mod protocol;

use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

pub use protocol::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("Signature could not be verified")]
    Signature,
    #[error("This packet could not be decrypted")]
    Crypto,
    #[error("Invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

/// Current Unix time in whole seconds; callers may inject a clock into protocol methods.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Generate 32 OS-random bytes as lowercase hex. Treat the returned string as a secret.
///
/// This does not persist the seed. Losing it loses the wallet identity.
pub fn random_seed() -> String {
    let mut seed = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(seed.as_mut());
    hex::encode(seed.as_ref())
}

/// Decode exactly 64 lowercase hexadecimal characters into 32 bytes.
pub fn decode32(value: &str) -> Result<[u8; 32]> {
    if value.len() != 64
        || value
            .bytes()
            .any(|b| !b.is_ascii_hexdigit() || b.is_ascii_uppercase())
    {
        return Err(Error::Invalid(
            "Expected a lowercase 32-byte hexadecimal key",
        ));
    }
    let bytes = hex::decode(value).map_err(|_| Error::Invalid("Invalid hexadecimal key"))?;
    bytes
        .try_into()
        .map_err(|_| Error::Invalid("Invalid key length"))
}

/// Signing and recipient-encryption keys derived from one wallet seed.
///
/// The signing public key identifies the wallet; display names do not. Private key
/// material is intentionally not serializable through this type.
pub struct Identity {
    signing: SigningKey,
    encryption: x25519_dalek::StaticSecret,
}

impl Identity {
    /// Recreate an identity from its 32-byte lowercase-hex seed without storing it.
    pub fn from_seed(seed: &str) -> Result<Self> {
        let seed = Zeroizing::new(decode32(seed)?);
        let mut enc = Zeroizing::new([0u8; 32]);
        hkdf::Hkdf::<Sha256>::new(Some(b"meshpay/identity/v1"), seed.as_ref())
            .expand(b"x25519", enc.as_mut())
            .map_err(|_| Error::Crypto)?;
        Ok(Self {
            signing: SigningKey::from_bytes(&seed),
            encryption: x25519_dalek::StaticSecret::from(*enc),
        })
    }

    /// Ed25519 verification key, encoded as lowercase hex.
    pub fn public_key(&self) -> String {
        hex::encode(self.signing.verifying_key().as_bytes())
    }
    /// Static X25519 recipient key, encoded as lowercase hex.
    pub fn encryption_key(&self) -> String {
        hex::encode(x25519_dalek::PublicKey::from(&self.encryption).as_bytes())
    }
    /// Sign domain-separated, compact typed JSON; use the same type when verifying.
    pub fn sign<T: Serialize>(&self, domain: &str, value: &T) -> Result<String> {
        Ok(hex::encode(
            self.signing.sign(&signing_bytes(domain, value)?).to_bytes(),
        ))
    }
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.public_key())
    }
}

/// Short comparison code from the textual public key; not an identity credential.
pub fn fingerprint(key: &str) -> String {
    let hash = hex::encode(Sha256::digest(key.as_bytes()));
    format!("{} {} {}", &hash[..4], &hash[4..8], &hash[8..12])
}

// Structs have a fixed field order; Value maps use serde_json's default sorted map.
// Sign the same typed structure at both ends; never the raw HTTP JSON string.
fn signing_bytes<T: Serialize>(domain: &str, value: &T) -> Result<Vec<u8>> {
    let mut bytes = domain.as_bytes().to_vec();
    bytes.push(0);
    bytes.extend(serde_json::to_vec(value)?);
    Ok(bytes)
}

/// Strictly verify a lowercase-hex Ed25519 key's domain-separated JSON signature.
pub fn verify<T: Serialize>(key: &str, domain: &str, value: &T, signature: &str) -> Result<()> {
    let key = VerifyingKey::from_bytes(&decode32(key)?).map_err(|_| Error::Signature)?;
    let signature = hex::decode(signature).map_err(|_| Error::Signature)?;
    let signature = Signature::from_slice(&signature).map_err(|_| Error::Signature)?;
    key.verify_strict(&signing_bytes(domain, value)?, &signature)
        .map_err(|_| Error::Signature)
}

/// Parse exact USD cents without floating point, whitespace, symbols, or rounding.
///
/// ```
/// assert_eq!(mesh_core::parse_cents("12.5").unwrap(), 1250);
/// assert!(mesh_core::parse_cents("12.501").is_err());
/// assert!(mesh_core::parse_cents("0").is_err());
/// ```
pub fn parse_cents(input: &str) -> Result<i64> {
    if input.is_empty() || input.len() > 12 || input.trim() != input {
        return Err(Error::Invalid("Enter an amount like 12.50"));
    }
    let parts: Vec<_> = input.split('.').collect();
    if parts.len() > 2 || parts[0].is_empty() || !parts[0].bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Invalid("Use dollars and up to two decimal places"));
    }
    let dollars: i64 = parts[0]
        .parse()
        .map_err(|_| Error::Invalid("Amount is too large"))?;
    let fraction = if parts.len() == 2 {
        let p = parts[1];
        if p.is_empty() || p.len() > 2 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::Invalid("Use up to two decimal places"));
        }
        p.parse::<i64>()
            .map_err(|_| Error::Invalid("Invalid cents"))?
            * if p.len() == 1 { 10 } else { 1 }
    } else {
        0
    };
    let cents = dollars
        .checked_mul(100)
        .and_then(|n| n.checked_add(fraction))
        .ok_or(Error::Invalid("Amount is too large"))?;
    validate_amount(cents)?;
    Ok(cents)
}

/// Per-operation v1 Sandbox maximum: $1,000.00.
pub const MAX_PAYMENT_CENTS: i64 = 100_000;
/// Require positive cents no greater than [`MAX_PAYMENT_CENTS`].
pub fn validate_amount(cents: i64) -> Result<()> {
    if !(1..=MAX_PAYMENT_CENTS).contains(&cents) {
        return Err(Error::Invalid("Choose an amount from $0.01 to $1,000.00"));
    }
    Ok(())
}

/// Encrypt server-side bank credentials; the wallet key is authenticated as AAD.
pub fn seal_secret(key: &[u8; 32], plaintext: &[u8], owner: &str) -> Result<String> {
    let mut nonce = [0; 12];
    OsRng.fill_bytes(&mut nonce);
    let cipher = ChaCha20Poly1305::new(key.into());
    let data = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            chacha20poly1305::aead::Payload {
                msg: plaintext,
                aad: owner.as_bytes(),
            },
        )
        .map_err(|_| Error::Crypto)?;
    Ok(format!("{}{}", hex::encode(nonce), hex::encode(data)))
}

/// Decrypt a server credential using the exact key and owner supplied at sealing.
///
/// A different owner fails authentication. This does not decrypt the ledger file.
pub fn open_secret(key: &[u8; 32], value: &str, owner: &str) -> Result<Zeroizing<Vec<u8>>> {
    let bytes = hex::decode(value).map_err(|_| Error::Crypto)?;
    if bytes.len() < 28 {
        return Err(Error::Crypto);
    }
    let cipher = ChaCha20Poly1305::new(key.into());
    let result = cipher
        .decrypt(
            Nonce::from_slice(&bytes[..12]),
            chacha20poly1305::aead::Payload {
                msg: &bytes[12..],
                aad: owner.as_bytes(),
            },
        )
        .map_err(|_| Error::Crypto)?;
    Ok(Zeroizing::new(result))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Authenticated HTTP request, separate from a radio packet or financial receipt.
pub struct SignedRequest {
    pub body: RequestBody,
    pub signature: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Typed signed RPC fields. The nonce is per attempt, not a payment's stable ID.
pub struct RequestBody {
    pub version: u8,
    pub public_key: String,
    pub timestamp: i64,
    pub nonce: uuid::Uuid,
    pub action: String,
    pub payload: serde_json::Value,
}
impl SignedRequest {
    /// Sign one fresh HTTP attempt. Preserve operation IDs inside `payload` on retry.
    pub fn new(
        identity: &Identity,
        action: &str,
        payload: serde_json::Value,
        at: i64,
    ) -> Result<Self> {
        let body = RequestBody {
            version: 1,
            public_key: identity.public_key(),
            timestamp: at,
            nonce: uuid::Uuid::new_v4(),
            action: action.to_owned(),
            payload,
        };
        let signature = identity.sign("meshpay/rpc/v1", &body)?;
        Ok(Self { body, signature })
    }
    /// Verify v1 structure, clock skew and signature; does not consume a nonce.
    ///
    /// The server must separately persist nonce use and authorize the action.
    pub fn verify(&self, at: i64) -> Result<()> {
        if self.body.version != 1 || self.body.timestamp.abs_diff(at) > 120 {
            return Err(Error::Invalid("Request expired; check your phone's clock"));
        }
        if self.body.action.len() > 32 {
            return Err(Error::Invalid("Invalid action"));
        }
        verify(
            &self.body.public_key,
            "meshpay/rpc/v1",
            &self.body,
            &self.signature,
        )
    }
}
