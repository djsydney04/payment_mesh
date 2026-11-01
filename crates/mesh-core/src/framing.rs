//! Each BLE value: UUID (16 bytes), big-endian index (2), total (2), payload.
//! Bounded reassembly, expiration, and duplicate suppression apply before routing.
use crate::{Error, MeshPacket, Result, MAX_PACKET_BYTES};
use std::collections::{HashMap, VecDeque};
use uuid::Uuid;

pub const HEADER: usize = 20;
const MAX_ASSEMBLIES: usize = 32;
const MAX_FRAMES: usize = 1024;

/// Serialize into values no larger than `mtu` (40–512 bytes), including the header.
///
/// This size/serialization operation does not verify the packet's signature.
pub fn fragment(packet: &MeshPacket, mtu: usize) -> Result<Vec<Vec<u8>>> {
    if !(40..=512).contains(&mtu) {
        return Err(Error::Invalid("BLE MTU must be 40–512 bytes"));
    }
    let bytes = serde_json::to_vec(packet)?;
    if bytes.len() > MAX_PACKET_BYTES {
        return Err(Error::Invalid("Packet is too large"));
    }
    let chunks: Vec<_> = bytes.chunks(mtu - HEADER).collect();
    let total = chunks.len() as u16;
    Ok(chunks
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| {
            let mut frame = Vec::with_capacity(mtu);
            frame.extend_from_slice(packet.envelope.body.id.as_bytes());
            frame.extend_from_slice(&(i as u16).to_be_bytes());
            frame.extend_from_slice(&total.to_be_bytes());
            frame.extend_from_slice(chunk);
            frame
        })
        .collect())
}

struct Assembly {
    at: i64,
    total: usize,
    bytes: usize,
    chunks: HashMap<usize, Vec<u8>>,
}
#[derive(Default)]
/// In-memory, bounded assemblies and recent packet IDs; not durable delivery state.
pub struct Reassembler {
    partial: HashMap<(String, Uuid), Assembly>,
    seen: VecDeque<Uuid>,
}
impl Reassembler {
    /// Add one frame from a stable radio source and verify a completed outer packet.
    ///
    /// Returns `None` for incomplete or recently seen packets. Assemblies expire
    /// after 30 seconds; duplicate suppression is bounded, not permanent replay
    /// protection. The recipient must still call [`MeshPacket::open`] and the
    /// ledger must enforce financial idempotency.
    pub fn push(&mut self, source: &str, frame: &[u8], at: i64) -> Result<Option<MeshPacket>> {
        self.partial.retain(|_, a| at.saturating_sub(a.at) <= 30);
        if frame.len() <= HEADER || frame.len() > 512 || source.len() > 128 {
            return Err(Error::Invalid("Invalid BLE frame"));
        }
        let id = Uuid::from_slice(&frame[..16]).map_err(|_| Error::Invalid("Invalid frame ID"))?;
        if self.seen.contains(&id) {
            return Ok(None);
        }
        let index = u16::from_be_bytes([frame[16], frame[17]]) as usize;
        let total = u16::from_be_bytes([frame[18], frame[19]]) as usize;
        if total == 0 || total > MAX_FRAMES || index >= total {
            return Err(Error::Invalid("Invalid fragment count"));
        }
        let key = (source.to_owned(), id);
        if !self.partial.contains_key(&key) && self.partial.len() >= MAX_ASSEMBLIES {
            return Err(Error::Invalid("Too many incomplete packets"));
        }
        let a = self.partial.entry(key.clone()).or_insert_with(|| Assembly {
            at,
            total,
            bytes: 0,
            chunks: HashMap::new(),
        });
        if a.total != total {
            self.partial.remove(&key);
            return Err(Error::Invalid("Fragment count changed"));
        }
        if let Some(existing) = a.chunks.get(&index) {
            if existing != &frame[HEADER..] {
                self.partial.remove(&key);
                return Err(Error::Invalid("Conflicting fragment"));
            }
            return Ok(None);
        }
        a.bytes += frame.len() - HEADER;
        if a.bytes > MAX_PACKET_BYTES {
            self.partial.remove(&key);
            return Err(Error::Invalid("Packet is too large"));
        }
        a.chunks.insert(index, frame[HEADER..].to_vec());
        if a.chunks.len() != total {
            return Ok(None);
        }
        let a = self
            .partial
            .remove(&key)
            .ok_or(Error::Invalid("Missing assembly"))?;
        let mut data = Vec::with_capacity(a.bytes);
        for i in 0..total {
            data.extend_from_slice(&a.chunks[&i]);
        }
        let packet: MeshPacket = serde_json::from_slice(&data)?;
        if packet.envelope.body.id != id {
            return Err(Error::Invalid("Frame ID mismatch"));
        }
        packet.verify(at)?;
        self.seen.push_back(id);
        if self.seen.len() > 2048 {
            self.seen.pop_front();
        }
        Ok(Some(packet))
    }
}
