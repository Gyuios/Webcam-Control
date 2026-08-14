//! Crash-tolerant, versioned frame exchange backed by a memory-mapped file.
//!
//! CTFRAME2 uses three independently versioned slots. The producer never
//! writes into the currently published slot and atomically publishes a token
//! only after the next payload is complete. Readers prefer the newest frame,
//! never wait for the producer and reject a slot if it changes while copied.

use memmap2::{Mmap, MmapMut};
use std::{
    fs::{File, OpenOptions},
    io,
    path::Path,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

pub const MAGIC: &[u8; 8] = b"CTFRAME2";
pub const HEADER_SIZE: usize = 64;
pub const SLOT_HEADER_SIZE: usize = 64;
pub const SLOT_COUNT: usize = 3;
pub const PIXEL_FORMAT_BGRA: u32 = 1;
pub const PIXEL_FORMAT_NV12: u32 = 2;
pub const MAX_FRAME_BYTES: usize = 256 * 1024 * 1024;
pub const ACTIVE_FRAME_MAX_AGE_MICROS: u64 = 2_000_000;

const SLOT_ALIGNMENT: usize = 64;
const SLOT_STATE_WRITING: u32 = 1;
const SLOT_STATE_PUBLISHED: u32 = 2;

const GLOBAL_ACTIVE_OFFSET: usize = 28;
const GLOBAL_PUBLICATION_OFFSET: usize = 32;
const GLOBAL_HEARTBEAT_OFFSET: usize = 48;

const SLOT_SEQUENCE_OFFSET: usize = 0;
const SLOT_TIMESTAMP_OFFSET: usize = 8;
const SLOT_WIDTH_OFFSET: usize = 16;
const SLOT_HEIGHT_OFFSET: usize = 20;
const SLOT_STRIDE_OFFSET: usize = 24;
const SLOT_PIXEL_FORMAT_OFFSET: usize = 28;
const SLOT_FRAME_SIZE_OFFSET: usize = 32;
const SLOT_STATE_OFFSET: usize = 36;
const SLOT_GENERATION_OFFSET: usize = 40;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameMetadata {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixel_format: u32,
    pub frame_size: u32,
    pub sequence: u64,
    pub timestamp_micros: u64,
    pub active: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameSnapshot {
    pub metadata: FrameMetadata,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
struct Layout {
    slot_span: usize,
    frame_capacity: usize,
    generation: u64,
}

pub struct FrameWriter {
    mapping: MmapMut,
    width: u32,
    height: u32,
    stride: u32,
    pixel_format: u32,
    frame_size: usize,
    slot_span: usize,
    sequence: u64,
    next_slot: usize,
    generation: u64,
}

pub struct FrameReader {
    mapping: Mmap,
    layout: Layout,
}

impl FrameWriter {
    pub fn create(path: &Path, width: u32, height: u32) -> io::Result<Self> {
        Self::create_with_format(path, width, height, PIXEL_FORMAT_BGRA)
    }

    pub fn create_with_format(
        path: &Path,
        width: u32,
        height: u32,
        pixel_format: u32,
    ) -> io::Result<Self> {
        let (stride, frame_size) = checked_frame_layout(width, height, pixel_format)?;
        Self::create_with_capacity_inner(
            path,
            width,
            height,
            pixel_format,
            stride,
            frame_size,
            frame_size,
        )
    }

    pub fn create_with_capacity(
        path: &Path,
        width: u32,
        height: u32,
        pixel_format: u32,
        frame_capacity: usize,
    ) -> io::Result<Self> {
        let (stride, frame_size) = checked_frame_layout(width, height, pixel_format)?;
        if frame_capacity < frame_size || frame_capacity > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "frame capacity must contain the frame and stay within the transport limit",
            ));
        }
        Self::create_with_capacity_inner(
            path,
            width,
            height,
            pixel_format,
            stride,
            frame_size,
            frame_capacity,
        )
    }

    fn create_with_capacity_inner(
        path: &Path,
        width: u32,
        height: u32,
        pixel_format: u32,
        stride: u32,
        frame_size: usize,
        frame_capacity: usize,
    ) -> io::Result<Self> {
        let slot_span = align_up(SLOT_HEADER_SIZE + frame_capacity, SLOT_ALIGNMENT)?;
        let mapping_size = HEADER_SIZE
            .checked_add(slot_span.checked_mul(SLOT_COUNT).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "frame exchange is too large")
            })?)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "frame exchange is too large")
            })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(windows)]
        options.custom_flags(0x0000_0100 | 0x1000_0000); // TEMPORARY | RANDOM_ACCESS
        let file = options.open(path)?;
        if file.metadata()?.len() != mapping_size as u64 {
            file.set_len(mapping_size as u64)?;
        }
        // SAFETY: the file is owned for initialization, has its final size and
        // all accesses below are bounds-checked against the mapping.
        let mut mapping = unsafe { MmapMut::map_mut(&file)? };
        mapping.fill(0);
        let generation = new_generation();
        encode_global_header(
            &mut mapping[..HEADER_SIZE],
            slot_span,
            frame_capacity,
            pixel_format,
            generation,
        );
        atomic_u32_mut(&mut mapping, GLOBAL_ACTIVE_OFFSET).store(1, Ordering::Release);
        atomic_u64_mut(&mut mapping, GLOBAL_HEARTBEAT_OFFSET)
            .store(timestamp_micros(), Ordering::Release);
        mapping.flush_range(0, HEADER_SIZE)?;
        Ok(Self {
            mapping,
            width,
            height,
            stride,
            pixel_format,
            frame_size,
            slot_span,
            sequence: 0,
            next_slot: 0,
            generation,
        })
    }

    pub fn write(&mut self, pixels: &[u8], timestamp_micros: u64) -> io::Result<u64> {
        if pixels.len() != self.frame_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "frame has {} bytes; expected {} for format {}",
                    pixels.len(),
                    self.frame_size,
                    self.pixel_format
                ),
            ));
        }

        let slot_index = self.next_slot;
        self.next_slot = (self.next_slot + 1) % SLOT_COUNT;
        self.sequence = self.sequence.wrapping_add(2).max(2);
        let slot_offset = HEADER_SIZE + slot_index * self.slot_span;
        let payload_offset = slot_offset + SLOT_HEADER_SIZE;

        atomic_u32_mut(&mut self.mapping, slot_offset + SLOT_STATE_OFFSET)
            .store(SLOT_STATE_WRITING, Ordering::Release);
        write_u64(
            &mut self.mapping,
            slot_offset + SLOT_TIMESTAMP_OFFSET,
            timestamp_micros,
        );
        write_u32(
            &mut self.mapping,
            slot_offset + SLOT_WIDTH_OFFSET,
            self.width,
        );
        write_u32(
            &mut self.mapping,
            slot_offset + SLOT_HEIGHT_OFFSET,
            self.height,
        );
        write_u32(
            &mut self.mapping,
            slot_offset + SLOT_STRIDE_OFFSET,
            self.stride,
        );
        write_u32(
            &mut self.mapping,
            slot_offset + SLOT_PIXEL_FORMAT_OFFSET,
            self.pixel_format,
        );
        write_u32(
            &mut self.mapping,
            slot_offset + SLOT_FRAME_SIZE_OFFSET,
            self.frame_size as u32,
        );
        write_u64(
            &mut self.mapping,
            slot_offset + SLOT_GENERATION_OFFSET,
            self.generation,
        );
        self.mapping[payload_offset..payload_offset + self.frame_size].copy_from_slice(pixels);
        atomic_u64_mut(&mut self.mapping, slot_offset + SLOT_SEQUENCE_OFFSET)
            .store(self.sequence, Ordering::Relaxed);
        atomic_u32_mut(&mut self.mapping, slot_offset + SLOT_STATE_OFFSET)
            .store(SLOT_STATE_PUBLISHED, Ordering::Release);

        atomic_u64_mut(&mut self.mapping, GLOBAL_HEARTBEAT_OFFSET)
            .store(timestamp_micros, Ordering::Relaxed);
        atomic_u64_mut(&mut self.mapping, GLOBAL_PUBLICATION_OFFSET).store(
            publication_token(self.sequence, slot_index),
            Ordering::Release,
        );
        Ok(self.sequence)
    }

    pub fn write_now(&mut self, pixels: &[u8]) -> io::Result<u64> {
        self.write(pixels, timestamp_micros())
    }

    pub fn mark_inactive(&mut self) {
        atomic_u64_mut(&mut self.mapping, GLOBAL_HEARTBEAT_OFFSET)
            .store(timestamp_micros(), Ordering::Relaxed);
        atomic_u32_mut(&mut self.mapping, GLOBAL_ACTIVE_OFFSET).store(0, Ordering::Release);
        let _ = self.mapping.flush_range(0, HEADER_SIZE);
    }
}

