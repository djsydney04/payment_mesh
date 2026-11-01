//! Minimal owned-string C ABI. Swift never implements payment or cryptographic rules.
//!
//! The JSON operation reference is in `docs/IOS_RUST.md`; the public header is
//! `ios/MeshPay/MeshCore.h`. All returned strings must be released with
//! [`mesh_string_free`], including seeds. Each engine requires exclusive access
//! across calls and destruction. Invalid pointer lifetimes remain undefined behavior;
//! the panic boundary only converts recoverable Rust panics into JSON errors.
use base64::{engine::general_purpose::STANDARD, Engine};
use mesh_core::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    ffi::{c_char, CStr, CString},
    panic::{catch_unwind, AssertUnwindSafe},
};
use zeroize::Zeroizing;

/// Opaque owned wallet identity plus mutable, in-memory radio reassembly state.
pub struct MeshEngine {
    identity: Identity,
    reassembler: framing::Reassembler,
}

#[no_mangle]
/// Allocate a fresh secret seed string; copy/store securely, then use [`mesh_string_free`].
pub extern "C" fn mesh_seed_new() -> *mut c_char {
    CString::new(mesh_core::random_seed())
        .expect("hex contains no NULs")
        .into_raw()
}

/// Borrow a seed for this call and allocate an engine; invalid input returns null.
///
/// # Safety
/// A non-null `seed` must point to a readable NUL-terminated UTF-8 C string for
/// the duration of this call. The caller retains ownership of that string and
/// owns the returned handle, which must eventually be passed to [`mesh_engine_free`].
#[no_mangle]
pub unsafe extern "C" fn mesh_engine_new(seed: *const c_char) -> *mut MeshEngine {
    if seed.is_null() {
        return std::ptr::null_mut();
    }
    catch_unwind(|| {
        let seed = Zeroizing::new(unsafe { CStr::from_ptr(seed) }.to_str().ok()?.to_owned());
        let identity = Identity::from_seed(&seed).ok()?;
        Some(Box::into_raw(Box::new(MeshEngine {
            identity,
            reassembler: framing::Reassembler::default(),
        })))
    })
    .ok()
    .flatten()
    .unwrap_or(std::ptr::null_mut())
}

/// Dispatch one JSON command and allocate its JSON result, including error results.
///
/// # Safety
/// A non-null `engine` must be a live handle returned by [`mesh_engine_new`],
/// exclusively accessed for this call. A non-null `request` must point to a
/// readable NUL-terminated UTF-8 C string for the duration of the call. Text
/// exceeding 256 KiB is rejected only after the C string is read. Release the
/// returned allocation with [`mesh_string_free`], never another allocator.
#[no_mangle]
pub unsafe extern "C" fn mesh_engine_call(
    engine: *mut MeshEngine,
    request: *const c_char,
) -> *mut c_char {
    let response = catch_unwind(AssertUnwindSafe(|| {
        if engine.is_null() || request.is_null() {
            return json!({"ok":false,"error":"Missing engine or request"});
        }
        let result = (|| -> Result<Value> {
            let text = unsafe { CStr::from_ptr(request) }
                .to_str()
                .map_err(|_| Error::Invalid("Invalid UTF-8"))?;
            if text.len() > 262_144 {
                return Err(Error::Invalid("Request too large"));
            }
            let command: Value = serde_json::from_str(text)?;
            unsafe { &mut *engine }.dispatch(command)
        })();
        match result {
            Ok(data) => json!({"ok":true,"data":data}),
            Err(e) => json!({"ok":false,"error":e.to_string()}),
        }
    }))
    .unwrap_or_else(
        |_| json!({"ok":false,"error":"The Rust core could not complete this request"}),
    );
    CString::new(response.to_string())
        .expect("JSON escapes NULs")
        .into_raw()
}

/// Release a wallet engine; null is a no-op.
///
/// # Safety
/// Pass an owned, live engine handle exactly once. Never call concurrently with
/// [`mesh_engine_call`] or access the handle afterward.
#[no_mangle]
pub unsafe extern "C" fn mesh_engine_free(engine: *mut MeshEngine) {
    if !engine.is_null() {
        drop(unsafe { Box::from_raw(engine) });
    }
}
/// Release an ABI string; null is a no-op.
///
/// # Safety
/// A non-null value must be an unmodified live allocation returned by
/// [`mesh_seed_new`] or [`mesh_engine_call`]. Pass it exactly once and do not
/// access the string afterward.
#[no_mangle]
pub unsafe extern "C" fn mesh_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or(Error::Invalid("Missing text field"))
}
fn typed<T: for<'a> Deserialize<'a>>(value: &Value, key: &str) -> Result<T> {
    Ok(serde_json::from_value(
        value
            .get(key)
            .cloned()
            .ok_or(Error::Invalid("Missing field"))?,
    )?)
}

