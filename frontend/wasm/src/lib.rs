use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::{Compression, read::DeflateDecoder, write::DeflateEncoder};
use serde::Serialize;
use sha2::{Digest, Sha256};
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
        || filename.len() > MAX_FILENAME_BYTES
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
        || filename.len() > MAX_FILENAME_BYTES
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

// Binary QR protocol. Unlike the legacy text protocol above, this format
// sends compressed bytes directly in QR byte mode, avoiding Base64 overhead.
const BINARY_MAGIC: [u8; 3] = *b"TN2";
const BINARY_HEADER_TYPE: u8 = 0;
const BINARY_DATA_TYPE: u8 = 1;
const BINARY_PARITY_TYPE: u8 = 2;
// Header layout: magic(3), type(1), codec(1), FEC(1), flags(1),
// FNV transfer id(8), SHA-256(32), file size(4), compressed size(4),
// chunk size(2), data packet count(4), filename length(2).
const BINARY_HEADER_FIXED_BYTES: usize = 63;
const BINARY_DATA_FIXED_BYTES: usize = 16;
const BINARY_MAX_CHUNK_BYTES: usize = 1_800;
const BINARY_MIN_CHUNK_BYTES: usize = 400;
const BINARY_MAX_PENDING_PACKETS: usize = 10_000;

#[derive(Clone, Debug)]
struct BinaryHeader {
    codec: u8,
    checksum: u64,
    digest: [u8; 32],
    file_size: usize,
    compressed_size: usize,
    chunk_size: usize,
    data_packets: usize,
    fec_group_size: usize,
    filename: String,
}

#[derive(Default)]
struct BinaryReceiverState {
    header: Option<BinaryHeader>,
    chunks: HashMap<usize, Vec<u8>>,
    parity: HashMap<usize, Vec<u8>>,
    pending: Vec<Vec<u8>>,
    received_packets: usize,
    recovered_packets: usize,
    complete: bool,
}

#[derive(Serialize)]
struct BinaryReceiverInfo {
    filename: String,
    total_packets: usize,
    received_packets: usize,
    missing_packets: usize,
    file_size: usize,
    compressed_size: usize,
    checksum: Option<String>,
    sha256: Option<String>,
    codec: String,
    fec_group_size: usize,
    recovered_packets: usize,
    complete: bool,
}

thread_local! {
    static BINARY_RECEIVER: RefCell<BinaryReceiverState> = RefCell::new(BinaryReceiverState::default());
}

fn push_u16(target: &mut Vec<u8>, value: usize) {
    target.extend_from_slice(&(value as u16).to_be_bytes());
}

fn push_u32(target: &mut Vec<u8>, value: usize) {
    target.extend_from_slice(&(value as u32).to_be_bytes());
}

fn read_u16(source: &[u8], offset: usize) -> Option<usize> {
    Some(u16::from_be_bytes(source.get(offset..offset + 2)?.try_into().ok()?) as usize)
}

fn read_u32(source: &[u8], offset: usize) -> Option<usize> {
    Some(u32::from_be_bytes(source.get(offset..offset + 4)?.try_into().ok()?) as usize)
}