impl Drop for FrameWriter {
    fn drop(&mut self) {
        self.mark_inactive();
    }
}

impl FrameReader {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        // SAFETY: the mapping is read-only and remains alive for the reader's
        // lifetime. The protocol layout is validated before payload access.
        let mapping = unsafe { Mmap::map(&file)? };
        let layout = decode_global_header(&mapping)?;
        validate_mapping_size(mapping.len(), layout)?;
        Ok(Self { mapping, layout })
    }

    pub fn read_latest(&self) -> io::Result<Option<FrameSnapshot>> {
        self.read_latest_after(None)
    }

    /// Returns the newest committed frame without copying when that sequence
    /// was already consumed. A collision is retried against the latest slot.
    pub fn read_latest_after(
        &self,
        last_sequence: impl Into<Option<u64>>,
    ) -> io::Result<Option<FrameSnapshot>> {
        let last_sequence = last_sequence.into();
        for _ in 0..SLOT_COUNT + 1 {
            if atomic_u32(&self.mapping, GLOBAL_ACTIVE_OFFSET).load(Ordering::Acquire) == 0 {
                return Ok(None);
            }
            let publication_before =
                atomic_u64(&self.mapping, GLOBAL_PUBLICATION_OFFSET).load(Ordering::Acquire);
            let Some((sequence, slot_index)) = decode_publication(publication_before) else {
                return Ok(None);
            };
            if last_sequence == Some(sequence) {
                return Ok(None);
            }
            let slot_offset = HEADER_SIZE + slot_index * self.layout.slot_span;
            if atomic_u32(&self.mapping, slot_offset + SLOT_STATE_OFFSET).load(Ordering::Acquire)
                != SLOT_STATE_PUBLISHED
            {
                std::thread::yield_now();
                continue;
            }
            let slot_sequence = atomic_u64(&self.mapping, slot_offset + SLOT_SEQUENCE_OFFSET)
                .load(Ordering::Acquire);
            if slot_sequence != sequence {
                std::thread::yield_now();
                continue;
            }
            let metadata = decode_slot_header(&self.mapping, slot_offset, self.layout, sequence)?;
            let frame_size = metadata.frame_size as usize;
            let payload_offset = slot_offset + SLOT_HEADER_SIZE;
            let bytes = self.mapping[payload_offset..payload_offset + frame_size].to_vec();

            let state_after =
                atomic_u32(&self.mapping, slot_offset + SLOT_STATE_OFFSET).load(Ordering::Acquire);
            let sequence_after = atomic_u64(&self.mapping, slot_offset + SLOT_SEQUENCE_OFFSET)
                .load(Ordering::Acquire);
            let publication_after =
                atomic_u64(&self.mapping, GLOBAL_PUBLICATION_OFFSET).load(Ordering::Acquire);
            if state_after == SLOT_STATE_PUBLISHED
                && sequence_after == sequence
                && publication_after == publication_before
            {
                return Ok(Some(FrameSnapshot { metadata, bytes }));
            }
            std::thread::yield_now();
        }
        Ok(None)
    }
}

