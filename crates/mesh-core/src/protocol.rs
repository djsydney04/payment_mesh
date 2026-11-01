//! Typed v1 wire objects and encrypted transport.
//!
//! Profiles advertise keys; payments authorize a ledger-bound instruction; packets
//! hide that instruction from relays. Verification here does not establish funds.
//! Do not reorder signed body fields without a protocol compatibility decision.
use crate::*;
use chacha20poly1305::aead::Payload;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Maximum serialized packet accepted by fragmentation/reassembly.
pub const MAX_PACKET_BYTES: usize = 16_384;
/// Initial forwarding budget for honest relays; the mutable counter is unsigned.
pub const MAX_HOPS: u8 = 4;
/// Maximum instruction lifetime in seconds (24 hours).
pub const PAYMENT_LIFETIME: i64 = 86_400;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProfileBody {
    pub version: u8,
    pub public_key: String,
    pub encryption_key: String,
    pub name: String,
    pub created_at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub body: ProfileBody,
    pub signature: String,
}

impl Profile {
    /// Sign a trimmed display name and this identity's public keys at `at`.
    pub fn new(identity: &Identity, name: &str, at: i64) -> Result<Self> {
        let body = ProfileBody {
            version: 1,
            public_key: identity.public_key(),
            encryption_key: identity.encryption_key(),
            name: name.trim().to_string(),
            created_at: at,
        };
        let signature = identity.sign("meshpay/profile/v1", &body)?;
        let profile = Self { body, signature };
        profile.verify(at)?;
        Ok(profile)
    }
    /// Validate name, keys, signature and profile age; not the person's identity.
    pub fn verify(&self, at: i64) -> Result<()> {
        if self.body.version != 1
            || self.body.created_at > at.saturating_add(120)
            || at.saturating_sub(self.body.created_at) > PAYMENT_LIFETIME
        {
            return Err(Error::Invalid("Nearby identity expired"));
        }
        if self.body.name.is_empty()
            || self.body.name.chars().count() > 32
            || self.body.name.chars().any(char::is_control)
        {
            return Err(Error::Invalid("Use a name with 1–32 characters"));
        }
        decode32(&self.body.encryption_key)?;
        verify(
            &self.body.public_key,
            "meshpay/profile/v1",
            &self.body,
            &self.signature,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PaymentBody {
    pub version: u8,
    pub ledger_id: Uuid,
    pub id: Uuid,
    pub sender: String,
    pub recipient: String,
    pub amount_cents: i64,
    pub currency: String,
    pub memo: String,
    pub created_at: i64,
    pub expires_at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Payment {
    pub body: PaymentBody,
    pub signature: String,
}

impl Payment {
    /// Create a fresh UUID and sign an exact-cent instruction for one ledger.
    ///
    /// This validates the instruction, but cannot check registration or balance.
    pub fn new(
        identity: &Identity,
        recipient: &str,
        amount_cents: i64,
        memo: &str,
        ledger_id: Uuid,
        at: i64,
    ) -> Result<Self> {
        let body = PaymentBody {
            version: 1,
            ledger_id,
            id: Uuid::new_v4(),
            sender: identity.public_key(),
            recipient: recipient.to_owned(),
            amount_cents,
            currency: "USD".into(),
            memo: memo.trim().into(),
            created_at: at,
            expires_at: at.saturating_add(PAYMENT_LIFETIME),
        };
        let signature = identity.sign("meshpay/payment/v1", &body)?;
        let payment = Self { body, signature };
        payment.verify(at)?;
        Ok(payment)
    }
    /// Validate the signed instruction and expiry without performing settlement.
    pub fn verify(&self, at: i64) -> Result<()> {
        let b = &self.body;
        validate_amount(b.amount_cents)?;
        decode32(&b.recipient)?;
        if b.version != 1
            || b.currency != "USD"
            || b.sender == b.recipient
            || b.id.is_nil()
            || b.ledger_id.is_nil()
        {
            return Err(Error::Invalid("Invalid payment"));
        }
        if b.memo.chars().count() > 80 || b.memo.chars().any(char::is_control) {
            return Err(Error::Invalid("Keep the note under 80 characters"));
        }
        if b.created_at > at.saturating_add(120)
            || b.expires_at <= at
            || b.expires_at <= b.created_at
            || b.expires_at.saturating_sub(b.created_at) > PAYMENT_LIFETIME
        {
            return Err(Error::Invalid("This payment has expired"));
        }
        verify(&b.sender, "meshpay/payment/v1", b, &self.signature)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeBody {
    pub version: u8,
    pub id: Uuid,
    pub sender: String,
    pub recipient: String,
    pub expires_at: i64,
    pub ephemeral_key: String,
    pub nonce: String,
    pub ciphertext: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub body: EnvelopeBody,
    pub signature: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshPacket {
    pub hops_remaining: u8,
    pub envelope: Envelope,
}

fn envelope_aad(body: &EnvelopeBody) -> Vec<u8> {
    format!(
        "meshpay/sealed/v1:{}:{}:{}:{}",
        body.id, body.sender, body.recipient, body.expires_at
    )
    .into_bytes()
}

fn shared_key(secret: &x25519_dalek::StaticSecret, public: &str) -> Result<Zeroizing<[u8; 32]>> {
    let public = x25519_dalek::PublicKey::from(decode32(public)?);
    let shared = secret.diffie_hellman(&public);
    if !shared.was_contributory() {
        return Err(Error::Crypto);
    }
    let mut key = Zeroizing::new([0; 32]);
    hkdf::Hkdf::<Sha256>::new(Some(b"meshpay/sealed/v1"), shared.as_bytes())
        .expand(b"chacha20poly1305", key.as_mut())
        .map_err(|_| Error::Crypto)?;
    Ok(key)
}

impl MeshPacket {
    /// Encrypt a valid payment to a current signed recipient profile and sign its envelope.
    pub fn seal(
        identity: &Identity,
        recipient: &Profile,
        payment: &Payment,
        at: i64,
    ) -> Result<Self> {
        recipient.verify(at)?;
        payment.verify(at)?;
        if payment.body.sender != identity.public_key()
            || payment.body.recipient != recipient.body.public_key
        {
            return Err(Error::Invalid("Payment identity mismatch"));
        }
        let ephemeral = x25519_dalek::StaticSecret::random_from_rng(OsRng);
        let key = shared_key(&ephemeral, &recipient.body.encryption_key)?;
        let mut nonce = [0; 12];
        OsRng.fill_bytes(&mut nonce);
        let mut body = EnvelopeBody {
            version: 1,
            id: payment.body.id,
            sender: payment.body.sender.clone(),
            recipient: payment.body.recipient.clone(),
            expires_at: payment.body.expires_at,
            ephemeral_key: hex::encode(x25519_dalek::PublicKey::from(&ephemeral).as_bytes()),
            nonce: hex::encode(nonce),
            ciphertext: String::new(),
        };
        let cipher = ChaCha20Poly1305::new(key.as_ref().into());
        body.ciphertext = hex::encode(
            cipher
                .encrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: &serde_json::to_vec(payment)?,
                        aad: &envelope_aad(&body),
                    },
                )
                .map_err(|_| Error::Crypto)?,
        );
        let signature = identity.sign("meshpay/envelope/v1", &body)?;
        Ok(Self {
            hops_remaining: MAX_HOPS,
            envelope: Envelope { body, signature },
        })
    }
    /// Check the outer envelope without decrypting the recipient-only payment.
    pub fn verify(&self, at: i64) -> Result<()> {
        let b = &self.envelope.body;
        if self.hops_remaining > MAX_HOPS
            || b.version != 1
            || b.expires_at <= at
            || b.expires_at > at.saturating_add(PAYMENT_LIFETIME + 120)
            || b.id.is_nil()
            || b.sender == b.recipient
        {
            return Err(Error::Invalid("Invalid or expired mesh packet"));
        }
        if b.ciphertext.len() > 8192 || b.ciphertext.len() < 32 || b.nonce.len() != 24 {
            return Err(Error::Invalid("Invalid packet size"));
        }
        decode32(&b.recipient)?;
        decode32(&b.ephemeral_key)?;
        verify(
            &b.sender,
            "meshpay/envelope/v1",
            b,
            &self.envelope.signature,
        )
    }
    /// Decrypt as the intended recipient, verify the inner signature, and match headers.
    pub fn open(&self, identity: &Identity, at: i64) -> Result<Payment> {
        self.verify(at)?;
        let b = &self.envelope.body;
        if b.recipient != identity.public_key() {
            return Err(Error::Invalid("This packet is for another wallet"));
        }
        let key = shared_key(&identity.encryption, &b.ephemeral_key)?;
        let nonce = hex::decode(&b.nonce).map_err(|_| Error::Crypto)?;
        let ciphertext = hex::decode(&b.ciphertext).map_err(|_| Error::Crypto)?;
        let cipher = ChaCha20Poly1305::new(key.as_ref().into());
        let plain = Zeroizing::new(
            cipher
                .decrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: &ciphertext,
                        aad: &envelope_aad(b),
                    },
                )
                .map_err(|_| Error::Crypto)?,
        );
        let payment: Payment = serde_json::from_slice(&plain)?;
        payment.verify(at)?;
        if payment.body.sender != b.sender
            || payment.body.recipient != b.recipient
            || payment.body.id != b.id
            || payment.body.expires_at != b.expires_at
        {
            return Err(Error::Invalid("Packet does not match payment"));
        }
        Ok(payment)
    }
    /// Validate and decrement the honest relay budget; `None` means no hops remain.
    ///
    /// Does not persist or transmit the result. The hop count is not signed.
    pub fn forwarded(&self, at: i64) -> Result<Option<Self>> {
        self.verify(at)?;
        if self.hops_remaining == 0 {
            return Ok(None);
        }
        let mut packet = self.clone();
        packet.hops_remaining -= 1;
        Ok(Some(packet))
    }
}