fn read_u64(source: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        source.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn binary_compress(buffer: &[u8]) -> Option<(u8, Vec<u8>)> {
    let compressed = compress_file(buffer)?;
    if compressed.len() < buffer.len() {
        Some((1, compressed))
    } else {
        Some((0, buffer.to_vec()))
    }
}

fn file_digest(bytes: &[u8], filename: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(filename);
    digest.update(bytes);
    digest.finalize().into()
}

fn bytes_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn binary_header_packet(header: &BinaryHeader) -> Vec<u8> {
    let filename = header.filename.as_bytes();
    let mut packet = Vec::with_capacity(BINARY_HEADER_FIXED_BYTES + filename.len());
    packet.extend_from_slice(&BINARY_MAGIC);
    packet.push(BINARY_HEADER_TYPE);
    packet.push(header.codec);
    packet.push(header.fec_group_size as u8);
    packet.push(0); // reserved flags
    packet.extend_from_slice(&header.checksum.to_be_bytes());
    packet.extend_from_slice(&header.digest);
    push_u32(&mut packet, header.file_size);
    push_u32(&mut packet, header.compressed_size);
    push_u16(&mut packet, header.chunk_size);
    push_u32(&mut packet, header.data_packets);
    push_u16(&mut packet, filename.len());
    packet.extend_from_slice(filename);
    packet
}

fn build_binary_packets(
    buffer: &[u8],
    filename: &str,
    chunk_size: usize,
    fec_group_size: usize,
) -> Option<Vec<Vec<u8>>> {
    if !(BINARY_MIN_CHUNK_BYTES..=BINARY_MAX_CHUNK_BYTES).contains(&chunk_size)
        || !matches!(fec_group_size, 0 | 8)
        || buffer.len() > MAX_FILE_BYTES
        || filename.is_empty()
        || filename.len() > MAX_FILENAME_BYTES
        || filename.chars().any(|character| character.is_control())
    {
        return None;
    }

    let (codec, encoded) = binary_compress(buffer)?;
    let data_packets = encoded.len().max(1).div_ceil(chunk_size);
    let checksum = checksum(buffer, filename.as_bytes());
    let digest = file_digest(buffer, filename.as_bytes());
    let header = BinaryHeader {
        codec,
        checksum,
        digest,
        file_size: buffer.len(),
        compressed_size: encoded.len(),
        chunk_size,
        data_packets,
        fec_group_size,
        filename: filename.to_string(),
    };

    let mut padded_chunks = Vec::with_capacity(data_packets);
    for index in 0..data_packets {
        let start = index * chunk_size;
        let end = (start + chunk_size).min(encoded.len());
        let mut chunk = vec![0u8; chunk_size];
        if start < end {
            chunk[..end - start].copy_from_slice(&encoded[start..end]);
        }
        padded_chunks.push(chunk);
    }

    let mut packets = vec![binary_header_packet(&header)];
    for (index, chunk) in padded_chunks.iter().enumerate() {
        let start = index * chunk_size;
        let payload_len = encoded.len().saturating_sub(start).min(chunk_size);
        let mut packet = Vec::with_capacity(BINARY_DATA_FIXED_BYTES + payload_len);
        packet.extend_from_slice(&BINARY_MAGIC);
        packet.push(BINARY_DATA_TYPE);
        packet.extend_from_slice(&checksum.to_be_bytes());
        push_u32(&mut packet, index);
        packet.extend_from_slice(&chunk[..payload_len]);
        packets.push(packet);
    }

    if fec_group_size > 0 {
        for group in 0..data_packets.div_ceil(fec_group_size) {
            let start = group * fec_group_size;
            let end = (start + fec_group_size).min(data_packets);
            let mut parity = vec![0u8; chunk_size];
            for chunk in &padded_chunks[start..end] {
                for (position, byte) in chunk.iter().enumerate() {
                    parity[position] ^= byte;
                }
            }
            let mut packet = Vec::with_capacity(BINARY_DATA_FIXED_BYTES + chunk_size);
            packet.extend_from_slice(&BINARY_MAGIC);
            packet.push(BINARY_PARITY_TYPE);
            packet.extend_from_slice(&checksum.to_be_bytes());
            push_u32(&mut packet, group);
            packet.extend_from_slice(&parity);
            packets.push(packet);
        }
    }

    Some(packets)
}

#[wasm_bindgen]
pub fn prepare_binary_packets(
    buffer: Vec<u8>,
    filename: String,
    chunk_bytes: u32,
    fec_group_size: u32,
) -> JsValue {
    let Some(packets) = build_binary_packets(
        &buffer,
        &filename,
        chunk_bytes as usize,
        fec_group_size as usize,
    ) else {
        return js_sys::Array::new().into();
    };

    let output = js_sys::Array::new();
    for packet in packets {
        output.push(&js_sys::Uint8Array::from(packet.as_slice()));
    }
    output.into()
}

fn parse_binary_header(packet: &[u8]) -> Option<BinaryHeader> {
    if packet.len() < BINARY_HEADER_FIXED_BYTES
        || packet.get(..3)? != BINARY_MAGIC
        || packet[3] != BINARY_HEADER_TYPE
    {
        return None;
    }
    let codec = packet[4];
    let fec_group_size = packet[5] as usize;
    let checksum = read_u64(packet, 7)?;
    let digest: [u8; 32] = packet.get(15..47)?.try_into().ok()?;
    let file_size = read_u32(packet, 47)?;
    let compressed_size = read_u32(packet, 51)?;
    let chunk_size = read_u16(packet, 55)?;
    let data_packets = read_u32(packet, 57)?;
    let filename_len = read_u16(packet, 61)?;
    let filename_start = BINARY_HEADER_FIXED_BYTES;
    let filename_end = filename_start.checked_add(filename_len)?;
    let filename = String::from_utf8(packet.get(filename_start..filename_end)?.to_vec()).ok()?;

    if codec > 1
        || !matches!(fec_group_size, 0 | 8)
        || !(BINARY_MIN_CHUNK_BYTES..=BINARY_MAX_CHUNK_BYTES).contains(&chunk_size)
        || data_packets == 0
        || data_packets > BINARY_MAX_PENDING_PACKETS
        || file_size > MAX_FILE_BYTES
        || compressed_size > MAX_FILE_BYTES * 2
        || filename.is_empty()
        || filename.len() > MAX_FILENAME_BYTES
        || filename.chars().any(|character| character.is_control())
    {
        return None;
    }

    let expected_packets = compressed_size.max(1).div_ceil(chunk_size);
    if data_packets != expected_packets {
        return None;
    }

    Some(BinaryHeader {
        codec,
        checksum,
        digest,
        file_size,
        compressed_size,
        chunk_size,
        data_packets,
        fec_group_size,
        filename,
    })
}

fn binary_missing_ranges(state: &BinaryReceiverState) -> String {
    let Some(header) = state.header.as_ref() else {
        return String::new();
    };
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < header.data_packets {
        if state.chunks.contains_key(&index) {
            index += 1;
            continue;
        }
        let start = index;
        while index < header.data_packets && !state.chunks.contains_key(&index) {
            index += 1;
        }
        let end = index - 1;
        ranges.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
    }
    ranges.join(",")
}

fn binary_expected_len(header: &BinaryHeader, index: usize) -> usize {
    header
        .compressed_size
        .saturating_sub(index * header.chunk_size)
        .min(header.chunk_size)
}

fn try_binary_recover(state: &mut BinaryReceiverState) {
    let Some(header) = state.header.as_ref() else {
        return;
    };
    if header.fec_group_size == 0 {
        return;
    }

    let group_count = header.data_packets.div_ceil(header.fec_group_size);
    for group in 0..group_count {
        let Some(parity) = state.parity.get(&group).cloned() else {
            continue;
        };
        let start = group * header.fec_group_size;
        let end = (start + header.fec_group_size).min(header.data_packets);
        let missing: Vec<usize> = (start..end)
            .filter(|index| !state.chunks.contains_key(index))
            .collect();
        if missing.len() != 1 {
            continue;
        }

        let missing_index = missing[0];
        let mut recovered = parity;
        for index in start..end {
            if index == missing_index {
                continue;
            }
            if let Some(chunk) = state.chunks.get(&index) {
                for (position, byte) in chunk.iter().enumerate() {
                    recovered[position] ^= byte;
                }
            }
        }
        if state.chunks.insert(missing_index, recovered).is_none() {
            state.received_packets += 1;
            state.recovered_packets += 1;
        }
    }
}

fn try_binary_assemble(state: &mut BinaryReceiverState) -> Option<Vec<u8>> {
    let header = state.header.as_ref()?;
    if state.complete || state.received_packets != header.data_packets {
        return None;
    }

    let mut compressed = Vec::with_capacity(header.compressed_size);
    for index in 0..header.data_packets {
        compressed.extend_from_slice(state.chunks.get(&index)?);
    }
    compressed.truncate(header.compressed_size);
    let file = if header.codec == 1 {
        decompress_file(&compressed)?
    } else {
        compressed
    };
    if file.len() != header.file_size
        || checksum(&file, header.filename.as_bytes()) != header.checksum
        || file_digest(&file, header.filename.as_bytes()) != header.digest
    {
        return None;
    }
    state.complete = true;
    Some(file)
}

fn install_binary_header(state: &mut BinaryReceiverState, header: BinaryHeader) -> Option<Vec<u8>> {
    let is_new_transfer = state
        .header
        .as_ref()
        .is_none_or(|current| current.checksum != header.checksum);
    let pending = if is_new_transfer {
        let pending = std::mem::take(&mut state.pending);
        *state = BinaryReceiverState {
            header: Some(header),
            ..BinaryReceiverState::default()
        };
        pending
    } else {
        Vec::new()
    };

    let mut assembled = None;
    for pending_packet in pending {
        assembled = process_binary_inner(state, &pending_packet, false).or(assembled);
    }
    assembled.or_else(|| try_binary_assemble(state))
}

fn process_binary_inner(
    state: &mut BinaryReceiverState,
    packet: &[u8],
    allow_pending: bool,
) -> Option<Vec<u8>> {
    if packet.len() >= 4 && packet.get(..3) == Some(&BINARY_MAGIC) {
        match packet[3] {
            BINARY_HEADER_TYPE => {
                return install_binary_header(state, parse_binary_header(packet)?);
            }
            BINARY_DATA_TYPE => {
                let checksum_value = read_u64(packet, 4)?;
                let index = read_u32(packet, 12)?;
                let Some(header) = state.header.as_ref() else {
                    if allow_pending && state.pending.len() < BINARY_MAX_PENDING_PACKETS {
                        state.pending.push(packet.to_vec());
                    }
                    return None;
                };
                if checksum_value != header.checksum
                    || index >= header.data_packets
                    || packet.len() != BINARY_DATA_FIXED_BYTES + binary_expected_len(header, index)
                {
                    return None;
                }
                let mut chunk = vec![0u8; header.chunk_size];
                chunk[..packet.len() - BINARY_DATA_FIXED_BYTES]
                    .copy_from_slice(&packet[BINARY_DATA_FIXED_BYTES..]);
                if state.chunks.insert(index, chunk).is_none() {
                    state.received_packets += 1;
                }
            }
            BINARY_PARITY_TYPE => {
                let checksum_value = read_u64(packet, 4)?;
                let group = read_u32(packet, 12)?;
                let Some(header) = state.header.as_ref() else {
                    if allow_pending && state.pending.len() < BINARY_MAX_PENDING_PACKETS {
                        state.pending.push(packet.to_vec());
                    }
                    return None;
                };
                let group_count = header.data_packets.div_ceil(header.fec_group_size.max(1));
                if checksum_value != header.checksum
                    || header.fec_group_size == 0
                    || group >= group_count
                    || packet.len() != BINARY_DATA_FIXED_BYTES + header.chunk_size
                {
                    return None;
                }
                state
                    .parity
                    .entry(group)
                    .or_insert_with(|| packet[BINARY_DATA_FIXED_BYTES..].to_vec());
            }
            _ => return None,
        }
    } else if allow_pending && state.header.is_none() {
        if state.pending.len() < BINARY_MAX_PENDING_PACKETS {
            state.pending.push(packet.to_vec());
        }
        return None;
    } else {
        return None;
    }

    try_binary_recover(state);
    try_binary_assemble(state)
}

#[wasm_bindgen]
pub fn reset_binary_receiver() {
    BINARY_RECEIVER.with(|receiver| *receiver.borrow_mut() = BinaryReceiverState::default());
}

#[wasm_bindgen]
pub fn process_binary_packet(packet: Vec<u8>) -> Option<Vec<u8>> {
    BINARY_RECEIVER.with(|receiver| process_binary_inner(&mut receiver.borrow_mut(), &packet, true))
}

#[wasm_bindgen]
pub fn binary_receiver_progress() -> Vec<u32> {
    BINARY_RECEIVER.with(|receiver| {
        let state = receiver.borrow();
        let total = state
            .header
            .as_ref()
            .map_or(0, |header| header.data_packets);
        vec![state.received_packets as u32, total as u32]
    })
}

#[wasm_bindgen]
pub fn binary_receiver_filename() -> String {
    BINARY_RECEIVER.with(|receiver| {
        receiver
            .borrow()
            .header
            .as_ref()
            .map_or_else(String::new, |header| header.filename.clone())
    })
}

#[wasm_bindgen]
pub fn binary_receiver_packet_map() -> Vec<u8> {
    BINARY_RECEIVER.with(|receiver| {
        let state = receiver.borrow();
        let total = state
            .header
            .as_ref()
            .map_or(0, |header| header.data_packets);
        (0..total)
            .map(|index| u8::from(state.chunks.contains_key(&index)))
            .collect()
    })
}

#[wasm_bindgen]
pub fn binary_receiver_missing_ranges() -> String {
    BINARY_RECEIVER.with(|receiver| binary_missing_ranges(&receiver.borrow()))
}

#[wasm_bindgen]
pub fn binary_receiver_control_packet() -> String {
    BINARY_RECEIVER.with(|receiver| {
        let state = receiver.borrow();
        let Some(header) = state.header.as_ref() else {
            return String::new();
        };
        format!(
            "REQUEST|{:016x}|{}",
            header.checksum,
            binary_missing_ranges(&state)
        )
    })
}

#[wasm_bindgen]
pub fn binary_receiver_info() -> String {
    BINARY_RECEIVER.with(|receiver| {
        let state = receiver.borrow();
        let Some(header) = state.header.as_ref() else {
            return "{}".to_string();
        };
        let info = BinaryReceiverInfo {
            filename: header.filename.clone(),
            total_packets: header.data_packets,
            received_packets: state.received_packets,
            missing_packets: header.data_packets.saturating_sub(state.received_packets),
            file_size: header.file_size,
            compressed_size: header.compressed_size,
            checksum: Some(format!("{:016x}", header.checksum)),
            sha256: Some(bytes_hex(&header.digest)),
            codec: if header.codec == 1 { "deflate" } else { "raw" }.to_string(),
            fec_group_size: header.fec_group_size,
            recovered_packets: state.recovered_packets,
            complete: state.complete,
        };
        serde_json::to_string(&info).unwrap_or_else(|_| "{}".to_string())
    })
}

// Windowed LT fountain transport. It keeps source packets systematic and adds
// XOR equations over windows of 32 chunks. This avoids a large global matrix
// while allowing the receiver to recover several losses without a NACK.
const FOUNTAIN_MAGIC: [u8; 3] = *b"TNF";
const FOUNTAIN_HEADER_TYPE: u8 = 0;
const FOUNTAIN_DATA_TYPE: u8 = 1;
const FOUNTAIN_REPAIR_TYPE: u8 = 2;
const FOUNTAIN_HEADER_FIXED_BYTES: usize = 68;
const FOUNTAIN_DATA_FIXED_BYTES: usize = 16;
const FOUNTAIN_REPAIR_FIXED_BYTES: usize = 18;
const FOUNTAIN_WINDOW_SIZE: usize = 32;
const FOUNTAIN_MAX_OVERHEAD_PERCENT: usize = 50;

#[derive(Clone, Debug)]
struct FountainHeader {
    codec: u8,
    checksum: u64,
    digest: [u8; 32],
    file_size: usize,
    compressed_size: usize,
    chunk_size: usize,
    data_packets: usize,
    overhead_percent: usize,
    repair_packets: usize,
    filename: String,
}

#[derive(Clone)]
struct FountainEquation {
    mask: u32,
    payload: Vec<u8>,
}

struct FountainWindow {
    width: usize,
    basis: Vec<Option<FountainEquation>>,
    chunks: Vec<Option<Vec<u8>>>,
}

impl FountainWindow {
    fn new(width: usize) -> Self {
        Self {
            width,
            basis: (0..width).map(|_| None).collect(),
            chunks: (0..width).map(|_| None).collect(),
        }
    }

    fn add_equation(&mut self, mut equation: FountainEquation) -> bool {
        for pivot in 0..self.width {
            if equation.mask & (1u32 << pivot) == 0 {
                continue;
            }
            let Some(existing) = self.basis[pivot].as_ref() else {
                self.basis[pivot] = Some(equation);
                return true;
            };
            equation.mask ^= existing.mask;
            for (left, right) in equation.payload.iter_mut().zip(&existing.payload) {
                *left ^= right;
            }
            if equation.mask == 0 {
                return false;
            }
        }
        false
    }

    fn solve(&mut self) -> usize {
        let mut solved = 0;
        for pivot in (0..self.width).rev() {
            let Some(equation) = self.basis[pivot].as_ref() else {
                continue;
            };
            let mut payload = equation.payload.clone();
            let mut resolvable = true;
            for bit in (pivot + 1)..self.width {
                if equation.mask & (1u32 << bit) == 0 {
                    continue;
                }
                let Some(chunk) = self.chunks[bit].as_ref() else {
                    resolvable = false;
                    break;
                };
                for (left, right) in payload.iter_mut().zip(chunk) {
                    *left ^= right;
                }
            }
            if resolvable && self.chunks[pivot].is_none() {
                self.chunks[pivot] = Some(payload);
                solved += 1;
            }
        }
        solved
    }
}

#[derive(Default)]
struct FountainReceiverState {
    header: Option<FountainHeader>,
    windows: HashMap<usize, FountainWindow>,
    pending: Vec<Vec<u8>>,
    seen: HashMap<(u8, usize, u32), bool>,
    received_symbols: usize,
    received_data: HashMap<usize, bool>,
    recovered_packets: usize,
    complete: bool,
}

#[derive(Serialize)]
struct FountainReceiverInfo {
    strategy: String,
    filename: String,
    total_packets: usize,
    received_packets: usize,
    missing_packets: usize,
    file_size: usize,
    compressed_size: usize,
    checksum: Option<String>,
    sha256: Option<String>,
    codec: String,
    overhead_percent: usize,
    repair_packets: usize,
    received_symbols: usize,
    recovered_packets: usize,
    complete: bool,
}

thread_local! {
    static FOUNTAIN_RECEIVER: RefCell<FountainReceiverState> = RefCell::new(FountainReceiverState::default());
}

fn next_fountain_random(seed: &mut u32) -> u32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    *seed
}