pub fn has_active_frame(path: &Path) -> io::Result<bool> {
    let reader = FrameReader::open(path)?;
    if atomic_u32(&reader.mapping, GLOBAL_ACTIVE_OFFSET).load(Ordering::Acquire) == 0 {
        return Ok(false);
    }
    if atomic_u64(&reader.mapping, GLOBAL_PUBLICATION_OFFSET).load(Ordering::Acquire) == 0 {
        return Ok(false);
    }
    let heartbeat = atomic_u64(&reader.mapping, GLOBAL_HEARTBEAT_OFFSET).load(Ordering::Acquire);
    let age_micros = timestamp_micros().saturating_sub(heartbeat);
    Ok(heartbeat > 0 && age_micros <= ACTIVE_FRAME_MAX_AGE_MICROS)
}

fn encode_global_header(
    bytes: &mut [u8],
    slot_span: usize,
    frame_capacity: usize,
    pixel_format: u32,
    generation: u64,
) {
    bytes[..HEADER_SIZE].fill(0);
    bytes[0..8].copy_from_slice(MAGIC);
    write_u32(bytes, 8, HEADER_SIZE as u32);
    write_u32(bytes, 12, SLOT_COUNT as u32);
    write_u32(bytes, 16, slot_span as u32);
    write_u32(bytes, 20, frame_capacity as u32);
    write_u32(bytes, 24, pixel_format);
    write_u64(bytes, 40, generation);
}

