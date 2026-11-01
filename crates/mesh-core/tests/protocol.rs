use mesh_core::*;
use serde_json::json;

fn wallet(n: u8) -> Identity {
    Identity::from_seed(&hex::encode([n; 32])).unwrap()
}
fn fixture() -> (Identity, Identity, Profile, Payment) {
    let a = wallet(1);
    let b = wallet(2);
    let profile = Profile::new(&b, "Sam", now()).unwrap();
    let payment = Payment::new(
        &a,
        &b.public_key(),
        1250,
        "Coffee",
        uuid::Uuid::from_u128(1),
        now(),
    )
    .unwrap();
    (a, b, profile, payment)
}

#[test]
fn amounts_use_exact_cents_and_reject_ambiguous_values() {
    for (input, expected) in [
        ("0.01", 1),
        ("1", 100),
        ("1.2", 120),
        ("12.50", 1250),
        ("1000.00", 100000),
    ] {
        assert_eq!(parse_cents(input).unwrap(), expected);
    }
    for bad in [
        "0",
        "-1",
        "1.001",
        ".50",
        "1.",
        "1e2",
        "NaN",
        " 1",
        "1 ",
        "1,000",
        "$1",
        "1000.01",
        "999999999999999999999",
        "",
        "1.2.3",
    ] {
        assert!(parse_cents(bad).is_err(), "{bad}");
    }
}
#[test]
fn keys_and_fingerprints_are_deterministic_and_domain_separated() {
    let a = wallet(1);
    let again = wallet(1);
    assert_eq!(a.public_key(), again.public_key());
    assert_eq!(a.encryption_key(), again.encryption_key());
    assert_ne!(a.public_key(), a.encryption_key());
    assert_eq!(a.fingerprint(), again.fingerprint());
    assert!(Identity::from_seed("bad").is_err());
    assert!(Identity::from_seed(&"FF".repeat(32)).is_err());
    let sig = a.sign("one", &json!({"n":1})).unwrap();
    assert!(verify(&a.public_key(), "two", &json!({"n":1}), &sig).is_err());
}
#[test]
fn signed_profiles_cannot_be_impersonated_or_renamed() {
    let (_, _, mut profile, _) = fixture();
    profile.body.name = "Not Sam".into();
    assert!(profile.verify(now()).is_err());
    assert!(Profile::new(&wallet(3), "", now()).is_err());
    assert!(Profile::new(&wallet(3), "Bad\nName", now()).is_err());
    assert!(Profile::new(&wallet(3), &"x".repeat(33), now()).is_err());
}
#[test]
fn payment_mutations_fail() {
    let (_, _, _, payment) = fixture();
    for field in [
        "amount_cents",
        "recipient",
        "memo",
        "currency",
        "expires_at",
    ] {
        let mut value = serde_json::to_value(&payment).unwrap();
        value["body"][field] = match field {
            "amount_cents" => json!(2000),
            "expires_at" => json!(now() + 100),
            "recipient" => json!(wallet(3).public_key()),
            "currency" => json!("EUR"),
            _ => json!("changed"),
        };
        let modified: Payment = serde_json::from_value(value).unwrap();
        assert!(modified.verify(now()).is_err(), "{field}");
    }
}
#[test]
fn self_payment_expired_future_and_zero_are_rejected() {
    let (a, b, _, _) = fixture();
    assert!(Payment::new(
        &a,
        &a.public_key(),
        100,
        "",
        uuid::Uuid::from_u128(1),
        now()
    )
    .is_err());
    assert!(Payment::new(&a, &b.public_key(), 0, "", uuid::Uuid::from_u128(1), now()).is_err());
    let expired = Payment::new(
        &a,
        &b.public_key(),
        100,
        "",
        uuid::Uuid::from_u128(1),
        now() - PAYMENT_LIFETIME - 1,
    )
    .unwrap();
    assert!(expired.verify(now()).is_err());
    let future = Payment::new(
        &a,
        &b.public_key(),
        100,
        "",
        uuid::Uuid::from_u128(1),
        now() + 121,
    )
    .unwrap();
    assert!(future.verify(now()).is_err());
}
#[test]
fn mesh_confidentiality_and_tampering() {
    let (a, b, profile, payment) = fixture();
    let packet = MeshPacket::seal(&a, &profile, &payment, now()).unwrap();
    assert_eq!(packet.open(&b, now()).unwrap(), payment);
    assert!(packet.open(&wallet(3), now()).is_err());
    assert!(!serde_json::to_string(&packet).unwrap().contains("Coffee"));
    let mut modified = packet.clone();
    modified.envelope.body.ciphertext.replace_range(..2, "ff");
    assert!(modified.open(&b, now()).is_err());
    modified = packet;
    modified.envelope.body.recipient = wallet(3).public_key();
    assert!(modified.verify(now()).is_err());
}
#[test]
fn low_order_encryption_key_is_rejected_even_with_valid_identity() {
    let (a, b, mut profile, payment) = fixture();
    profile.body.encryption_key = "00".repeat(32);
    profile.signature = b.sign("meshpay/profile/v1", &profile.body).unwrap();
    assert!(MeshPacket::seal(&a, &profile, &payment, now()).is_err());
}
#[test]
fn relay_hops_stop_and_cannot_change_signed_contents() {
    let (a, _, profile, payment) = fixture();
    let mut packet = MeshPacket::seal(&a, &profile, &payment, now()).unwrap();
    for hops in (0..MAX_HOPS).rev() {
        packet = packet.forwarded(now()).unwrap().unwrap();
        assert_eq!(packet.hops_remaining, hops);
    }
    assert!(packet.forwarded(now()).unwrap().is_none());
    packet.hops_remaining = MAX_HOPS + 1;
    assert!(packet.verify(now()).is_err());
}
#[test]
fn fragmented_mesh_survives_ordering_duplicates_and_different_mtus() {
    let (a, b, profile, payment) = fixture();
    let packet = MeshPacket::seal(&a, &profile, &payment, now()).unwrap();
    for mtu in [40, 80, 180, 512] {
        let frames = framing::fragment(&packet, mtu).unwrap();
        let mut assembler = framing::Reassembler::default();
        let mut deliveries = 0;
        for frame in frames.iter().rev().chain(frames.iter()) {
            if let Some(p) = assembler.push("radio", frame, now()).unwrap() {
                assert_eq!(p.open(&b, now()).unwrap(), payment);
                deliveries += 1;
            }
        }
        assert_eq!(deliveries, 1);
    }
}
#[test]
fn fragments_from_different_radios_do_not_mix() {
    let (a, _, profile, payment) = fixture();
    let frames = framing::fragment(
        &MeshPacket::seal(&a, &profile, &payment, now()).unwrap(),
        180,
    )
    .unwrap();
    let mut assembler = framing::Reassembler::default();
    for (i, f) in frames.iter().enumerate() {
        assert!(assembler
            .push(if i % 2 == 0 { "one" } else { "two" }, f, now())
            .unwrap()
            .is_none());
    }
}
#[test]
fn malformed_frames_are_bounded_and_never_panic() {
    let mut assembler = framing::Reassembler::default();
    for len in 0..600 {
        let frame: Vec<_> = (0..len).map(|i| (i % 256) as u8).collect();
        let _ = assembler.push("attacker", &frame, now());
    }
    let (a, _, profile, payment) = fixture();
    let mut frames = framing::fragment(
        &MeshPacket::seal(&a, &profile, &payment, now()).unwrap(),
        80,
    )
    .unwrap();
    assembler.push("one", &frames[0], now()).unwrap();
    frames[0][21] ^= 1;
    assert!(assembler.push("one", &frames[0], now()).is_err());
    assert!(framing::fragment(
        &MeshPacket::seal(&a, &profile, &payment, now()).unwrap(),
        20
    )
    .is_err());
}
#[test]
fn http_requests_bind_payload_action_wallet_and_time() {
    let a = wallet(1);
    let request = SignedRequest::new(&a, "snapshot", json!({}), now()).unwrap();
    request.verify(now()).unwrap();
    assert!(request.verify(now() + 121).is_err());
    let mut altered = request.clone();
    altered.body.action = "withdraw".into();
    assert!(altered.verify(now()).is_err());
    altered = request.clone();
    altered.body.payload = json!({"amount":100});
    assert!(altered.verify(now()).is_err());
    altered = request;
    altered.body.public_key = wallet(2).public_key();
    assert!(altered.verify(now()).is_err());
}
#[test]
fn bank_tokens_are_encrypted_and_bound_to_their_owner() {
    let key = [42; 32];
    let token = seal_secret(&key, b"access-sandbox-secret", "alice").unwrap();
    assert!(!token.contains("sandbox"));
    assert_eq!(
        &*open_secret(&key, &token, "alice").unwrap(),
        b"access-sandbox-secret"
    );
    assert!(open_secret(&key, &token, "bob").is_err());
    assert!(open_secret(&[43; 32], &token, "alice").is_err());
    assert!(open_secret(&key, "00", "alice").is_err());
}