fn fountain_mask(seed: u32, width: usize, repair_index: usize) -> u32 {
    let mut state = seed
        .wrapping_add((repair_index as u32).wrapping_mul(0x9e37_79b9))
        .max(1);
    let mut mask = 0u32;
    // Dense random equations are deliberate here: each window is capped at
    // 32 symbols, so this keeps the XOR decoder small while making several
    // missing source symbols recoverable with a modest overhead.
    while mask == 0 {
        for bit in 0..width {
            if next_fountain_random(&mut state) & 1 == 1 {
                mask |= 1u32 << bit;
            }
        }
    }
    mask
}

fn fountain_header_packet(header: &FountainHeader) -> Vec<u8> {
    let filename = header.filename.as_bytes();
    let mut packet = Vec::with_capacity(FOUNTAIN_HEADER_FIXED_BYTES + filename.len());
    packet.extend_from_slice(&FOUNTAIN_MAGIC);
    packet.push(FOUNTAIN_HEADER_TYPE);
    packet.push(header.codec);
    packet.push(FOUNTAIN_WINDOW_SIZE as u8);
    packet.push(0);
    packet.extend_from_slice(&header.checksum.to_be_bytes());
    packet.extend_from_slice(&header.digest);
    push_u32(&mut packet, header.file_size);
    push_u32(&mut packet, header.compressed_size);
    push_u16(&mut packet, header.chunk_size);
    push_u32(&mut packet, header.data_packets);
    packet.push(header.overhead_percent as u8);
    push_u32(&mut packet, header.repair_packets);
    push_u16(&mut packet, filename.len());
    packet.extend_from_slice(filename);
    packet
}