fn decode_global_header(bytes: &[u8]) -> io::Result<Layout> {
    if bytes.len() < HEADER_SIZE || &bytes[..8] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid CTFRAME2 magic",
        ));
    }
    if read_u32(bytes, 8)? as usize != HEADER_SIZE || read_u32(bytes, 12)? as usize != SLOT_COUNT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported CTFRAME2 layout",
        ));
    }
    let slot_span = read_u32(bytes, 16)? as usize;
    let frame_capacity = read_u32(bytes, 20)? as usize;
    let pixel_format = read_u32(bytes, 24)?;
    if slot_span < SLOT_HEADER_SIZE + frame_capacity
        || !slot_span.is_multiple_of(SLOT_ALIGNMENT)
        || frame_capacity == 0
        || frame_capacity > MAX_FRAME_BYTES
        || !matches!(pixel_format, PIXEL_FORMAT_BGRA | PIXEL_FORMAT_NV12)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid CTFRAME2 slot layout",
        ));
    }
    Ok(Layout {
        slot_span,
        frame_capacity,
        generation: read_u64(bytes, 40)?,
    })
}

fn decode_slot_header(
    bytes: &[u8],
    slot_offset: usize,
    layout: Layout,
    sequence: u64,
) -> io::Result<FrameMetadata> {
    let metadata = FrameMetadata {
        width: read_u32(bytes, slot_offset + SLOT_WIDTH_OFFSET)?,
        height: read_u32(bytes, slot_offset + SLOT_HEIGHT_OFFSET)?,
        stride: read_u32(bytes, slot_offset + SLOT_STRIDE_OFFSET)?,
        pixel_format: read_u32(bytes, slot_offset + SLOT_PIXEL_FORMAT_OFFSET)?,
        frame_size: read_u32(bytes, slot_offset + SLOT_FRAME_SIZE_OFFSET)?,
        sequence,
        timestamp_micros: read_u64(bytes, slot_offset + SLOT_TIMESTAMP_OFFSET)?,
        active: true,
    };
    let generation = read_u64(bytes, slot_offset + SLOT_GENERATION_OFFSET)?;
    let (expected_stride, expected_size) =
        checked_frame_layout(metadata.width, metadata.height, metadata.pixel_format)?;
    if generation != layout.generation
        || metadata.stride != expected_stride
        || metadata.frame_size as usize != expected_size
        || expected_size > layout.frame_capacity
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid CTFRAME2 frame metadata",
        ));
    }
    Ok(metadata)
}

fn validate_mapping_size(mapping_len: usize, layout: Layout) -> io::Result<()> {
    let required = HEADER_SIZE
        .checked_add(layout.slot_span.checked_mul(SLOT_COUNT).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "CTFRAME2 mapping is too large")
        })?)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "CTFRAME2 mapping is too large")
        })?;
    if mapping_len < required {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "CTFRAME2 mapping is truncated",
        ));
    }
    Ok(())
}

