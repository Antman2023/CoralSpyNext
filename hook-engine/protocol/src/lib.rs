#![no_std]
#![forbid(unsafe_code)]

//! Pointer-free, little-endian IPC for a single bounded debug-hook request.
//!
//! The broker initializes the entire mapping before loading a nonce-named DLL.
//! The callback copies and validates `request` before using any request field,
//! then claims `state` with Pending -> Running compare-exchange (Acquire).
//! Only that callback writes response/payload; it publishes Complete (Release).
//! The broker reads response/payload only after observing Complete (Acquire).
//! A deadline never permits the broker to mutate/reuse an in-flight mapping.
//! Mapping ownership, ACLs, process/thread identity, control class and password
//! protection must also be checked by the Windows layer. A nonce is correlation,
//! not authentication. No structure contains a pointer or architecture-sized int.

use core::sync::atomic::AtomicU32;

pub const MAGIC: u32 = u32::from_le_bytes(*b"CSN1");
pub const VERSION: u32 = 1;
pub const STATE_PENDING: u32 = 0;
pub const STATE_RUNNING: u32 = 1;
pub const STATE_COMPLETE: u32 = 2;
pub const MAX_TIMEOUT_MS: u32 = 10_000;
pub const MAX_ROWS: u32 = 512;
pub const MAX_COLUMNS: u32 = 32;
pub const MAX_NODES: u32 = 1024;
/// Number of levels, including level zero: valid depth values are 0..32.
pub const MAX_DEPTH: u32 = 32;
pub const MAX_TEXT_UNITS: u32 = 2048;
pub const MAX_RESULT_BYTES: usize = 1024 * 1024;
pub const REQUEST_HEADER_BYTES: u32 = 112;
pub const RESPONSE_HEADER_BYTES: u32 = 72;
pub const PAYLOAD_OFFSET: usize = 192;
pub const SHARED_MEMORY_BYTES: u32 = (PAYLOAD_OFFSET + MAX_RESULT_BYTES) as u32;
pub const RESULT_FLAG_TRUNCATED: u32 = 1;
pub const UNKNOWN_COUNT: u32 = u32::MAX;
pub const CANCEL_NOT_REQUESTED: u32 = 0;
pub const CANCEL_REQUESTED: u32 = 1;
pub const RECORD_FLAG_TEXT_TRUNCATED: u32 = 1;
pub const RECORD_FLAG_HAS_CHILDREN: u32 = 2;
pub const RECORD_FLAG_DISABLED: u32 = 4;
pub const RECORD_FLAG_CHECKED: u32 = 8;
pub const RECORD_FLAG_SEPARATOR: u32 = 16;
pub const RECORD_FLAG_OWNER_DRAW: u32 = 32;
pub const RECORD_FLAG_DEFAULT: u32 = 64;
pub const RECORD_FLAG_DEPTH_LIMIT: u32 = 128;
pub const RECORD_FLAG_NODE_LIMIT: u32 = 256;
pub const RECORD_FLAG_EXPANDED: u32 = 512;
pub const RECORD_FLAG_SELECTED: u32 = 1024;
pub const RECORD_HEADER_BYTES: usize = 20;
pub const NONCE_HEX_LEN: usize = 32;
pub const HOOK_FILENAME_LEN: usize = 43;
pub const MAPPING_NAME_LEN: usize = 51;