fn build_fountain_packets(
    buffer: &[u8],
    filename: &str,
    chunk_size: usize,
    overhead_percent: usize,
) -> Option<Vec<Vec<u8>>> {
    if !(BINARY_MIN_CHUNK_BYTES..=BINARY_MAX_CHUNK_BYTES).contains(&chunk_size)
        || overhead_percent > FOUNTAIN_MAX_OVERHEAD_PERCENT
        || buffer.len() > MAX_FILE_BYTES
        || filename.is_empty()
        || filename.len() > MAX_FILENAME_BYTES
        || filename.chars().any(|character| character.is_control())
    {
        return None;
    }

    let (codec, encoded) = binary_compress(buffer)?;
    let data_packets = encoded.len().max(1).div_ceil(chunk_size);
    let checksum = checksum(buffer, filename.as_bytes());
    let digest = file_digest(buffer, filename.as_bytes());
    let window_count = data_packets.div_ceil(FOUNTAIN_WINDOW_SIZE);
    let mut repair_packets = 0;
    for window in 0..window_count {
        let width = (data_packets - window * FOUNTAIN_WINDOW_SIZE).min(FOUNTAIN_WINDOW_SIZE);
        if overhead_percent > 0 {
            repair_packets += (width * overhead_percent).div_ceil(100).max(1);
        }
    }
    let header = FountainHeader {
        codec,
        checksum,
        digest,
        file_size: buffer.len(),
        compressed_size: encoded.len(),
        chunk_size,
        data_packets,
        overhead_percent,
        repair_packets,
        filename: filename.to_string(),
    };

    let mut padded_chunks = Vec::with_capacity(data_packets);
    for index in 0..data_packets {
        let start = index * chunk_size;
        let end = (start + chunk_size).min(encoded.len());
        let mut chunk = vec![0u8; chunk_size];
        if start < end {
            chunk[..end - start].copy_from_slice(&encoded[start..end]);
        }
        padded_chunks.push(chunk);
    }

    let mut packets = vec![fountain_header_packet(&header)];
    for (index, chunk) in padded_chunks.iter().enumerate() {
        let start = index * chunk_size;
        let payload_len = encoded.len().saturating_sub(start).min(chunk_size);
        let mut packet = Vec::with_capacity(FOUNTAIN_DATA_FIXED_BYTES + payload_len);
        packet.extend_from_slice(&FOUNTAIN_MAGIC);
        packet.push(FOUNTAIN_DATA_TYPE);
        packet.extend_from_slice(&checksum.to_be_bytes());
        push_u32(&mut packet, index);
        packet.extend_from_slice(&chunk[..payload_len]);
        packets.push(packet);
    }

    let mut repair_number = 0;
    for window in 0..window_count {
        let start = window * FOUNTAIN_WINDOW_SIZE;
        let width = (data_packets - start).min(FOUNTAIN_WINDOW_SIZE);
        let repair_count = if overhead_percent == 0 {
            0
        } else {
            (width * overhead_percent).div_ceil(100).max(1)
        };
        for repair_index in 0..repair_count {
            let mask = fountain_mask(checksum as u32 ^ window as u32, width, repair_index);
            let mut payload = vec![0u8; chunk_size];
            for local in 0..width {
                if mask & (1u32 << local) == 0 {
                    continue;
                }
                for (left, right) in payload.iter_mut().zip(&padded_chunks[start + local]) {
                    *left ^= right;
                }
            }
            let mut packet = Vec::with_capacity(FOUNTAIN_REPAIR_FIXED_BYTES + chunk_size);
            packet.extend_from_slice(&FOUNTAIN_MAGIC);
            packet.push(FOUNTAIN_REPAIR_TYPE);
            packet.extend_from_slice(&checksum.to_be_bytes());
            push_u16(&mut packet, window);
            packet.extend_from_slice(&mask.to_be_bytes());
            packet.extend_from_slice(&payload);
            packets.push(packet);
            repair_number += 1;
        }
    }
    debug_assert_eq!(repair_number, repair_packets);
    Some(packets)
}