fn checked_frame_layout(width: u32, height: u32, pixel_format: u32) -> io::Result<(u32, usize)> {
    let (stride, size) = match pixel_format {
        PIXEL_FORMAT_BGRA => {
            let stride = width.checked_mul(4);
            let size = stride.and_then(|value| value.checked_mul(height));
            (stride, size)
        }
        PIXEL_FORMAT_NV12 if width.is_multiple_of(2) && height.is_multiple_of(2) => {
            let size = width
                .checked_mul(height)
                .and_then(|luma| luma.checked_add(luma / 2));
            (Some(width), size)
        }
        _ => (None, None),
    };
    let stride = stride.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid frame dimensions or format",
        )
    })?;
    let size = size
        .map(|bytes| bytes as usize)
        .filter(|bytes| *bytes > 0 && *bytes <= MAX_FRAME_BYTES)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid frame dimensions or format",
            )
        })?;
    Ok((stride, size))
}

fn align_up(value: usize, alignment: usize) -> io::Result<usize> {
    value
        .checked_add(alignment - 1)
        .map(|next| next & !(alignment - 1))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "frame slot is too large"))
}

fn publication_token(sequence: u64, slot_index: usize) -> u64 {
    (sequence << 2) | (slot_index as u64 + 1)
}

fn decode_publication(token: u64) -> Option<(u64, usize)> {
    if token == 0 {
        return None;
    }
    let slot_value = (token & 0b11) as usize;
    let sequence = token >> 2;
    if !(1..=SLOT_COUNT).contains(&slot_value) || sequence == 0 {
        return None;
    }
    Some((sequence, slot_value - 1))
}

fn write_u32(buffer: &mut [u8], offset: usize, value: u32) {
    buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(buffer: &mut [u8], offset: usize, value: u64) {
    buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn read_u32(buffer: &[u8], offset: usize) -> io::Result<u32> {
    buffer
        .get(offset..offset + 4)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "truncated CTFRAME2 data"))
}

fn read_u64(buffer: &[u8], offset: usize) -> io::Result<u64> {
    buffer
        .get(offset..offset + 8)
        .and_then(|value| value.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "truncated CTFRAME2 data"))
}

fn atomic_u32(bytes: &[u8], offset: usize) -> &AtomicU32 {
    debug_assert_eq!(
        (bytes.as_ptr() as usize + offset) % align_of::<AtomicU32>(),
        0
    );
    // SAFETY: every atomic field has explicit natural alignment in CTFRAME2,
    // remains inside a live mapping and is accessed atomically by all peers.
    unsafe { &*(bytes.as_ptr().add(offset).cast::<AtomicU32>()) }
}

fn atomic_u64(bytes: &[u8], offset: usize) -> &AtomicU64 {
    debug_assert_eq!(
        (bytes.as_ptr() as usize + offset) % align_of::<AtomicU64>(),
        0
    );
    // SAFETY: see `atomic_u32`; all u64 fields used atomically are 8-byte aligned.
    unsafe { &*(bytes.as_ptr().add(offset).cast::<AtomicU64>()) }
}

fn atomic_u32_mut(bytes: &mut [u8], offset: usize) -> &AtomicU32 {
    atomic_u32(bytes, offset)
}

fn atomic_u64_mut(bytes: &mut [u8], offset: usize) -> &AtomicU64 {
    atomic_u64(bytes, offset)
}