macro_rules! wire_enum {
    ($(#[$meta:meta])* pub enum $name:ident { $($variant:ident = $value:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[repr(u32)]
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum $name { $($variant = $value),+ }
        impl TryFrom<u32> for $name {
            type Error = Status;
            fn try_from(value: u32) -> Result<Self, Self::Error> {
                match value { $($value => Ok(Self::$variant),)+ _ => Err(Status::InvalidRequest) }
            }
        }
    };
}

wire_enum! { pub enum Operation {
    RichEditRtf = 1, ListView = 2, TreeView = 3, MenuTarget = 4, MenuDesktopOnce = 5
} }
wire_enum! { pub enum Architecture { X86 = 32, X64 = 64 } }
wire_enum! { pub enum Status {
    Ok = 0, Truncated = 1, InvalidRequest = 2, ClassMismatch = 3,
    PasswordControl = 4, AccessDenied = 5, Expired = 6, Unsupported = 7,
    ControlError = 8, WrongArchitecture = 9, Cancelled = 10
} }
wire_enum! { pub enum ResultKind { Rtf = 1, ListView = 2, TreeView = 3, Menu = 4 } }
wire_enum! { pub enum RecordKind { Cell = 1, TreeNode = 2, MenuItem = 3, Column = 4 } }

impl Operation {
    pub const fn result_kind(self) -> ResultKind {
        match self {
            Self::RichEditRtf => ResultKind::Rtf,
            Self::ListView => ResultKind::ListView,
            Self::TreeView => ResultKind::TreeView,
            Self::MenuTarget | Self::MenuDesktopOnce => ResultKind::Menu,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C, align(8))]
pub struct Target {
    pub hwnd: u64,
    pub pid: u32,
    pub tid: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct Limits {
    pub max_rows: u32,
    pub max_columns: u32,
    pub max_nodes: u32,
    pub max_depth: u32,
    pub max_text_units: u32,
    pub max_result_bytes: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_rows: MAX_ROWS,
            max_columns: MAX_COLUMNS,
            max_nodes: MAX_NODES,
            max_depth: MAX_DEPTH,
            max_text_units: MAX_TEXT_UNITS,
            max_result_bytes: MAX_RESULT_BYTES as u32,
        }
    }
}

impl Limits {
    pub fn validate(self) -> Result<(), Status> {
        let pairs = [
            (self.max_rows, MAX_ROWS),
            (self.max_columns, MAX_COLUMNS),
            (self.max_nodes, MAX_NODES),
            (self.max_depth, MAX_DEPTH),
            (self.max_text_units, MAX_TEXT_UNITS),
            (self.max_result_bytes, MAX_RESULT_BYTES as u32),
        ];
        if pairs
            .iter()
            .any(|&(actual, maximum)| actual == 0 || actual > maximum)
        {
            return Err(Status::InvalidRequest);
        }
        Ok(())
    }

    fn record_limit(self, kind: ResultKind) -> u32 {
        match kind {
            ResultKind::ListView => self.max_rows * self.max_columns + self.max_columns,
            ResultKind::TreeView | ResultKind::Menu => self.max_nodes,
            ResultKind::Rtf => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C, align(8))]
pub struct RequestHeader {
    pub magic: u32,
    pub version: u32,
    pub header_bytes: u32,
    pub mapping_bytes: u32,
    pub nonce: [u8; 16],
    pub operation: u32,
    pub architecture: u32,
    pub target_hwnd: u64,
    pub target_pid: u32,
    pub target_tid: u32,
    pub broker_pid: u32,
    pub timeout_ms: u32,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub max_rows: u32,
    pub max_columns: u32,
    pub max_nodes: u32,
    pub max_depth: u32,
    pub max_text_units: u32,
    pub max_result_bytes: u32,
    pub reserved: [u32; 2],
}

impl RequestHeader {
    pub fn new(
        operation: Operation,
        architecture: Architecture,
        nonce: [u8; 16],
        target: Target,
        broker_pid: u32,
        now_ms: u64,
        timeout_ms: u32,
    ) -> Result<Self, Status> {
        let expires_at_ms = now_ms
            .checked_add(u64::from(timeout_ms))
            .ok_or(Status::InvalidRequest)?;
        let value = Self {
            magic: MAGIC,
            version: VERSION,
            header_bytes: REQUEST_HEADER_BYTES,
            mapping_bytes: SHARED_MEMORY_BYTES,
            nonce,
            operation: operation as u32,
            architecture: architecture as u32,
            target_hwnd: target.hwnd,
            target_pid: target.pid,
            target_tid: target.tid,
            broker_pid,
            timeout_ms,
            created_at_ms: now_ms,
            expires_at_ms,
            max_rows: MAX_ROWS,
            max_columns: MAX_COLUMNS,
            max_nodes: MAX_NODES,
            max_depth: MAX_DEPTH,
            max_text_units: MAX_TEXT_UNITS,
            max_result_bytes: MAX_RESULT_BYTES as u32,
            reserved: [0; 2],
        };
        value.validate(&nonce, architecture, now_ms)?;
        Ok(value)
    }

    pub const fn target(&self) -> Target {
        Target {
            hwnd: self.target_hwnd,
            pid: self.target_pid,
            tid: self.target_tid,
        }
    }

    pub const fn limits(&self) -> Limits {
        Limits {
            max_rows: self.max_rows,
            max_columns: self.max_columns,
            max_nodes: self.max_nodes,
            max_depth: self.max_depth,
            max_text_units: self.max_text_units,
            max_result_bytes: self.max_result_bytes,
        }
    }

    /// Call only on a private copy of the header, never repeatedly on live IPC.
    pub fn validate(
        &self,
        expected_nonce: &[u8; 16],
        expected_arch: Architecture,
        now_ms: u64,
    ) -> Result<Operation, Status> {
        if self.magic != MAGIC
            || self.version != VERSION
            || self.header_bytes != REQUEST_HEADER_BYTES
            || self.mapping_bytes != SHARED_MEMORY_BYTES
            || self.reserved != [0; 2]
            || self.nonce == [0; 16]
            || self.nonce != *expected_nonce
            || self.broker_pid == 0
            || self.timeout_ms == 0
            || self.timeout_ms > MAX_TIMEOUT_MS
        {
            return Err(Status::InvalidRequest);
        }
        self.limits().validate()?;
        let operation = Operation::try_from(self.operation)?;
        let architecture = Architecture::try_from(self.architecture)?;
        if architecture != expected_arch {
            return Err(Status::WrongArchitecture);
        }
        match operation {
            Operation::MenuDesktopOnce => {
                if self.target() != Target::default() {
                    return Err(Status::InvalidRequest);
                }
            }
            _ => {
                if self.target_hwnd == 0
                    || self.target_pid == 0
                    || self.target_tid == 0
                    || (architecture == Architecture::X86 && self.target_hwnd > u64::from(u32::MAX))
                {
                    return Err(Status::InvalidRequest);
                }
            }
        }
        let lifetime = self
            .expires_at_ms
            .checked_sub(self.created_at_ms)
            .ok_or(Status::InvalidRequest)?;
        if lifetime == 0 || lifetime > u64::from(self.timeout_ms) || now_ms < self.created_at_ms {
            return Err(Status::InvalidRequest);
        }
        if now_ms >= self.expires_at_ms {
            return Err(Status::Expired);
        }
        Ok(operation)
    }

    pub fn validate_for_broker(
        &self,
        expected_nonce: &[u8; 16],
        expected_broker_pid: u32,
        expected_arch: Architecture,
        now_ms: u64,
    ) -> Result<Operation, Status> {
        if expected_broker_pid == 0 || self.broker_pid != expected_broker_pid {
            return Err(Status::InvalidRequest);
        }
        self.validate(expected_nonce, expected_arch, now_ms)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C, align(8))]
pub struct ResponseHeader {
    pub status: u32,
    pub kind: u32,
    pub bytes_written: u32,
    pub record_count: u32,
    pub flags: u32,
    pub win32_error: u32,
    pub reserved: [u32; 2],
    pub actual_hwnd: u64,
    pub actual_pid: u32,
    pub actual_tid: u32,
    pub root_menu: u64,
    pub reported_total_rows: u32,
    pub reported_total_columns: u32,
    pub reported_total_nodes: u32,
    pub reserved2: u32,
}

impl Default for ResponseHeader {
    fn default() -> Self {
        Self {
            status: 0,
            kind: 0,
            bytes_written: 0,
            record_count: 0,
            flags: 0,
            win32_error: 0,
            reserved: [0; 2],
            actual_hwnd: 0,
            actual_pid: 0,
            actual_tid: 0,
            root_menu: 0,
            reported_total_rows: UNKNOWN_COUNT,
            reported_total_columns: UNKNOWN_COUNT,
            reported_total_nodes: UNKNOWN_COUNT,
            reserved2: 0,
        }
    }
}

impl ResponseHeader {
    pub fn new(status: Status, kind: ResultKind, bytes_written: u32, record_count: u32) -> Self {
        Self {
            status: status as u32,
            kind: kind as u32,
            bytes_written,
            record_count,
            flags: if status == Status::Truncated {
                RESULT_FLAG_TRUNCATED
            } else {
                0
            },
            ..Self::default()
        }
    }

    /// Validate a copied header before slicing IPC. Payload content is validated
    /// separately with `RecordReader`; RTF is opaque bytes, never executable code.
    pub fn validate(&self, request: &RequestHeader) -> Result<Status, Status> {
        request.limits().validate()?;
        let operation = Operation::try_from(request.operation)?;
        let status = Status::try_from(self.status)?;
        let kind = ResultKind::try_from(self.kind)?;
        if kind != operation.result_kind()
            || self.reserved != [0; 2]
            || self.reserved2 != 0
            || self.flags & !RESULT_FLAG_TRUNCATED != 0
            || self.bytes_written > request.max_result_bytes
            || self.bytes_written as usize > MAX_RESULT_BYTES
            || (self.flags & RESULT_FLAG_TRUNCATED != 0) != (status == Status::Truncated)
        {
            return Err(Status::InvalidRequest);
        }
        if kind == ResultKind::Rtf {
            if self.record_count != 0 {
                return Err(Status::InvalidRequest);
            }
        } else if self.record_count > request.limits().record_limit(kind)
            || u64::from(self.record_count) * RECORD_HEADER_BYTES as u64
                > u64::from(self.bytes_written)
        {
            return Err(Status::InvalidRequest);
        }
        if status == Status::Ok || status == Status::Truncated {
            if self.actual_hwnd == 0
                || self.actual_pid == 0
                || self.actual_tid == 0
                || (request.architecture == Architecture::X86 as u32
                    && self.actual_hwnd > u64::from(u32::MAX))
                || (operation != Operation::MenuDesktopOnce
                    && (self.actual_hwnd != request.target_hwnd
                        || self.actual_pid != request.target_pid
                        || self.actual_tid != request.target_tid))
            {
                return Err(Status::InvalidRequest);
            }
        } else if self.bytes_written != 0 || self.record_count != 0 {
            return Err(Status::InvalidRequest);
        }
        Ok(status)
    }

    /// Validate an exact private payload copy, including the declared count and
    /// every record. Never pass bytes beyond `bytes_written` from the mapping.
    pub fn validate_payload(
        &self,
        request: &RequestHeader,
        payload: &[u8],
    ) -> Result<Status, Status> {
        let status = self.validate(request)?;
        if payload.len() != self.bytes_written as usize {
            return Err(Status::InvalidRequest);
        }
        let kind = ResultKind::try_from(self.kind)?;
        if kind != ResultKind::Rtf {
            RecordReader::new(payload, kind, request.limits())
                .and_then(|reader| reader.validate_count(self.record_count))
                .map_err(|_| Status::InvalidRequest)?;
        }
        Ok(status)
    }
}

/// Do not construct this 1 MiB value on a callback stack. Initialize a fresh OS
/// mapping in place. All-zero bytes contain a valid AtomicU32 in Pending state;
/// the broker must finish writing a valid request before installing the hook.
#[repr(C, align(8))]
pub struct SharedMemory {
    pub state: AtomicU32,
    /// The broker may set this to one (Release); callback reads it (Acquire).
    pub cancelled: AtomicU32,
    pub request: RequestHeader,
    pub response: ResponseHeader,
    pub payload: [u8; MAX_RESULT_BYTES],
}

// These assertions compile on every target, including both Windows architectures.
const _: () = {
    assert!(core::mem::size_of::<AtomicU32>() == 4);
    assert!(core::mem::size_of::<Target>() == 16);
    assert!(core::mem::size_of::<Limits>() == 24);
    assert!(core::mem::size_of::<RequestHeader>() == REQUEST_HEADER_BYTES as usize);
    assert!(core::mem::size_of::<ResponseHeader>() == RESPONSE_HEADER_BYTES as usize);
    assert!(core::mem::size_of::<SharedMemory>() == SHARED_MEMORY_BYTES as usize);
    assert!(core::mem::align_of::<SharedMemory>() == 8);
    assert!(core::mem::offset_of!(RequestHeader, target_hwnd) == 40);
    assert!(core::mem::offset_of!(RequestHeader, created_at_ms) == 64);
    assert!(core::mem::offset_of!(RequestHeader, expires_at_ms) == 72);
    assert!(core::mem::offset_of!(RequestHeader, reserved) == 104);
    assert!(core::mem::offset_of!(ResponseHeader, actual_hwnd) == 32);
    assert!(core::mem::offset_of!(ResponseHeader, root_menu) == 48);
    assert!(core::mem::offset_of!(ResponseHeader, reported_total_rows) == 56);
    assert!(core::mem::offset_of!(ResponseHeader, reserved2) == 68);
    assert!(core::mem::offset_of!(SharedMemory, cancelled) == 4);
    assert!(core::mem::offset_of!(SharedMemory, request) == 8);
    assert!(core::mem::offset_of!(SharedMemory, response) == 120);
    assert!(core::mem::offset_of!(SharedMemory, payload) == PAYLOAD_OFFSET);
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NameError {
    Length,
    Format,
    ZeroNonce,
}

pub fn nonce_hex(nonce: &[u8; 16]) -> [u8; NONCE_HEX_LEN] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0; NONCE_HEX_LEN];
    for (i, &byte) in nonce.iter().enumerate() {
        out[2 * i] = HEX[(byte >> 4) as usize];
        out[2 * i + 1] = HEX[(byte & 15) as usize];
    }
    out
}

pub fn parse_nonce_hex(bytes: &[u8]) -> Result<[u8; 16], NameError> {
    fn nibble(byte: u8) -> Result<u8, NameError> {
        match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            b'A'..=b'F' => Ok(byte - b'A' + 10),
            _ => Err(NameError::Format),
        }
    }
    if bytes.len() != NONCE_HEX_LEN {
        return Err(NameError::Length);
    }
    let mut out = [0; 16];
    for (i, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        out[i] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    if out == [0; 16] {
        return Err(NameError::ZeroNonce);
    }
    Ok(out)
}

pub fn hook_filename(nonce: &[u8; 16]) -> [u8; HOOK_FILENAME_LEN] {
    let mut out = [0; HOOK_FILENAME_LEN];
    out[..7].copy_from_slice(b"cshook-");
    out[7..39].copy_from_slice(&nonce_hex(nonce));
    out[39..].copy_from_slice(b".dll");
    out
}

/// Accepts a basename, never a full path; callers must first isolate the basename.
pub fn parse_hook_filename(bytes: &[u8]) -> Result<[u8; 16], NameError> {
    if bytes.len() != HOOK_FILENAME_LEN {
        return Err(NameError::Length);
    }
    if !bytes[..7].eq_ignore_ascii_case(b"cshook-") || !bytes[39..].eq_ignore_ascii_case(b".dll") {
        return Err(NameError::Format);
    }
    parse_nonce_hex(&bytes[7..39])
}

/// ASCII name without a terminating NUL. Widen into a caller-owned UTF-16 buffer.
pub fn mapping_name(nonce: &[u8; 16]) -> [u8; MAPPING_NAME_LEN] {
    let mut out = [0; MAPPING_NAME_LEN];
    out[..19].copy_from_slice(b"Local\\CoralSpyNext-");
    out[19..].copy_from_slice(&nonce_hex(nonce));
    out
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordError {
    InvalidLimits,
    WrongKind,
    InvalidField,
    TooManyRecords,
    TooLarge,
    Truncated,
    NoSpace,
}

fn check_record(
    kind: RecordKind,
    result_kind: ResultKind,
    depth: u32,
    id: u32,
    text_units: usize,
    limits: Limits,
) -> Result<(), RecordError> {
    if text_units > limits.max_text_units as usize {
        return Err(RecordError::TooLarge);
    }
    match (kind, result_kind) {
        (RecordKind::Column, ResultKind::ListView) => {
            // Column headers use id as the column index; depth is reserved zero.
            if depth != 0 || id >= limits.max_columns {
                return Err(RecordError::InvalidField);
            }
        }
        (RecordKind::Cell, ResultKind::ListView) => {
            // For a list cell, depth is the column index and id is the row index.
            if depth >= limits.max_columns || id >= limits.max_rows {
                return Err(RecordError::InvalidField);
            }
        }
        (RecordKind::TreeNode, ResultKind::TreeView) | (RecordKind::MenuItem, ResultKind::Menu) => {
            if depth >= limits.max_depth {
                return Err(RecordError::InvalidField);
            }
        }
        _ => return Err(RecordError::WrongKind),
    }
    Ok(())
}

/// Allocation-free, transactional record writer. A failed push changes neither
/// the buffer nor its counters. Text stays UTF-16, including unpaired surrogates.
pub struct RecordWriter<'a> {
    buffer: &'a mut [u8],
    kind: ResultKind,
    limits: Limits,
    position: usize,
    count: u32,
}

impl<'a> RecordWriter<'a> {
    pub fn new(
        buffer: &'a mut [u8],
        kind: ResultKind,
        limits: Limits,
    ) -> Result<Self, RecordError> {
        limits.validate().map_err(|_| RecordError::InvalidLimits)?;
        if kind == ResultKind::Rtf {
            return Err(RecordError::WrongKind);
        }
        Ok(Self {
            buffer,
            kind,
            limits,
            position: 0,
            count: 0,
        })
    }
    pub const fn bytes_written(&self) -> usize {
        self.position
    }
    pub const fn record_count(&self) -> u32 {
        self.count
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.buffer[..self.position]
    }

    pub fn push_column(
        &mut self,
        column: u32,
        flags: u32,
        text: &[u16],
    ) -> Result<(), RecordError> {
        self.push(RecordKind::Column, 0, column, flags, text)
    }
    pub fn push_cell(
        &mut self,
        row: u32,
        column: u32,
        flags: u32,
        text: &[u16],
    ) -> Result<(), RecordError> {
        self.push(RecordKind::Cell, column, row, flags, text)
    }
    pub fn push_tree(
        &mut self,
        depth: u32,
        id: u32,
        flags: u32,
        text: &[u16],
    ) -> Result<(), RecordError> {
        self.push(RecordKind::TreeNode, depth, id, flags, text)
    }
    pub fn push_menu(
        &mut self,
        depth: u32,
        id: u32,
        flags: u32,
        text: &[u16],
    ) -> Result<(), RecordError> {
        self.push(RecordKind::MenuItem, depth, id, flags, text)
    }
    fn push(
        &mut self,
        kind: RecordKind,
        depth: u32,
        id: u32,
        flags: u32,
        text: &[u16],
    ) -> Result<(), RecordError> {
        check_record(kind, self.kind, depth, id, text.len(), self.limits)?;
        if self.count >= self.limits.record_limit(self.kind) {
            return Err(RecordError::TooManyRecords);
        }
        let size = text
            .len()
            .checked_mul(2)
            .and_then(|n| n.checked_add(RECORD_HEADER_BYTES))
            .ok_or(RecordError::TooLarge)?;
        let end = self
            .position
            .checked_add(size)
            .ok_or(RecordError::TooLarge)?;
        if end > self.buffer.len() || end > self.limits.max_result_bytes as usize {
            return Err(RecordError::NoSpace);
        }
        let record = &mut self.buffer[self.position..end];
        for (slot, field) in record[..RECORD_HEADER_BYTES]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip([kind as u32, depth, id, flags, text.len() as u32])
        {
            slot.copy_from_slice(&field.to_le_bytes());
        }
        for (slot, unit) in record[RECORD_HEADER_BYTES..]
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(text)
        {
            slot.copy_from_slice(&unit.to_le_bytes());
        }
        self.position = end;
        self.count += 1;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Utf16Le<'a> {
    bytes: &'a [u8],
}
impl<'a> Utf16Le<'a> {
    pub const fn as_bytes(&self) -> &'a [u8] {
        self.bytes
    }
    pub const fn len(&self) -> usize {
        self.bytes.len() / 2
    }
    pub const fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}
impl Iterator for Utf16Le<'_> {
    type Item = u16;
    fn next(&mut self) -> Option<u16> {
        if self.bytes.is_empty() {
            return None;
        }
        let unit = u16::from_le_bytes([self.bytes[0], self.bytes[1]]);
        self.bytes = &self.bytes[2..];
        Some(unit)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len(), Some(self.len()))
    }
}
impl ExactSizeIterator for Utf16Le<'_> {}
impl core::iter::FusedIterator for Utf16Le<'_> {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Record<'a> {
    pub kind: RecordKind,
    /// Cell: column index. Column header: zero. Tree/menu: zero-based depth.
    pub depth: u32,
    /// Cell: row index. Column header: column index. Tree: producer's ID. Menu: command ID.
    pub id: u32,
    pub flags: u32,
    pub text: Utf16Le<'a>,
}