#[wasm_bindgen]
pub fn prepare_fountain_packets(
    buffer: Vec<u8>,
    filename: String,
    chunk_bytes: u32,
    overhead_percent: u32,
) -> JsValue {
    let Some(packets) = build_fountain_packets(
        &buffer,
        &filename,
        chunk_bytes as usize,
        overhead_percent as usize,
    ) else {
        return js_sys::Array::new().into();
    };
    let output = js_sys::Array::new();
    for packet in packets {
        output.push(&js_sys::Uint8Array::from(packet.as_slice()));
    }
    output.into()
}

fn parse_fountain_header(packet: &[u8]) -> Option<FountainHeader> {
    if packet.len() < FOUNTAIN_HEADER_FIXED_BYTES
        || packet.get(..3)? != FOUNTAIN_MAGIC
        || packet[3] != FOUNTAIN_HEADER_TYPE
    {
        return None;
    }
    let codec = packet[4];
    let window_size = packet[5] as usize;
    let checksum = read_u64(packet, 7)?;
    let digest: [u8; 32] = packet.get(15..47)?.try_into().ok()?;
    let file_size = read_u32(packet, 47)?;
    let compressed_size = read_u32(packet, 51)?;
    let chunk_size = read_u16(packet, 55)?;
    let data_packets = read_u32(packet, 57)?;
    let overhead_percent = packet[61] as usize;
    let repair_packets = read_u32(packet, 62)?;
    let filename_len = read_u16(packet, 66)?;
    let filename_start = FOUNTAIN_HEADER_FIXED_BYTES;
    let filename_end = filename_start.checked_add(filename_len)?;
    let filename = String::from_utf8(packet.get(filename_start..filename_end)?.to_vec()).ok()?;
    if codec > 1
        || window_size != FOUNTAIN_WINDOW_SIZE
        || overhead_percent > FOUNTAIN_MAX_OVERHEAD_PERCENT
        || !(BINARY_MIN_CHUNK_BYTES..=BINARY_MAX_CHUNK_BYTES).contains(&chunk_size)
        || data_packets == 0
        || data_packets > BINARY_MAX_PENDING_PACKETS
        || file_size > MAX_FILE_BYTES
        || compressed_size > MAX_FILE_BYTES * 2
        || repair_packets > BINARY_MAX_PENDING_PACKETS
        || filename.is_empty()
        || filename.len() > MAX_FILENAME_BYTES
        || filename.chars().any(|character| character.is_control())
    {
        return None;
    }
    let expected_packets = compressed_size.max(1).div_ceil(chunk_size);
    if data_packets != expected_packets {
        return None;
    }
    Some(FountainHeader {
        codec,
        checksum,
        digest,
        file_size,
        compressed_size,
        chunk_size,
        data_packets,
        overhead_percent,
        repair_packets,
        filename,
    })
}

