use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::{Compression, read::DeflateDecoder, write::DeflateEncoder};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use wasm_bindgen::prelude::*;

// The QR payload is selected at runtime by the frontend. These bounds protect
// the receiver while allowing a device-specific speed/readability trade-off.
const DEFAULT_DATA_CHUNK_CHARS: usize = 600;
const MIN_DATA_CHUNK_CHARS: usize = 300;
const MAX_DATA_CHUNK_CHARS: usize = 1_500;
const MAX_FILENAME_BYTES: usize = 255;
const MAX_FILE_BYTES: usize = 1_500 * 1024;
// A 1.5 MiB incompressible file expands by roughly 4/3 in Base64. Keep a
// bounded headroom for Deflate framing and reject hostile, unbounded headers.
const MAX_TOTAL_PACKETS: usize = 10_000;

#[derive(Clone, Debug)]
struct HeaderMetadata {
    checksum: u64,
    total_packets: usize,
    file_size: usize,
    filename: String,
}

#[derive(Default)]
struct ReceiverState {
    checksum: Option<u64>,
    total_packets: usize,
    file_size: usize,
    filename: String,
    chunks: HashMap<usize, String>,
    pending: HashMap<usize, String>,
    pending_checksum: Option<u64>,
    received_packets: usize,
    complete: bool,
}

#[derive(Serialize)]
struct ReceiverInfo {
    filename: String,
    total_packets: usize,
    file_size: usize,
    checksum: Option<String>,
    received_packets: usize,
    missing_packets: usize,
    complete: bool,
}

thread_local! {
    static RECEIVER: RefCell<ReceiverState> = RefCell::new(ReceiverState::default());
}

fn checksum(bytes: &[u8], filename: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in filename.iter().chain(bytes.iter()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn compress_file(buffer: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(6));
    encoder.write_all(buffer).ok()?;
    encoder.finish().ok()
}

fn decompress_file(buffer: &[u8]) -> Option<Vec<u8>> {
    let mut decoder = DeflateDecoder::new(buffer);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).ok()?;
    Some(output)
}

fn parse_checksum(value: &str) -> Option<u64> {
    u64::from_str_radix(value, 16).ok()
}

fn parse_header(packet: &str) -> Option<HeaderMetadata> {
    let fields: Vec<&str> = packet.split('|').collect();
    if fields.len() != 5 || fields[0] != "METADATA" {
        return None;
    }

    let filename = fields[1].to_string();
    if filename.is_empty()
        || filename.as_bytes().len() > MAX_FILENAME_BYTES
        || filename.chars().any(|character| character.is_control())
    {
        return None;
    }

    let total_packets = fields[2].parse::<usize>().ok()?;
    let checksum = parse_checksum(fields[3])?;
    let file_size = fields[4].parse::<usize>().ok()?;
    if total_packets == 0 || total_packets > MAX_TOTAL_PACKETS || file_size > MAX_FILE_BYTES {
        return None;
    }

    Some(HeaderMetadata {
        checksum,
        total_packets,
        file_size,
        filename,
    })
}

fn reset_state(state: &mut ReceiverState) {
    *state = ReceiverState::default();
}

fn try_assemble(state: &mut ReceiverState) -> Option<Vec<u8>> {
    if state.complete || state.checksum.is_none() || state.received_packets != state.total_packets {
        return None;
    }

    let mut encoded = String::new();
    for index in 0..state.total_packets {
        encoded.push_str(state.chunks.get(&index)?);
    }

    let compressed = STANDARD.decode(encoded).ok()?;
    let file = decompress_file(&compressed)?;
    if file.len() != state.file_size
        || checksum(&file, state.filename.as_bytes()) != state.checksum?
    {
        return None;
    }

    state.complete = true;
    Some(file)
}