/// Parses unaligned bytes safely; rejects short headers/text, unknown types,
/// excessive lengths/counts and invalid row/column/depth before yielding data.
pub struct RecordReader<'a> {
    bytes: &'a [u8],
    kind: ResultKind,
    limits: Limits,
    position: usize,
    count: u32,
    failed: bool,
}

impl<'a> RecordReader<'a> {
    pub fn new(bytes: &'a [u8], kind: ResultKind, limits: Limits) -> Result<Self, RecordError> {
        limits.validate().map_err(|_| RecordError::InvalidLimits)?;
        if kind == ResultKind::Rtf {
            return Err(RecordError::WrongKind);
        }
        if bytes.len() > limits.max_result_bytes as usize {
            return Err(RecordError::TooLarge);
        }
        Ok(Self {
            bytes,
            kind,
            limits,
            position: 0,
            count: 0,
            failed: false,
        })
    }
    pub const fn record_count(&self) -> u32 {
        self.count
    }
    pub fn validate_count(mut self, expected_count: u32) -> Result<(), RecordError> {
        if self.failed {
            return Err(RecordError::InvalidField);
        }
        for record in &mut self {
            record?;
        }
        if self.count != expected_count {
            return Err(RecordError::InvalidField);
        }
        Ok(())
    }
    fn read(&mut self) -> Result<Record<'a>, RecordError> {
        if self.count >= self.limits.record_limit(self.kind) {
            return Err(RecordError::TooManyRecords);
        }
        let remaining = &self.bytes[self.position..];
        if remaining.len() < RECORD_HEADER_BYTES {
            return Err(RecordError::Truncated);
        }
        let mut fields = [0_u32; 5];
        for (field, slot) in fields
            .iter_mut()
            .zip(remaining[..RECORD_HEADER_BYTES].as_chunks::<4>().0.iter())
        {
            *field = u32::from_le_bytes([slot[0], slot[1], slot[2], slot[3]]);
        }
        let [kind, depth, id, flags, units] = fields;
        let kind = RecordKind::try_from(kind).map_err(|_| RecordError::WrongKind)?;
        check_record(kind, self.kind, depth, id, units as usize, self.limits)?;
        let size = (units as usize)
            .checked_mul(2)
            .and_then(|n| n.checked_add(RECORD_HEADER_BYTES))
            .ok_or(RecordError::TooLarge)?;
        let end = self
            .position
            .checked_add(size)
            .ok_or(RecordError::TooLarge)?;
        if end > self.bytes.len() {
            return Err(RecordError::Truncated);
        }
        let text = Utf16Le {
            bytes: &self.bytes[self.position + RECORD_HEADER_BYTES..end],
        };
        self.position = end;
        self.count += 1;
        Ok(Record {
            kind,
            depth,
            id,
            flags,
            text,
        })
    }
}