fn fountain_window_mut(
    state: &mut FountainReceiverState,
    window: usize,
) -> Option<&mut FountainWindow> {
    let header = state.header.as_ref()?;
    let start = window * FOUNTAIN_WINDOW_SIZE;
    if start >= header.data_packets {
        return None;
    }
    let width = (header.data_packets - start).min(FOUNTAIN_WINDOW_SIZE);
    Some(
        state
            .windows
            .entry(window)
            .or_insert_with(|| FountainWindow::new(width)),
    )
}

fn fountain_missing_ranges(state: &FountainReceiverState) -> String {
    let Some(header) = state.header.as_ref() else {
        return String::new();
    };
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < header.data_packets {
        let present = state
            .windows
            .get(&(index / FOUNTAIN_WINDOW_SIZE))
            .and_then(|window| window.chunks.get(index % FOUNTAIN_WINDOW_SIZE))
            .is_some_and(Option::is_some);
        if present {
            index += 1;
            continue;
        }
        let start = index;
        while index < header.data_packets {
            let present = state
                .windows
                .get(&(index / FOUNTAIN_WINDOW_SIZE))
                .and_then(|window| window.chunks.get(index % FOUNTAIN_WINDOW_SIZE))
                .is_some_and(Option::is_some);
            if present {
                break;
            }
            index += 1;
        }
        let end = index - 1;
        ranges.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
    }
    ranges.join(",")
}

fn fountain_try_assemble(state: &mut FountainReceiverState) -> Option<Vec<u8>> {
    let header = state.header.as_ref()?;
    if state.complete {
        return None;
    }
    let mut compressed = Vec::with_capacity(header.compressed_size);
    for index in 0..header.data_packets {
        let chunk = state
            .windows
            .get(&(index / FOUNTAIN_WINDOW_SIZE))?
            .chunks
            .get(index % FOUNTAIN_WINDOW_SIZE)?
            .as_ref()?;
        compressed.extend_from_slice(chunk);
    }
    compressed.truncate(header.compressed_size);
    let file = if header.codec == 1 {
        decompress_file(&compressed)?
    } else {
        compressed
    };
    if file.len() != header.file_size
        || checksum(&file, header.filename.as_bytes()) != header.checksum
        || file_digest(&file, header.filename.as_bytes()) != header.digest
    {
        return None;
    }
    state.complete = true;
    Some(file)
}