fn install_header(state: &mut ReceiverState, header: HeaderMetadata) -> Option<Vec<u8>> {
    let is_new_transfer = state.checksum != Some(header.checksum)
        || state.total_packets != header.total_packets
        || state.file_size != header.file_size;

    if is_new_transfer {
        let pending_checksum = state.pending_checksum;
        let pending = std::mem::take(&mut state.pending);
        reset_state(state);
        state.checksum = Some(header.checksum);
        state.total_packets = header.total_packets;
        state.file_size = header.file_size;
        state.filename = header.filename;

        if pending_checksum == Some(header.checksum) {
            for (index, chunk) in pending {
                if index < state.total_packets && state.chunks.insert(index, chunk).is_none() {
                    state.received_packets += 1;
                }
            }
        }
    }

    try_assemble(state)
}

#[wasm_bindgen]
pub fn compress_and_split(buffer: Vec<u8>, filename: String) -> Vec<String> {
    compress_and_split_with_chunk_size(buffer, filename, DEFAULT_DATA_CHUNK_CHARS as u32)
}

#[wasm_bindgen]
pub fn compress_and_split_with_chunk_size(
    buffer: Vec<u8>,
    filename: String,
    chunk_chars: u32,
) -> Vec<String> {
    let chunk_chars = chunk_chars as usize;
    if !(MIN_DATA_CHUNK_CHARS..=MAX_DATA_CHUNK_CHARS).contains(&chunk_chars) {
        return Vec::new();
    }

    if buffer.len() > MAX_FILE_BYTES
        || filename.is_empty()
        || filename.as_bytes().len() > MAX_FILENAME_BYTES
        || filename
            .chars()
            .any(|character| character == '|' || character.is_control())
    {
        return Vec::new();
    }

    let compressed = match compress_file(&buffer) {
        Some(compressed) => compressed,
        None => return Vec::new(),
    };
    let encoded = STANDARD.encode(compressed);
    let total_packets = encoded.len().div_ceil(chunk_chars);
    let file_checksum = checksum(&buffer, filename.as_bytes());

    let header = format!(
        "METADATA|{}|{}|{:016x}|{}",
        filename,
        total_packets,
        file_checksum,
        buffer.len()
    );
    let mut packets = Vec::with_capacity(total_packets + 1);
    packets.push(header);

    for (index, chunk) in encoded.as_bytes().chunks(chunk_chars).enumerate() {
        let chunk = std::str::from_utf8(chunk).expect("Base64 is always valid UTF-8");
        packets.push(format!("DATA|{:016x}|{}|{}", file_checksum, index, chunk));
    }

    packets
}

// Backwards-compatible alias for clients built against the previous PoC API.
#[wasm_bindgen]
pub fn prepare_file(buffer: &[u8], filename: String) -> Vec<String> {
    compress_and_split(buffer.to_vec(), filename)
}

#[wasm_bindgen]
pub fn reset_receiver() {
    RECEIVER.with(|receiver| reset_state(&mut receiver.borrow_mut()));
}