impl MeshEngine {
    fn dispatch(&mut self, v: Value) -> Result<Value> {
        let at = now();
        match field(&v, "op")? {
            "parse_amount" => Ok(json!({"amount_cents":parse_cents(field(&v,"amount")?)?})),
            "profile" => {
                let profile = Profile::new(&self.identity, field(&v, "name")?, at)?;
                Ok(
                    json!({"profile":profile,"fingerprint":self.identity.fingerprint(),"public_key":self.identity.public_key()}),
                )
            }
            "verify_profile" => {
                let profile: Profile = typed(&v, "profile")?;
                profile.verify(at)?;
                Ok(json!({"profile":profile,"fingerprint":fingerprint(&profile.body.public_key)}))
            }
            "request" => Ok(serde_json::to_value(SignedRequest::new(
                &self.identity,
                field(&v, "action")?,
                v.get("payload").cloned().unwrap_or(json!({})),
                at,
            )?)?),
            "create_payment" => {
                let profile: Profile = typed(&v, "recipient")?;
                profile.verify(at)?;
                let cents = parse_cents(field(&v, "amount")?)?;
                let available = self.spendable(&v)?;
                if cents > available {
                    return Err(Error::Invalid("Your available balance is too low"));
                }
                let payment = Payment::new(
                    &self.identity,
                    &profile.body.public_key,
                    cents,
                    v.get("memo").and_then(Value::as_str).unwrap_or(""),
                    uuid::Uuid::parse_str(field(&v, "ledger_id")?)
                        .map_err(|_| Error::Invalid("Connect once to set up your ledger"))?,
                    at,
                )?;
                let packet = MeshPacket::seal(&self.identity, &profile, &payment, at)?;
                Ok(json!({"payment":payment,"packet":packet}))
            }
            "spendable" => Ok(json!({"amount_cents":self.spendable(&v)?})),
            "fragment" => {
                let packet: MeshPacket = typed(&v, "packet")?;
                packet.verify(at)?;
                let mtu = v.get("mtu").and_then(Value::as_u64).unwrap_or(180) as usize;
                let frames: Vec<String> = framing::fragment(&packet, mtu)?
                    .iter()
                    .map(|f| STANDARD.encode(f))
                    .collect();
                Ok(json!({"frames":frames}))
            }
            "receive_frame" => {
                let bytes = STANDARD
                    .decode(field(&v, "frame")?)
                    .map_err(|_| Error::Invalid("Invalid frame encoding"))?;
                match self.reassembler.push(field(&v, "source")?, &bytes, at)? {
                    Some(packet) => {
                        let payment =
                            if packet.envelope.body.recipient == self.identity.public_key() {
                                Some(packet.open(&self.identity, at)?)
                            } else {
                                None
                            };
                        let forward = if payment.is_none() {
                            packet.forwarded(at)?
                        } else {
                            None
                        };
                        Ok(json!({"packet":packet,"payment":payment,"forward":forward}))
                    }
                    None => Ok(json!({})),
                }
            }
            "validate_payment" => {
                let payment: Payment = typed(&v, "payment")?;
                payment.verify(at)?;
                Ok(json!({"valid":true}))
            }
            _ => Err(Error::Invalid("Unknown Rust operation")),
        }
    }
    fn spendable(&self, v: &Value) -> Result<i64> {
        let balance = v
            .get("balance_cents")
            .and_then(Value::as_i64)
            .ok_or(Error::Invalid("Missing balance"))?;
        if balance < 0 {
            return Err(Error::Invalid("Invalid balance"));
        }
        let pending: Vec<Payment> = typed(v, "pending")?;
        if pending.len() > 512 {
            return Err(Error::Invalid(
                "Too many pending payments; reconnect to sync",
            ));
        }
        let mut seen = std::collections::HashSet::new();
        let reserved = pending
            .iter()
            .filter(|p| {
                p.body.sender == self.identity.public_key()
                    && p.body.expires_at > now()
                    && seen.insert(p.body.id)
            })
            .try_fold(0i64, |a, p| {
                a.checked_add(p.body.amount_cents)
                    .ok_or(Error::Invalid("Pending amount overflow"))
            })?;
        Ok(balance.saturating_sub(reserved).max(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ffi_round_trip_and_errors() {
        unsafe {
            let seed = CString::new(random_seed()).unwrap();
            let handle = mesh_engine_new(seed.as_ptr());
            assert!(!handle.is_null());
            for (text, ok) in [
                (r#"{"op":"profile","name":"Dylan"}"#, true),
                ("bad-json", false),
                (r#"{"op":"unknown"}"#, false),
            ] {
                let input = CString::new(text).unwrap();
                let output = mesh_engine_call(handle, input.as_ptr());
                let response: Value =
                    serde_json::from_str(CStr::from_ptr(output).to_str().unwrap()).unwrap();
                assert_eq!(response["ok"], ok);
                mesh_string_free(output);
            }
            mesh_engine_free(handle);
        }
    }
}