fn install_fountain_header(
    state: &mut FountainReceiverState,
    header: FountainHeader,
) -> Option<Vec<u8>> {
    let is_new = state
        .header
        .as_ref()
        .is_none_or(|current| current.checksum != header.checksum);
    let pending = if is_new {
        let pending = std::mem::take(&mut state.pending);
        *state = FountainReceiverState {
            header: Some(header),
            ..FountainReceiverState::default()
        };
        pending
    } else {
        Vec::new()
    };
    let mut assembled = None;
    for packet in pending {
        assembled = process_fountain_inner(state, &packet, false).or(assembled);
    }
    assembled.or_else(|| fountain_try_assemble(state))
}

fn process_fountain_inner(
    state: &mut FountainReceiverState,
    packet: &[u8],
    allow_pending: bool,
) -> Option<Vec<u8>> {
    if packet.len() < 4 || packet.get(..3) != Some(&FOUNTAIN_MAGIC) {
        if allow_pending
            && state.header.is_none()
            && state.pending.len() < BINARY_MAX_PENDING_PACKETS
        {
            state.pending.push(packet.to_vec());
        }
        return None;
    }
    match packet[3] {
        FOUNTAIN_HEADER_TYPE => install_fountain_header(state, parse_fountain_header(packet)?),
        FOUNTAIN_DATA_TYPE => {
            let checksum_value = read_u64(packet, 4)?;
            let index = read_u32(packet, 12)?;
            let Some(header) = state.header.as_ref() else {
                if allow_pending && state.pending.len() < BINARY_MAX_PENDING_PACKETS {
                    state.pending.push(packet.to_vec());
                }
                return None;
            };
            if checksum_value != header.checksum
                || index >= header.data_packets
                || packet.len()
                    != FOUNTAIN_DATA_FIXED_BYTES
                        + header
                            .compressed_size
                            .saturating_sub(index * header.chunk_size)
                            .min(header.chunk_size)
            {
                return None;
            }
            let window = index / FOUNTAIN_WINDOW_SIZE;
            let local = index % FOUNTAIN_WINDOW_SIZE;
            let key = (FOUNTAIN_DATA_TYPE, index, 0);
            if state.seen.insert(key, true).is_none() {
                state.received_symbols += 1;
                state.received_data.insert(index, true);
            }
            let mut payload = vec![0u8; header.chunk_size];
            payload[..packet.len() - FOUNTAIN_DATA_FIXED_BYTES]
                .copy_from_slice(&packet[FOUNTAIN_DATA_FIXED_BYTES..]);
            let window_state = fountain_window_mut(state, window)?;
            window_state.add_equation(FountainEquation {
                mask: 1u32 << local,
                payload,
            });
            let _ = window_state.solve();
            fountain_try_assemble(state)
        }
        FOUNTAIN_REPAIR_TYPE => {
            let checksum_value = read_u64(packet, 4)?;
            let window = read_u16(packet, 12)?;
            let mask = u32::from_be_bytes(packet.get(14..18)?.try_into().ok()?);
            let Some(header) = state.header.as_ref() else {
                if allow_pending && state.pending.len() < BINARY_MAX_PENDING_PACKETS {
                    state.pending.push(packet.to_vec());
                }
                return None;
            };
            let width =
                (header.data_packets - window * FOUNTAIN_WINDOW_SIZE).min(FOUNTAIN_WINDOW_SIZE);
            let key = (FOUNTAIN_REPAIR_TYPE, window, mask);
            if checksum_value != header.checksum
                || window * FOUNTAIN_WINDOW_SIZE >= header.data_packets
                || mask == 0
                || (width < 32 && mask >> width != 0)
                || packet.len() != FOUNTAIN_REPAIR_FIXED_BYTES + header.chunk_size
            {
                return None;
            }
            if state.seen.insert(key, true).is_none() {
                state.received_symbols += 1;
            }
            let recovered_locals = {
                let window_state = fountain_window_mut(state, window)?;
                window_state.add_equation(FountainEquation {
                    mask,
                    payload: packet[FOUNTAIN_REPAIR_FIXED_BYTES..].to_vec(),
                });
                let _ = window_state.solve();
                (0..width)
                    .filter(|local| window_state.chunks[*local].is_some())
                    .collect::<Vec<_>>()
            };
            for local in recovered_locals {
                let index = window * FOUNTAIN_WINDOW_SIZE + local;
                if state.received_data.insert(index, true).is_none() {
                    state.recovered_packets += 1;
                }
            }
            fountain_try_assemble(state)
        }
        _ => None,
    }
}

#[wasm_bindgen]
pub fn reset_fountain_receiver() {
    FOUNTAIN_RECEIVER.with(|receiver| *receiver.borrow_mut() = FountainReceiverState::default());
}

#[wasm_bindgen]
pub fn process_fountain_packet(packet: Vec<u8>) -> Option<Vec<u8>> {
    FOUNTAIN_RECEIVER
        .with(|receiver| process_fountain_inner(&mut receiver.borrow_mut(), &packet, true))
}

#[wasm_bindgen]
pub fn fountain_receiver_progress() -> Vec<u32> {
    FOUNTAIN_RECEIVER.with(|receiver| {
        let state = receiver.borrow();
        let total = state
            .header
            .as_ref()
            .map_or(0, |header| header.data_packets);
        let received = state
            .windows
            .values()
            .flat_map(|window| window.chunks.iter())
            .filter(|chunk| chunk.is_some())
            .count();
        vec![received as u32, total as u32]
    })
}

