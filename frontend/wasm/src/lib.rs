use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use wasm_bindgen::prelude::*;

const HEADER_MAGIC: &[u8; 5] = b"QRF1H";
const DATA_MAGIC: &[u8; 5] = b"QRF1D";
const MAX_PACKET_BASE64_BYTES: usize = 1_800;
// Magic (5) + checksum (8) + index (4). This keeps the Base64 packet below 1,800 bytes.
const MAX_DATA_BYTES: usize = 1_333;
const MAX_FILENAME_BYTES: usize = 1_300;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct HeaderMetadata {
    checksum: u64,
    total_packets: u32,
    file_size: u64,
    filename: String,
}

#[derive(Default)]
struct ReceiverState {
    checksum: Option<u64>,
    total_packets: usize,
    file_size: usize,
    filename: String,
    packets: Vec<Option<Vec<u8>>>,
    pending: HashMap<u32, Vec<u8>>,
    pending_checksum: Option<u64>,
    received_packets: usize,
    complete: bool,
}

thread_local! {
    static RECEIVER: RefCell<ReceiverState> = RefCell::new(ReceiverState::default());
}

fn checksum(bytes: &[u8], filename: &[u8]) -> u64 {
    // FNV-1a is used as a lightweight corruption detector for this PoC.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in filename.iter().chain(bytes.iter()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn read_u16(bytes: &[u8], offset: &mut usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let value = u16::from_le_bytes(bytes.get(*offset..end)?.try_into().ok()?);
    *offset = end;
    Some(value)
}

fn read_u32(bytes: &[u8], offset: &mut usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let value = u32::from_le_bytes(bytes.get(*offset..end)?.try_into().ok()?);
    *offset = end;
    Some(value)
}

fn read_u64(bytes: &[u8], offset: &mut usize) -> Option<u64> {
    let end = offset.checked_add(8)?;
    let value = u64::from_le_bytes(bytes.get(*offset..end)?.try_into().ok()?);
    *offset = end;
    Some(value)
}

fn reset_state(state: &mut ReceiverState) {
    *state = ReceiverState::default();
}

fn try_assemble(state: &mut ReceiverState) -> Option<Vec<u8>> {
    if state.complete || state.checksum.is_none() || state.received_packets != state.total_packets {
        return None;
    }

    let mut file = Vec::with_capacity(state.file_size);
    for packet in &state.packets {
        file.extend_from_slice(packet.as_ref()?);
    }

    if file.len() != state.file_size
        || checksum(&file, state.filename.as_bytes()) != state.checksum.unwrap()
    {
        return None;
    }

    state.complete = true;
    Some(file)
}

#[wasm_bindgen]
pub fn prepare_file(buffer: &[u8], filename: String) -> Vec<String> {
    let filename_bytes = filename.as_bytes();
    if filename_bytes.len() > MAX_FILENAME_BYTES {
        return Vec::new();
    }

    let total_packets = buffer.len().div_ceil(MAX_DATA_BYTES);
    if total_packets > u32::MAX as usize || buffer.len() > u64::MAX as usize {
        return Vec::new();
    }

    let file_checksum = checksum(buffer, filename_bytes);
    let metadata = HeaderMetadata {
        checksum: file_checksum,
        total_packets: total_packets as u32,
        file_size: buffer.len() as u64,
        filename: filename.clone(),
    };

    let mut header = Vec::with_capacity(27 + filename_bytes.len());
    header.extend_from_slice(HEADER_MAGIC);
    header.extend_from_slice(&metadata.checksum.to_le_bytes());
    header.extend_from_slice(&metadata.total_packets.to_le_bytes());
    header.extend_from_slice(&metadata.file_size.to_le_bytes());
    header.extend_from_slice(&(filename_bytes.len() as u16).to_le_bytes());
    header.extend_from_slice(filename_bytes);

    let mut packets = Vec::with_capacity(total_packets + 1);
    let header_packet = STANDARD.encode(header);
    if header_packet.len() > MAX_PACKET_BASE64_BYTES {
        return Vec::new();
    }
    packets.push(header_packet);

    for (index, chunk) in buffer.chunks(MAX_DATA_BYTES).enumerate() {
        let mut packet = Vec::with_capacity(17 + chunk.len());
        packet.extend_from_slice(DATA_MAGIC);
        packet.extend_from_slice(&file_checksum.to_le_bytes());
        packet.extend_from_slice(&(index as u32).to_le_bytes());
        packet.extend_from_slice(chunk);
        let encoded = STANDARD.encode(packet);
        if encoded.len() > MAX_PACKET_BASE64_BYTES {
            return Vec::new();
        }
        packets.push(encoded);
    }

    packets
}