#[wasm_bindgen]
pub fn process_packet(packet: String) -> Option<Vec<u8>> {
    RECEIVER.with(|receiver| {
        let state = &mut *receiver.borrow_mut();

        if packet.starts_with("METADATA|") {
            return install_header(state, parse_header(&packet)?);
        }

        let fields: Vec<&str> = packet.splitn(4, '|').collect();
        if fields.len() != 4 || fields[0] != "DATA" {
            return None;
        }

        let checksum_value = parse_checksum(fields[1])?;
        let index = fields[2].parse::<usize>().ok()?;
        let chunk = fields[3];
        if chunk.is_empty() || chunk.len() > MAX_DATA_CHUNK_CHARS {
            return None;
        }

        if state.checksum != Some(checksum_value) {
            if state.checksum.is_none() {
                if state.pending_checksum != Some(checksum_value) {
                    state.pending.clear();
                    state.pending_checksum = Some(checksum_value);
                }
                state
                    .pending
                    .entry(index)
                    .or_insert_with(|| chunk.to_string());
            }
            return None;
        }

        if index < state.total_packets && state.chunks.insert(index, chunk.to_string()).is_none() {
            state.received_packets += 1;
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

fn missing_ranges(state: &ReceiverState) -> String {
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < state.total_packets {
        if state.chunks.contains_key(&index) {
            index += 1;
            continue;
        }

        let start = index;
        while index < state.total_packets && !state.chunks.contains_key(&index) {
            index += 1;
        }
        let end = index - 1;
        if start == end {
            ranges.push(start.to_string());
        } else {
            ranges.push(format!("{start}-{end}"));
        }
    }
    ranges.join(",")
}

#[wasm_bindgen]
pub fn receiver_info() -> String {
    RECEIVER.with(|receiver| {
        let state = receiver.borrow();
        let missing_packets = state.total_packets.saturating_sub(state.received_packets);
        let info = ReceiverInfo {
            filename: state.filename.clone(),
            total_packets: state.total_packets,
            file_size: state.file_size,
            checksum: state.checksum.map(|value| format!("{value:016x}")),
            received_packets: state.received_packets,
            missing_packets,
            complete: state.complete,
        };
        serde_json::to_string(&info).unwrap_or_else(|_| "{}".to_string())
    })
}

#[wasm_bindgen]
pub fn receiver_packet_map() -> Vec<u8> {
    RECEIVER.with(|receiver| {
        let state = receiver.borrow();
        (0..state.total_packets)
            .map(|index| u8::from(state.chunks.contains_key(&index)))
            .collect()
    })
}

#[wasm_bindgen]
pub fn receiver_missing_ranges() -> String {
    RECEIVER.with(|receiver| missing_ranges(&receiver.borrow()))
}

#[wasm_bindgen]
pub fn receiver_control_packet() -> String {
    RECEIVER.with(|receiver| {
        let state = receiver.borrow();
        let Some(checksum) = state.checksum else {
            return String::new();
        };
        format!("REQUEST|{checksum:016x}|{}", missing_ranges(&state))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_round_trip_allows_out_of_order_packets() {
        let input: Vec<u8> = (0..10_000).map(|value| (value % 251) as u8).collect();
        let packets =
            compress_and_split_with_chunk_size(input.clone(), "foto.bin".to_string(), 600);

        reset_receiver();
        for packet in packets.iter().skip(1).rev() {
            assert!(process_packet(packet.clone()).is_none());
        }
        let output = process_packet(packets[0].clone()).expect("transfer should complete");
        assert_eq!(output, input);
        assert_eq!(receiver_filename(), "foto.bin");
        assert_eq!(
            receiver_progress().to_vec(),
            vec![packets.len() as u32 - 1; 2]
        );
    }

    #[test]
    fn duplicate_packets_are_ignored() {
        let mut seed = 0x1234_5678u32;
        let input: Vec<u8> = (0..5_000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            })
            .collect();
        let packets =
            compress_and_split_with_chunk_size(input.clone(), "repetido.dat".to_string(), 600);
        reset_receiver();
        assert!(process_packet(packets[0].clone()).is_none());
        assert!(process_packet(packets[1].clone()).is_none());
        assert!(process_packet(packets[1].clone()).is_none());
        let mut output = None;
        for packet in packets.iter().skip(2) {
            output = process_packet(packet.clone());
        }
        let output = output.expect("all data packets received");
        assert_eq!(output, input);
    }

    #[test]
    fn data_packets_can_arrive_before_metadata() {
        let input = vec![7u8; 2_000];
        let packets =
            compress_and_split_with_chunk_size(input.clone(), "antes.bin".to_string(), 600);
        reset_receiver();
        for packet in packets.iter().skip(1) {
            assert!(process_packet(packet.clone()).is_none());
        }
        let output = process_packet(packets[0].clone()).expect("pending data should attach");
        assert_eq!(output, input);
    }

    #[test]
    fn receiver_exposes_metadata_map_and_missing_ranges() {
        let mut seed = 0x9e37_79b9u32;
        let input: Vec<u8> = (0..12_000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            })
            .collect();
        let packets = compress_and_split_with_chunk_size(input, "imagen.tar.gz".to_string(), 300);
        reset_receiver();
        process_packet(packets[0].clone());
        process_packet(packets[1].clone());
        process_packet(packets[3].clone());

        let map = receiver_packet_map();
        assert_eq!(map[0], 1);
        assert_eq!(map[1], 0);
        assert_eq!(map[2], 1);
        assert!(receiver_missing_ranges().contains('1'));
        assert!(receiver_control_packet().starts_with("REQUEST|"));
        assert!(receiver_info().contains("imagen.tar.gz"));
    }
}