#[wasm_bindgen]
pub fn fountain_receiver_packet_map() -> Vec<u8> {
    FOUNTAIN_RECEIVER.with(|receiver| {
        let state = receiver.borrow();
        let total = state
            .header
            .as_ref()
            .map_or(0, |header| header.data_packets);
        (0..total)
            .map(|index| {
                u8::from(
                    state
                        .windows
                        .get(&(index / FOUNTAIN_WINDOW_SIZE))
                        .and_then(|window| window.chunks.get(index % FOUNTAIN_WINDOW_SIZE))
                        .is_some_and(Option::is_some),
                )
            })
            .collect()
    })
}

#[wasm_bindgen]
pub fn fountain_receiver_missing_ranges() -> String {
    FOUNTAIN_RECEIVER.with(|receiver| fountain_missing_ranges(&receiver.borrow()))
}

#[wasm_bindgen]
pub fn fountain_receiver_info() -> String {
    FOUNTAIN_RECEIVER.with(|receiver| {
        let state = receiver.borrow();
        let Some(header) = state.header.as_ref() else {
            return "{}".to_string();
        };
        let received_packets = fountain_receiver_progress_internal(&state);
        let info = FountainReceiverInfo {
            strategy: "fountain-lt".to_string(),
            filename: header.filename.clone(),
            total_packets: header.data_packets,
            received_packets,
            missing_packets: header.data_packets.saturating_sub(received_packets),
            file_size: header.file_size,
            compressed_size: header.compressed_size,
            checksum: Some(format!("{:016x}", header.checksum)),
            sha256: Some(bytes_hex(&header.digest)),
            codec: if header.codec == 1 { "deflate" } else { "raw" }.to_string(),
            overhead_percent: header.overhead_percent,
            repair_packets: header.repair_packets,
            received_symbols: state.received_symbols,
            recovered_packets: state.recovered_packets,
            complete: state.complete,
        };
        serde_json::to_string(&info).unwrap_or_else(|_| "{}".to_string())
    })
}

fn fountain_receiver_progress_internal(state: &FountainReceiverState) -> usize {
    state
        .windows
        .values()
        .flat_map(|window| window.chunks.iter())
        .filter(|chunk| chunk.is_some())
        .count()
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

    #[test]
    fn binary_protocol_uses_fec_to_recover_one_missing_packet() {
        let mut seed = 0x1234_5678u32;
        let input: Vec<u8> = (0..24_000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            })
            .collect();
        let packets = build_binary_packets(&input, "video.bin", 800, 8).expect("packets");
        reset_binary_receiver();

        process_binary_packet(packets[0].clone());
        // Skip data packet 3. All remaining data and parity packets are sent.
        let mut output = None;
        for packet in packets.iter().skip(1) {
            let is_missing_data =
                packet.get(3) == Some(&BINARY_DATA_TYPE) && read_u32(packet, 12) == Some(3);
            if !is_missing_data {
                output = process_binary_packet(packet.clone()).or(output);
            }
        }

        let progress = binary_receiver_progress();
        assert_eq!(progress[0], progress[1]);
        assert_eq!(output.expect("assembled file"), input);
        assert!(binary_receiver_info().contains("recovered_packets"));
    }

    #[test]
    fn binary_protocol_accepts_data_before_header() {
        let input = vec![42u8; 5_000];
        let packets = build_binary_packets(&input, "antes.dat", 800, 0).expect("packets");
        reset_binary_receiver();
        process_binary_packet(packets[1].clone());
        let output = process_binary_packet(packets[0].clone()).expect("pending data");
        assert_eq!(output, input);
        assert!(binary_receiver_info().contains("antes.dat"));
    }

    #[test]
    fn fountain_protocol_recovers_multiple_missing_source_packets() {
        let mut seed = 0x3141_5926u32;
        let input: Vec<u8> = (0..20_000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            })
            .collect();
        let packets = build_fountain_packets(&input, "fountain.bin", 600, 30).expect("packets");
        reset_fountain_receiver();
        let mut output = None;
        for packet in &packets {
            let skip = packet.get(3) == Some(&FOUNTAIN_DATA_TYPE)
                && matches!(read_u32(packet, 12), Some(3 | 7 | 11));
            if !skip {
                output = process_fountain_packet(packet.clone()).or(output);
            }
        }
        let output = match output {
            Some(output) => output,
            None => {
                let info = fountain_receiver_info();
                let missing =
                    FOUNTAIN_RECEIVER.with(|receiver| fountain_missing_ranges(&receiver.borrow()));
                panic!("fountain should assemble file: info={info} missing={missing}");
            }
        };
        assert_eq!(output, input);
        assert!(fountain_receiver_info().contains("fountain-lt"));
        assert!(fountain_receiver_info().contains("\"recovered_packets\":3"));
    }

    #[test]
    fn binary_protocol_rejects_tampered_payload() {
        let input: Vec<u8> = (0..8_000).map(|value| (value % 251) as u8).collect();
        let mut packets = build_binary_packets(&input, "integro.bin", 600, 0).expect("packets");
        packets[1][BINARY_DATA_FIXED_BYTES] ^= 0x80;
        reset_binary_receiver();
        process_binary_packet(packets[0].clone());
        let mut output = None;
        for packet in packets.iter().skip(1) {
            output = process_binary_packet(packet.clone()).or(output);
        }
        assert!(output.is_none());
        assert!(binary_receiver_info().contains("\"complete\":false"));
    }
}