#[wasm_bindgen]
pub fn reset_receiver() {
    RECEIVER.with(|receiver| reset_state(&mut receiver.borrow_mut()));
}

#[wasm_bindgen]
pub fn process_packet(packet_base64: String) -> Option<Vec<u8>> {
    let packet = STANDARD.decode(packet_base64).ok()?;
    if packet.len() < 5 {
        return None;
    }

    RECEIVER.with(|receiver| {
        let state = &mut *receiver.borrow_mut();

        if packet.starts_with(HEADER_MAGIC) {
            let mut offset = HEADER_MAGIC.len();
            let checksum_value = read_u64(&packet, &mut offset)?;
            let total_packets = read_u32(&packet, &mut offset)? as usize;
            let file_size = usize::try_from(read_u64(&packet, &mut offset)?).ok()?;
            let filename_len = read_u16(&packet, &mut offset)? as usize;
            let filename_end = offset.checked_add(filename_len)?;
            let filename = String::from_utf8(packet.get(offset..filename_end)?.to_vec()).ok()?;

            let is_new_transfer = state.checksum != Some(checksum_value)
                || state.total_packets != total_packets
                || state.file_size != file_size;
            if is_new_transfer {
                let pending = std::mem::take(&mut state.pending);
                reset_state(state);
                state.checksum = Some(checksum_value);
                state.total_packets = total_packets;
                state.file_size = file_size;
                state.filename = filename;
                state.packets = (0..total_packets).map(|_| None).collect();

                for (index, data) in pending {
                    if let Some(slot) = state.packets.get_mut(index as usize) {
                        *slot = Some(data);
                        state.received_packets += 1;
                    }
                }
            }

            return try_assemble(state);
        }

        if !packet.starts_with(DATA_MAGIC) {
            return None;
        }

        let mut offset = DATA_MAGIC.len();
        let checksum_value = read_u64(&packet, &mut offset)?;
        let index = read_u32(&packet, &mut offset)?;
        let data = packet.get(offset..)?.to_vec();

        if state.checksum != Some(checksum_value) {
            if state.checksum.is_none() {
                if state.pending_checksum != Some(checksum_value) {
                    state.pending.clear();
                    state.pending_checksum = Some(checksum_value);
                }
                state.pending.entry(index).or_insert(data);
            }
            return None;
        }

        if let Some(slot) = state.packets.get_mut(index as usize) {
            if slot.is_none() {
                *slot = Some(data);
                state.received_packets += 1;
            }
        }

        try_assemble(state)
    })
}

#[wasm_bindgen]
pub fn receiver_progress() -> Vec<u32> {
    RECEIVER.with(|receiver| {
        let state = receiver.borrow();
        vec![state.received_packets as u32, state.total_packets as u32]
    })
}

#[wasm_bindgen]
pub fn receiver_filename() -> String {
    RECEIVER.with(|receiver| receiver.borrow().filename.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_allows_out_of_order_packets() {
        let input: Vec<u8> = (0..10_000).map(|value| (value % 251) as u8).collect();
        let packets = prepare_file(&input, "foto.bin".to_string());
        assert!(
            packets
                .iter()
                .all(|packet| packet.len() <= MAX_PACKET_BASE64_BYTES)
        );

        reset_receiver();
        for packet in packets.iter().skip(1).rev() {
            assert!(process_packet(packet.clone()).is_none());
        }
        let output = process_packet(packets[0].clone()).expect("header completes transfer");
        assert_eq!(output, input);
        assert_eq!(receiver_filename(), "foto.bin");
        assert_eq!(
            receiver_progress().to_vec(),
            vec![packets.len() as u32 - 1, packets.len() as u32 - 1]
        );
    }

    #[test]
    fn duplicate_packets_are_ignored() {
        let input = vec![42u8; MAX_DATA_BYTES + 5];
        let packets = prepare_file(&input, "repetido.dat".to_string());
        reset_receiver();
        assert!(process_packet(packets[0].clone()).is_none());
        assert!(process_packet(packets[1].clone()).is_none());
        assert!(process_packet(packets[1].clone()).is_none());
        let output = process_packet(packets[2].clone()).expect("all data packets received");
        assert_eq!(output, input);
    }
}