fn timestamp_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn new_generation() -> u64 {
    timestamp_micros()
        .rotate_left(13)
        .wrapping_add(u64::from(std::process::id()))
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc,
        },
        thread,
    };

    #[test]
    fn writer_and_reader_exchange_committed_frames() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("frame.bin");
        let mut writer = FrameWriter::create(&path, 2, 2).unwrap();
        let reader = FrameReader::open(&path).unwrap();
        assert_eq!(reader.read_latest().unwrap(), None);
        writer.write(&[7; 16], 1234).unwrap();
        let snapshot = reader.read_latest().unwrap().unwrap();
        assert_eq!(snapshot.bytes, vec![7; 16]);
        assert_eq!(snapshot.metadata.timestamp_micros, 1234);
        assert_eq!(snapshot.metadata.sequence, 2);
        assert_eq!(reader.read_latest_after(2).unwrap(), None);
        assert!(reader.read_latest_after(0).unwrap().is_some());
        writer.mark_inactive();
        assert_eq!(reader.read_latest().unwrap(), None);
    }

    #[test]
    fn writer_rotates_across_three_slots() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("frame.bin");
        let mut writer = FrameWriter::create(&path, 2, 2).unwrap();
        let reader = FrameReader::open(&path).unwrap();
        for value in 1..=9_u8 {
            writer.write(&[value; 16], u64::from(value)).unwrap();
            let snapshot = reader.read_latest().unwrap().unwrap();
            assert_eq!(snapshot.bytes, vec![value; 16]);
            assert_eq!(snapshot.metadata.timestamp_micros, u64::from(value));
        }
    }

    #[test]
    fn concurrent_reader_never_accepts_a_torn_frame() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("frame.bin");
        let mut writer = FrameWriter::create(&path, 64, 64).unwrap();
        writer.write(&vec![0; 64 * 64 * 4], 1).unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let reader_running = Arc::clone(&running);
        let reader_path = path.clone();
        let (reader_ready_tx, reader_ready_rx) = mpsc::sync_channel(0);
        let reader = thread::spawn(move || {
            let reader = FrameReader::open(&reader_path).unwrap();
            let initial = reader.read_latest().unwrap().unwrap();
            assert!(initial.bytes.iter().all(|byte| *byte == initial.bytes[0]));
            let mut accepted = 1;
            reader_ready_tx.send(()).unwrap();
            while reader_running.load(Ordering::Acquire) {
                if let Some(snapshot) = reader.read_latest().unwrap() {
                    let expected = snapshot.bytes[0];
                    assert!(snapshot.bytes.iter().all(|byte| *byte == expected));
                    accepted += 1;
                }
            }
            accepted
        });
        reader_ready_rx.recv().unwrap();
        for value in 1..=200_u8 {
            writer
                .write(&vec![value; 64 * 64 * 4], u64::from(value))
                .unwrap();
            thread::yield_now();
        }
        running.store(false, Ordering::Release);
        assert!(reader.join().unwrap() > 0);
    }

    #[test]
    fn writer_rejects_wrong_frame_size() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("frame.bin");
        let mut writer = FrameWriter::create(&path, 2, 2).unwrap();
        assert_eq!(
            writer.write(&[0; 15], 0).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn nv12_requires_even_dimensions_and_uses_compact_layout() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("frame.bin");
        let writer = FrameWriter::create_with_format(&path, 4, 2, PIXEL_FORMAT_NV12).unwrap();
        assert_eq!(writer.frame_size, 12);
        assert!(FrameWriter::create_with_format(&path, 3, 2, PIXEL_FORMAT_NV12).is_err());
    }

    #[test]
    fn fixed_capacity_keeps_mapping_size_stable_across_resolutions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("frame.bin");
        let capacity = 1920 * 1080 * 3 / 2;
        let first = FrameWriter::create_with_capacity(&path, 640, 360, PIXEL_FORMAT_NV12, capacity)
            .unwrap();
        let first_len = std::fs::metadata(&path).unwrap().len();
        drop(first);
        let second =
            FrameWriter::create_with_capacity(&path, 1920, 1080, PIXEL_FORMAT_NV12, capacity)
                .unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), first_len);
        drop(second);
    }

    #[test]
    fn rejects_oversized_dimensions() {
        assert_eq!(
            checked_frame_layout(u32::MAX, u32::MAX, PIXEL_FORMAT_BGRA)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn active_frame_status_rejects_a_stale_crashed_producer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("frame.bin");
        let mut writer = FrameWriter::create(&path, 2, 2).unwrap();
        writer
            .write(
                &[7; 16],
                timestamp_micros().saturating_sub(ACTIVE_FRAME_MAX_AGE_MICROS + 1),
            )
            .unwrap();
        assert!(!has_active_frame(&path).unwrap());

        writer.write_now(&[7; 16]).unwrap();
        assert!(has_active_frame(&path).unwrap());
    }
}
