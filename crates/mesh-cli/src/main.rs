//! Two independent wallets exercise the real HTTP and Bluetooth wire protocol.
use mesh_core::*;
use serde_json::{json, Value};

async fn rpc(
    client: &reqwest::Client,
    url: &str,
    wallet: &Identity,
    action: &str,
    payload: Value,
) -> std::result::Result<Value, Box<dyn std::error::Error>> {
    let body = SignedRequest::new(wallet, action, payload, now())?;
    let response: Value = client
        .post(format!("{url}/v1/rpc"))
        .json(&body)
        .send()
        .await?
        .json()
        .await?;
    if response["ok"] != true {
        return Err(response.to_string().into());
    }
    Ok(response["data"].clone())
}
#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:8787".into());
    let client = reqwest::Client::new();
    let alice = Identity::from_seed(&random_seed())?;
    let bob = Identity::from_seed(&random_seed())?;
    let bob_profile = Profile::new(&bob, "Sam", now())?;
    rpc(
        &client,
        &url,
        &alice,
        "register",
        serde_json::to_value(Profile::new(&alice, "You", now())?)?,
    )
    .await?;
    rpc(
        &client,
        &url,
        &bob,
        "register",
        serde_json::to_value(&bob_profile)?,
    )
    .await?;
    rpc(&client, &url, &alice, "demo_fund", json!({})).await?;
    println!("Created two wallets. Sender funded with $125.40 in test money.");
    let snapshot = rpc(&client, &url, &alice, "snapshot", json!({})).await?;
    let ledger_id = uuid::Uuid::parse_str(
        snapshot["ledger_id"]
            .as_str()
            .ok_or("Missing ledger identity")?,
    )?;
    let payment = Payment::new(
        &alice,
        &bob.public_key(),
        1250,
        "Coffee + a little extra",
        ledger_id,
        now(),
    )?;
    let packet = MeshPacket::seal(&alice, &bob_profile, &payment, now())?;
    let mut reassembler = framing::Reassembler::default();
    let mut delivered = None;
    let frames = framing::fragment(&packet, 180)?;
    for frame in frames.iter().rev() {
        if let Some(packet) = reassembler.push("alice-radio", frame, now())? {
            delivered = Some(packet.open(&bob, now())?);
        }
    }
    let delivered = delivered.ok_or("Mesh reassembly failed")?;
    println!(
        "Encrypted $12.50 payment delivered in {} BLE fragments, out of order.",
        frames.len()
    );
    let receipt = rpc(
        &client,
        &url,
        &bob,
        "settle",
        serde_json::to_value(&delivered)?,
    )
    .await?;
    let retry = rpc(
        &client,
        &url,
        &alice,
        "settle",
        serde_json::to_value(&payment)?,
    )
    .await?;
    assert_eq!(receipt, retry);
    println!("Recipient submitted it. Sender retry returned the same receipt; no double charge.");
    rpc(&client, &url, &bob, "demo_link_bank", json!({})).await?;
    rpc(
        &client,
        &url,
        &bob,
        "withdraw",
        json!({"id":uuid::Uuid::new_v4(),"amount_cents":500}),
    )
    .await?;
    let a = rpc(&client, &url, &alice, "snapshot", json!({})).await?;
    let b = rpc(&client, &url, &bob, "snapshot", json!({})).await?;
    assert_eq!(a["balance_cents"], 11290);
    assert_eq!(b["balance_cents"], 750);
    println!("Sender: $112.90. Recipient: $7.50, with $5.00 reserved for the demo bank.");
    println!("End-to-end demo passed. No real money moved.");
    Ok(())
}