impl<'a> Iterator for RecordReader<'a> {
    type Item = Result<Record<'a>, RecordError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.position == self.bytes.len() {
            return None;
        }
        let record = self.read();
        if record.is_err() {
            self.failed = true;
        }
        Some(record)
    }
}
impl core::iter::FusedIterator for RecordReader<'_> {}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{vec, vec::Vec};

    const NONCE: [u8; 16] = [0x3a; 16];
    fn target() -> Target {
        Target {
            hwnd: 0x1234,
            pid: 123,
            tid: 456,
        }
    }
    fn request(operation: Operation) -> RequestHeader {
        RequestHeader::new(
            operation,
            Architecture::X64,
            NONCE,
            if operation == Operation::MenuDesktopOnce {
                Target::default()
            } else {
                target()
            },
            789,
            10_000,
            5_000,
        )
        .unwrap()
    }
    fn validate(value: &RequestHeader) -> Result<Operation, Status> {
        value.validate(&NONCE, Architecture::X64, 10_001)
    }
    fn set_limit(limits: &mut Limits, index: usize, value: u32) {
        match index {
            0 => limits.max_rows = value,
            1 => limits.max_columns = value,
            2 => limits.max_nodes = value,
            3 => limits.max_depth = value,
            4 => limits.max_text_units = value,
            5 => limits.max_result_bytes = value,
            _ => unreachable!(),
        }
    }

    #[test]
    fn abi_layout_is_fixed() {
        assert_eq!(core::mem::size_of::<RequestHeader>(), 112);
        assert_eq!(core::mem::size_of::<ResponseHeader>(), 72);
        assert_eq!(core::mem::size_of::<SharedMemory>(), 1_048_768);
        assert_eq!(core::mem::align_of::<RequestHeader>(), 8);
        assert_eq!(core::mem::align_of::<ResponseHeader>(), 8);
        assert_eq!(core::mem::offset_of!(SharedMemory, payload), 192);
        assert_eq!(core::mem::offset_of!(RequestHeader, nonce), 16);
        assert_eq!(core::mem::offset_of!(RequestHeader, operation), 32);
        assert_eq!(core::mem::offset_of!(RequestHeader, architecture), 36);
        assert_eq!(core::mem::offset_of!(RequestHeader, target_pid), 48);
        assert_eq!(core::mem::offset_of!(RequestHeader, target_tid), 52);
        assert_eq!(core::mem::offset_of!(RequestHeader, broker_pid), 56);
        assert_eq!(core::mem::offset_of!(RequestHeader, timeout_ms), 60);
        assert_eq!(core::mem::offset_of!(RequestHeader, max_rows), 80);
        assert_eq!(core::mem::offset_of!(RequestHeader, max_result_bytes), 100);
    }

    #[test]
    fn all_five_operations_validate_and_map_to_result_kinds() {
        for operation in [
            Operation::RichEditRtf,
            Operation::ListView,
            Operation::TreeView,
            Operation::MenuTarget,
            Operation::MenuDesktopOnce,
        ] {
            assert_eq!(validate(&request(operation)), Ok(operation));
            assert_eq!(
                operation as u32,
                Operation::try_from(operation as u32).unwrap() as u32
            );
        }
        assert_eq!(
            Operation::MenuTarget.result_kind(),
            Operation::MenuDesktopOnce.result_kind()
        );
    }

    #[test]
    fn header_identity_and_reserved_mutations_are_rejected() {
        let base = request(Operation::ListView);
        for index in 0..6 {
            let mut changed = base;
            match index {
                0 => changed.magic ^= 1,
                1 => changed.version += 1,
                2 => changed.header_bytes -= 1,
                3 => changed.mapping_bytes += 1,
                4 => changed.reserved[0] = 1,
                5 => changed.reserved[1] = u32::MAX,
                _ => unreachable!(),
            }
            assert_eq!(validate(&changed), Err(Status::InvalidRequest));
        }
    }

    #[test]
    fn every_nonce_bit_is_checked_and_zero_is_invalid() {
        let base = request(Operation::ListView);
        for byte in 0..16 {
            for bit in 0..8 {
                let mut changed = base;
                changed.nonce[byte] ^= 1 << bit;
                assert_eq!(validate(&changed), Err(Status::InvalidRequest));
            }
        }
        let mut changed = base;
        changed.nonce = [0; 16];
        assert_eq!(
            changed.validate(&[0; 16], Architecture::X64, 10_001),
            Err(Status::InvalidRequest)
        );
    }

    #[test]
    fn unknown_operations_and_architectures_are_rejected() {
        let base = request(Operation::ListView);
        for value in (0..1024).chain([u32::MAX]) {
            let mut changed = base;
            changed.operation = value;
            if !(1..=5).contains(&value) {
                assert_eq!(validate(&changed), Err(Status::InvalidRequest));
            }
            changed = base;
            changed.architecture = value;
            match value {
                64 => assert!(validate(&changed).is_ok()),
                32 => assert_eq!(validate(&changed), Err(Status::WrongArchitecture)),
                _ => assert_eq!(validate(&changed), Err(Status::InvalidRequest)),
            }
        }
    }

    #[test]
    fn targeted_operations_require_every_target_field() {
        for operation in [
            Operation::RichEditRtf,
            Operation::ListView,
            Operation::TreeView,
            Operation::MenuTarget,
        ] {
            for index in 0..4 {
                let mut changed = request(operation);
                match index {
                    0 => changed.target_hwnd = 0,
                    1 => changed.target_pid = 0,
                    2 => changed.target_tid = 0,
                    3 => changed.broker_pid = 0,
                    _ => unreachable!(),
                }
                assert_eq!(validate(&changed), Err(Status::InvalidRequest));
            }
        }
    }

    #[test]
    fn desktop_request_cannot_smuggle_target_fields() {
        for index in 0..3 {
            let mut changed = request(Operation::MenuDesktopOnce);
            match index {
                0 => changed.target_hwnd = 1,
                1 => changed.target_pid = 1,
                2 => changed.target_tid = 1,
                _ => unreachable!(),
            }
            assert_eq!(validate(&changed), Err(Status::InvalidRequest));
        }
    }

    #[test]
    fn x86_handles_and_broker_identity_are_checked() {
        let mut value = request(Operation::ListView);
        value.architecture = Architecture::X86 as u32;
        assert!(value.validate(&NONCE, Architecture::X86, 10_001).is_ok());
        value.target_hwnd = u64::from(u32::MAX) + 1;
        assert_eq!(
            value.validate(&NONCE, Architecture::X86, 10_001),
            Err(Status::InvalidRequest)
        );
        let value = request(Operation::ListView);
        assert!(value
            .validate_for_broker(&NONCE, 789, Architecture::X64, 10_001)
            .is_ok());
        for broker in [0, 1, 788, 790, u32::MAX] {
            assert_eq!(
                value.validate_for_broker(&NONCE, broker, Architecture::X64, 10_001),
                Err(Status::InvalidRequest)
            );
        }
    }

    #[test]
    fn deadlines_are_monotonic_bounded_and_overflow_safe() {
        let value = request(Operation::ListView);
        assert_eq!(
            value.validate(&NONCE, Architecture::X64, 9999),
            Err(Status::InvalidRequest)
        );
        assert!(value.validate(&NONCE, Architecture::X64, 10_000).is_ok());
        assert!(value.validate(&NONCE, Architecture::X64, 14_999).is_ok());
        for now in [15_000, 15_001, u64::MAX] {
            assert_eq!(
                value.validate(&NONCE, Architecture::X64, now),
                Err(Status::Expired)
            );
        }
        for expires in [0, 9999, 10_000, 15_001, u64::MAX] {
            let mut changed = value;
            changed.expires_at_ms = expires;
            assert_eq!(validate(&changed), Err(Status::InvalidRequest));
        }
        for timeout in [0, 10_001, u32::MAX] {
            let mut changed = value;
            changed.timeout_ms = timeout;
            assert_eq!(validate(&changed), Err(Status::InvalidRequest));
        }
        assert_eq!(
            RequestHeader::new(
                Operation::ListView,
                Architecture::X64,
                NONCE,
                target(),
                789,
                u64::MAX,
                1
            ),
            Err(Status::InvalidRequest)
        );
    }

    #[test]
    fn every_limit_rejects_zero_and_excess() {
        let maxima = [
            MAX_ROWS,
            MAX_COLUMNS,
            MAX_NODES,
            MAX_DEPTH,
            MAX_TEXT_UNITS,
            MAX_RESULT_BYTES as u32,
        ];
        for (index, maximum) in maxima.into_iter().enumerate() {
            for invalid in [0, maximum + 1, u32::MAX] {
                let mut limits = Limits::default();
                set_limit(&mut limits, index, invalid);
                assert_eq!(limits.validate(), Err(Status::InvalidRequest));
                assert!(RecordWriter::new(&mut [0; 32], ResultKind::ListView, limits).is_err());
                assert!(RecordReader::new(&[], ResultKind::ListView, limits).is_err());
                let mut changed = request(Operation::ListView);
                changed.max_rows = limits.max_rows;
                changed.max_columns = limits.max_columns;
                changed.max_nodes = limits.max_nodes;
                changed.max_depth = limits.max_depth;
                changed.max_text_units = limits.max_text_units;
                changed.max_result_bytes = limits.max_result_bytes;
                assert_eq!(validate(&changed), Err(Status::InvalidRequest));
            }
        }
    }

    #[test]
    fn names_round_trip_without_nul_or_paths() {
        assert_eq!(&nonce_hex(&NONCE), b"3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a");
        assert_eq!(parse_hook_filename(&hook_filename(&NONCE)), Ok(NONCE));
        assert_eq!(&mapping_name(&NONCE)[..19], b"Local\\CoralSpyNext-");
        assert_eq!(&hook_filename(&NONCE)[39..], b".dll");
        assert!(!mapping_name(&NONCE).contains(&0));
        assert_eq!(
            parse_hook_filename(b"cshook-3A3A3A3A3A3A3A3A3A3A3A3A3A3A3A3A.DLL"),
            Ok(NONCE)
        );
        assert_eq!(
            parse_nonce_hex(b"00000000000000000000000000000000"),
            Err(NameError::ZeroNonce)
        );
        assert!(parse_hook_filename(b"../cshook-3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a.dll").is_err());
    }

    #[test]
    fn malformed_nonce_and_filename_lengths_and_characters_fail() {
        let valid = hook_filename(&NONCE);
        for length in 0..valid.len() {
            assert!(parse_hook_filename(&valid[..length]).is_err());
        }
        for index in 7..39 {
            for bad in [0, b'/', b'\\', b'g', b' ', 0xff] {
                let mut changed = valid;
                changed[index] = bad;
                assert!(parse_hook_filename(&changed).is_err());
            }
        }
        let mut extended = [0; HOOK_FILENAME_LEN + 1];
        extended[..HOOK_FILENAME_LEN].copy_from_slice(&valid);
        assert_eq!(parse_hook_filename(&extended), Err(NameError::Length));
    }

    #[test]
    fn cells_round_trip_utf16_including_unpaired_surrogates() {
        let mut buffer = [0; 128];
        let units = [0, 65, 0x4e2d, 0xd83d, 0xde42, 0xd800];
        let mut writer =
            RecordWriter::new(&mut buffer, ResultKind::ListView, Limits::default()).unwrap();
        writer
            .push_cell(511, 31, RECORD_FLAG_SELECTED, &units)
            .unwrap();
        writer.push_cell(0, 0, 0, &[]).unwrap();
        assert_eq!(writer.record_count(), 2);
        assert_eq!(writer.bytes_written(), 52);
        let mut reader =
            RecordReader::new(writer.as_bytes(), ResultKind::ListView, Limits::default()).unwrap();
        let record = reader.next().unwrap().unwrap();
        assert_eq!(
            (record.kind, record.id, record.depth, record.flags),
            (RecordKind::Cell, 511, 31, RECORD_FLAG_SELECTED)
        );
        assert_eq!(record.text.len(), units.len());
        assert_eq!(record.text.collect::<Vec<_>>(), units);
        assert!(reader.next().unwrap().unwrap().text.is_empty());
        assert!(reader.next().is_none());
        assert_eq!(reader.record_count(), 2);
    }

    #[test]
    fn column_headers_round_trip_with_cells_and_count_together() {
        let limits = Limits {
            max_rows: 1,
            max_columns: 2,
            ..Limits::default()
        };
        let mut buffer = [0; 128];
        let title = [0x4e2d, 0xd83d, 0xde42, 0xd800];
        let mut writer = RecordWriter::new(&mut buffer, ResultKind::ListView, limits).unwrap();
        writer
            .push_column(0, RECORD_FLAG_TEXT_TRUNCATED, &title)
            .unwrap();
        writer.push_column(1, 0, &[]).unwrap();
        writer.push_cell(0, 0, 0, &[65]).unwrap();
        writer.push_cell(0, 1, 0, &[66]).unwrap();
        assert_eq!(writer.record_count(), 4);
        assert_eq!(
            writer.push_column(0, 0, &[]),
            Err(RecordError::TooManyRecords)
        );
        let records = RecordReader::new(writer.as_bytes(), ResultKind::ListView, limits)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(records.len(), 4);
        assert_eq!(
            (
                records[0].kind,
                records[0].depth,
                records[0].id,
                records[0].flags
            ),
            (RecordKind::Column, 0, 0, RECORD_FLAG_TEXT_TRUNCATED)
        );
        assert_eq!(records[0].text.collect::<Vec<_>>(), title);
        assert_eq!(
            (records[1].kind, records[1].depth, records[1].id),
            (RecordKind::Column, 0, 1)
        );
        assert_eq!(
            (records[2].kind, records[2].depth, records[2].id),
            (RecordKind::Cell, 0, 0)
        );
        assert_eq!(
            (records[3].kind, records[3].depth, records[3].id),
            (RecordKind::Cell, 1, 0)
        );
        assert_eq!(
            RecordReader::new(writer.as_bytes(), ResultKind::ListView, limits)
                .unwrap()
                .validate_count(4),
            Ok(())
        );
        let mut req = request(Operation::ListView);
        req.max_rows = 1;
        req.max_columns = 2;
        let mut res = response(&req);
        res.bytes_written = writer.bytes_written() as u32;
        res.record_count = 4;
        assert_eq!(
            res.validate_payload(&req, writer.as_bytes()),
            Ok(Status::Ok)
        );
        res.record_count = 5;
        assert_eq!(res.validate(&req), Err(Status::InvalidRequest));
    }

    #[test]
    fn column_writer_enforces_index_text_capacity_and_result_kind() {
        let mut buffer = [0; RECORD_HEADER_BYTES + 2 * MAX_TEXT_UNITS as usize];
        let mut writer =
            RecordWriter::new(&mut buffer, ResultKind::ListView, Limits::default()).unwrap();
        for column in [MAX_COLUMNS, u32::MAX] {
            assert_eq!(
                writer.push_column(column, 0, &[]),
                Err(RecordError::InvalidField)
            );
        }
        assert_eq!(
            writer.push_column(0, 0, &[0; MAX_TEXT_UNITS as usize + 1]),
            Err(RecordError::TooLarge)
        );
        assert_eq!((writer.bytes_written(), writer.record_count()), (0, 0));
        writer
            .push_column(MAX_COLUMNS - 1, 0, &[0; MAX_TEXT_UNITS as usize])
            .unwrap();
        assert_eq!(writer.bytes_written(), buffer.len());
        for kind in [ResultKind::TreeView, ResultKind::Menu] {
            let mut writer = RecordWriter::new(&mut buffer, kind, Limits::default()).unwrap();
            assert_eq!(writer.push_column(0, 0, &[]), Err(RecordError::WrongKind));
        }
        let limits = Limits {
            max_columns: 1,
            ..Limits::default()
        };
        let mut writer = RecordWriter::new(&mut buffer, ResultKind::ListView, limits).unwrap();
        assert_eq!(
            writer.push_column(1, 0, &[]),
            Err(RecordError::InvalidField)
        );
    }

    #[test]
    fn column_reader_rejects_nonzero_depth_bad_index_lengths_and_wrong_kind() {
        let mut buffer = [0; RECORD_HEADER_BYTES + 2];
        buffer[..4].copy_from_slice(&(RecordKind::Column as u32).to_le_bytes());
        buffer[16..20].copy_from_slice(&1_u32.to_le_bytes());
        buffer[20..22].copy_from_slice(&0xd800_u16.to_le_bytes());
        for length in 1..buffer.len() {
            let mut reader =
                RecordReader::new(&buffer[..length], ResultKind::ListView, Limits::default())
                    .unwrap();
            assert_eq!(reader.next(), Some(Err(RecordError::Truncated)));
            assert!(reader.next().is_none());
        }
        for (offset, value) in [(4, 1_u32), (4, u32::MAX), (8, MAX_COLUMNS), (8, u32::MAX)] {
            let mut changed = buffer;
            changed[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert_eq!(
                RecordReader::new(&changed, ResultKind::ListView, Limits::default())
                    .unwrap()
                    .next(),
                Some(Err(RecordError::InvalidField))
            );
        }
        for units in [MAX_TEXT_UNITS + 1, u32::MAX] {
            let mut changed = buffer;
            changed[16..20].copy_from_slice(&units.to_le_bytes());
            assert_eq!(
                RecordReader::new(&changed, ResultKind::ListView, Limits::default())
                    .unwrap()
                    .next(),
                Some(Err(RecordError::TooLarge))
            );
        }
        for kind in [ResultKind::TreeView, ResultKind::Menu] {
            assert_eq!(
                RecordReader::new(&buffer, kind, Limits::default())
                    .unwrap()
                    .next(),
                Some(Err(RecordError::WrongKind))
            );
        }
    }

    #[test]
    fn columns_and_cells_share_a_bounded_reader_record_budget() {
        let mut buffer = [0; RECORD_HEADER_BYTES * 3];
        let mut writer =
            RecordWriter::new(&mut buffer, ResultKind::ListView, Limits::default()).unwrap();
        writer.push_column(0, 0, &[]).unwrap();
        writer.push_cell(0, 0, 0, &[]).unwrap();
        writer.push_column(0, 0, &[]).unwrap();
        let limits = Limits {
            max_rows: 1,
            max_columns: 1,
            ..Limits::default()
        };
        let mut reader =
            RecordReader::new(writer.as_bytes(), ResultKind::ListView, limits).unwrap();
        assert_eq!(reader.next().unwrap().unwrap().kind, RecordKind::Column);
        assert_eq!(reader.next().unwrap().unwrap().kind, RecordKind::Cell);
        assert_eq!(reader.next(), Some(Err(RecordError::TooManyRecords)));
        assert!(reader.next().is_none());
    }

    #[test]
    fn tree_and_menu_preserve_depth_ids_and_flags() {
        for (kind, record_kind) in [
            (ResultKind::TreeView, RecordKind::TreeNode),
            (ResultKind::Menu, RecordKind::MenuItem),
        ] {
            let mut buffer = [0; 64];
            let mut writer = RecordWriter::new(&mut buffer, kind, Limits::default()).unwrap();
            let flags = RECORD_FLAG_DISABLED | RECORD_FLAG_HAS_CHILDREN;
            if kind == ResultKind::TreeView {
                writer.push_tree(31, u32::MAX, flags, &[0x4e2d]).unwrap();
            } else {
                writer.push_menu(31, u32::MAX, flags, &[0x4e2d]).unwrap();
            }
            let item = RecordReader::new(writer.as_bytes(), kind, Limits::default())
                .unwrap()
                .next()
                .unwrap()
                .unwrap();
            assert_eq!(
                (item.kind, item.depth, item.id, item.flags),
                (record_kind, 31, u32::MAX, flags)
            );
        }
    }

    #[test]
    fn writer_errors_never_modify_existing_output() {
        let mut buffer = [0xa5; 24];
        let mut writer =
            RecordWriter::new(&mut buffer, ResultKind::ListView, Limits::default()).unwrap();
        assert_eq!(
            writer.push_cell(512, 0, 0, &[]),
            Err(RecordError::InvalidField)
        );
        assert_eq!(
            writer.push_cell(0, 32, 0, &[]),
            Err(RecordError::InvalidField)
        );
        assert_eq!(writer.push_tree(0, 0, 0, &[]), Err(RecordError::WrongKind));
        assert_eq!(
            writer.push_cell(0, 0, 0, &[0; 2049]),
            Err(RecordError::TooLarge)
        );
        assert_eq!((writer.bytes_written(), writer.record_count()), (0, 0));
        writer.push_cell(0, 0, 0, &[1, 2]).unwrap();
        let before = writer.as_bytes().to_vec();
        assert_eq!(writer.push_cell(0, 1, 0, &[]), Err(RecordError::NoSpace));
        assert_eq!(writer.as_bytes(), before);
        assert_eq!((writer.bytes_written(), writer.record_count()), (24, 1));
    }

    #[test]
    fn writer_honors_custom_byte_depth_and_count_limits() {
        let mut buffer = [0; 100];
        let limits = Limits {
            max_nodes: 1,
            max_depth: 1,
            max_result_bytes: 20,
            ..Limits::default()
        };
        let mut writer = RecordWriter::new(&mut buffer, ResultKind::TreeView, limits).unwrap();
        assert_eq!(
            writer.push_tree(1, 0, 0, &[]),
            Err(RecordError::InvalidField)
        );
        assert_eq!(writer.push_tree(0, 0, 0, &[1]), Err(RecordError::NoSpace));
        writer.push_tree(0, 0, 0, &[]).unwrap();
        assert_eq!(
            writer.push_tree(0, 1, 0, &[]),
            Err(RecordError::TooManyRecords)
        );
        let limits = Limits {
            max_rows: 1,
            max_columns: 1,
            ..Limits::default()
        };
        let mut writer = RecordWriter::new(&mut buffer, ResultKind::ListView, limits).unwrap();
        writer.push_column(0, 0, &[]).unwrap();
        writer.push_cell(0, 0, 0, &[]).unwrap();
        assert_eq!(
            writer.push_cell(0, 0, 0, &[]),
            Err(RecordError::TooManyRecords)
        );
    }

    #[test]
    fn maximum_text_length_is_accepted() {
        let mut buffer = [0; RECORD_HEADER_BYTES + 2 * MAX_TEXT_UNITS as usize];
        let text = [0xd800; MAX_TEXT_UNITS as usize];
        let mut writer =
            RecordWriter::new(&mut buffer, ResultKind::Menu, Limits::default()).unwrap();
        writer.push_menu(0, 0, 0, &text).unwrap();
        assert_eq!(writer.bytes_written(), 4116);
        assert!(
            RecordReader::new(writer.as_bytes(), ResultKind::Menu, Limits::default())
                .unwrap()
                .validate_count(1)
                .is_ok()
        );
    }

    #[test]
    fn every_truncated_record_is_rejected_without_panicking() {
        let mut buffer = [0; 64];
        let mut writer =
            RecordWriter::new(&mut buffer, ResultKind::ListView, Limits::default()).unwrap();
        writer.push_cell(0, 0, 0, &[1, 2, 3, 4]).unwrap();
        for length in 1..writer.bytes_written() {
            let mut reader = RecordReader::new(
                &writer.as_bytes()[..length],
                ResultKind::ListView,
                Limits::default(),
            )
            .unwrap();
            assert_eq!(reader.next(), Some(Err(RecordError::Truncated)));
            assert!(reader.next().is_none());
            assert!(reader.validate_count(0).is_err());
        }
    }

    #[test]
    fn reader_rejects_oversized_and_overflow_lengths_before_slicing() {
        let mut buffer = [0; 20];
        buffer[..4].copy_from_slice(&(RecordKind::Cell as u32).to_le_bytes());
        for units in [2049, u32::MAX / 2, u32::MAX] {
            buffer[16..].copy_from_slice(&units.to_le_bytes());
            assert_eq!(
                RecordReader::new(&buffer, ResultKind::ListView, Limits::default())
                    .unwrap()
                    .next(),
                Some(Err(RecordError::TooLarge))
            );
        }
        assert!(RecordReader::new(
            &vec![0; MAX_RESULT_BYTES + 1],
            ResultKind::ListView,
            Limits::default()
        )
        .is_err());
        assert!(RecordReader::new(&[], ResultKind::Rtf, Limits::default()).is_err());
        assert!(RecordWriter::new(&mut [], ResultKind::Rtf, Limits::default()).is_err());
    }

    #[test]
    fn malformed_record_types_depth_rows_and_columns_are_rejected() {
        let mut buffer = [0; 20];
        buffer[..4].copy_from_slice(&(RecordKind::Cell as u32).to_le_bytes());
        for kind in [0, 2, 3, 5, u32::MAX] {
            let mut changed = buffer;
            changed[..4].copy_from_slice(&kind.to_le_bytes());
            assert_eq!(
                RecordReader::new(&changed, ResultKind::ListView, Limits::default())
                    .unwrap()
                    .next(),
                Some(Err(RecordError::WrongKind))
            );
        }
        for (offset, value) in [(4, 32_u32), (8, 512_u32), (4, u32::MAX), (8, u32::MAX)] {
            let mut changed = buffer;
            changed[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert_eq!(
                RecordReader::new(&changed, ResultKind::ListView, Limits::default())
                    .unwrap()
                    .next(),
                Some(Err(RecordError::InvalidField))
            );
        }
        buffer[..4].copy_from_slice(&(RecordKind::TreeNode as u32).to_le_bytes());
        buffer[4..8].copy_from_slice(&32_u32.to_le_bytes());
        assert_eq!(
            RecordReader::new(&buffer, ResultKind::TreeView, Limits::default())
                .unwrap()
                .next(),
            Some(Err(RecordError::InvalidField))
        );
    }

    #[test]
    fn reader_rejects_extra_records_and_declared_count_mismatch() {
        let mut buffer = [0; 40];
        let mut writer =
            RecordWriter::new(&mut buffer, ResultKind::TreeView, Limits::default()).unwrap();
        writer.push_tree(0, 0, 0, &[]).unwrap();
        writer.push_tree(0, 1, 0, &[]).unwrap();
        let limits = Limits {
            max_nodes: 1,
            ..Limits::default()
        };
        let mut reader =
            RecordReader::new(writer.as_bytes(), ResultKind::TreeView, limits).unwrap();
        assert!(reader.next().unwrap().is_ok());
        assert_eq!(reader.next(), Some(Err(RecordError::TooManyRecords)));
        assert!(reader.next().is_none());
        for count in [0, 1, 3, u32::MAX] {
            assert_eq!(
                RecordReader::new(writer.as_bytes(), ResultKind::TreeView, Limits::default())
                    .unwrap()
                    .validate_count(count),
                Err(RecordError::InvalidField)
            );
        }
    }

    fn response(request: &RequestHeader) -> ResponseHeader {
        ResponseHeader {
            actual_hwnd: request.target_hwnd,
            actual_pid: request.target_pid,
            actual_tid: request.target_tid,
            ..ResponseHeader::new(
                Status::Ok,
                Operation::try_from(request.operation)
                    .unwrap()
                    .result_kind(),
                0,
                0,
            )
        }
    }

    #[test]
    fn response_metadata_checks_lengths_kinds_flags_and_target_identity() {
        let request = request(Operation::ListView);
        let base = response(&request);
        assert_eq!(base.validate_payload(&request, &[]), Ok(Status::Ok));
        for index in 0..12 {
            let mut value = base;
            match index {
                0 => value.status = u32::MAX,
                1 => value.kind = 0,
                2 => value.kind = ResultKind::Menu as u32,
                3 => value.bytes_written = MAX_RESULT_BYTES as u32 + 1,
                4 => value.record_count = 1,
                5 => value.flags = 2,
                6 => value.flags = RESULT_FLAG_TRUNCATED,
                7 => value.reserved[1] = 1,
                8 => value.actual_hwnd = 0,
                9 => value.actual_pid += 1,
                10 => value.actual_tid += 1,
                11 => value.status = Status::Truncated as u32,
                _ => unreachable!(),
            }
            assert_eq!(
                value.validate(&request),
                Err(Status::InvalidRequest),
                "case {index}"
            );
        }
    }

    #[test]
    fn truncated_response_is_explicit_and_error_responses_cannot_carry_data() {
        let request = request(Operation::ListView);
        let mut value = response(&request);
        value.status = Status::Truncated as u32;
        value.flags = RESULT_FLAG_TRUNCATED;
        assert_eq!(value.validate(&request), Ok(Status::Truncated));
        for status in [
            Status::InvalidRequest,
            Status::ClassMismatch,
            Status::PasswordControl,
            Status::AccessDenied,
            Status::Expired,
            Status::Unsupported,
            Status::ControlError,
            Status::WrongArchitecture,
            Status::Cancelled,
        ] {
            let mut value = ResponseHeader::new(status, ResultKind::ListView, 0, 0);
            assert_eq!(value.validate(&request), Ok(status));
            value.bytes_written = 20;
            assert_eq!(value.validate(&request), Err(Status::InvalidRequest));
        }
    }

    #[test]
    fn desktop_response_requires_actual_event_identity() {
        let request = request(Operation::MenuDesktopOnce);
        let mut value = response(&request);
        assert_eq!(value.validate(&request), Err(Status::InvalidRequest));
        value.actual_hwnd = target().hwnd;
        value.actual_pid = target().pid;
        value.actual_tid = target().tid;
        value.root_menu = 0x8765;
        assert_eq!(value.validate(&request), Ok(Status::Ok));
    }

    #[test]
    fn response_payload_validation_checks_every_record_and_exact_count() {
        let request = request(Operation::ListView);
        let mut buffer = [0; 40];
        let mut writer =
            RecordWriter::new(&mut buffer, ResultKind::ListView, Limits::default()).unwrap();
        writer.push_cell(0, 0, 0, &[]).unwrap();
        writer.push_cell(1, 0, 0, &[]).unwrap();
        let mut value = response(&request);
        value.bytes_written = 40;
        value.record_count = 2;
        assert_eq!(value.validate_payload(&request, &buffer), Ok(Status::Ok));
        value.record_count = 1;
        assert_eq!(
            value.validate_payload(&request, &buffer),
            Err(Status::InvalidRequest)
        );
        value.record_count = 2;
        assert_eq!(
            value.validate_payload(&request, &buffer[..39]),
            Err(Status::InvalidRequest)
        );
        buffer[36..40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            value.validate_payload(&request, &buffer),
            Err(Status::InvalidRequest)
        );
    }

    #[test]
    fn raw_rtf_remains_opaque_and_bounded() {
        let request = request(Operation::RichEditRtf);
        let mut value = response(&request);
        value.bytes_written = 3;
        assert_eq!(
            value.validate_payload(&request, &[0xff, 0, 0x80]),
            Ok(Status::Ok)
        );
        value.record_count = 1;
        assert_eq!(value.validate(&request), Err(Status::InvalidRequest));
    }

    #[test]
    fn deterministic_byte_mutation_fuzz_is_bounded_and_panic_free() {
        let mut seed = 0x1267_3456_u32;
        for iteration in 0..20_000 {
            let mut bytes = [0_u8; 256];
            for byte in &mut bytes {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *byte = seed as u8;
            }
            let length = (seed as usize) % (bytes.len() + 1);
            let kind = match iteration % 3 {
                0 => ResultKind::ListView,
                1 => ResultKind::TreeView,
                _ => ResultKind::Menu,
            };
            if iteration % 2 == 0 && length >= 20 {
                bytes[..4].copy_from_slice(&(iteration % 3 + 1_u32).to_le_bytes());
                bytes[4..8].copy_from_slice(&(iteration % 40).to_le_bytes());
                bytes[8..12].copy_from_slice(&(iteration % 600).to_le_bytes());
                bytes[16..20].copy_from_slice(&(iteration % 128).to_le_bytes());
            }
            let mut reader = RecordReader::new(&bytes[..length], kind, Limits::default()).unwrap();
            let mut steps = 0;
            for result in reader.by_ref() {
                steps += 1;
                assert!(steps <= length / RECORD_HEADER_BYTES + 1);
                if let Ok(record) = result {
                    assert!(record.text.len() <= MAX_TEXT_UNITS as usize);
                }
            }
            assert!(reader.next().is_none());
            let _ = parse_hook_filename(&bytes[..length]);
            let _ = parse_nonce_hex(&bytes[..length]);
        }
    }
}
