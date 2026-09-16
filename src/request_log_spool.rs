//! Crash-recoverable append-only storage for terminal request-log events.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use crc32fast::hash;
use fs2::FileExt;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use uuid::Uuid;

use crate::{
    application::RequestLogIntent,
    domain::RequestLogEvent,
    request_log_admission::{AdmissionStore, RESERVATION_BYTES},
    request_log_journal::{EncodedRequestLog, JournalCodecError},
};

const FRAME_MAGIC: [u8; 4] = *b"AIGL";
const FRAME_HEADER_BYTES: usize = 32;
pub(crate) const MAX_PAYLOAD_BYTES: usize = 1_048_576;
const CHECKPOINT_FILE: &str = "checkpoint";
const CHECKPOINT_TEMP_FILE: &str = "checkpoint.tmp";
const EVENTS_FILE: &str = "events.log";
const LOCK_FILE: &str = "spool.lock";

struct SpoolWriter {
    file: File,
    end_offset: u64,
    failed: bool,
    admissions: AdmissionStore,
}

pub(crate) struct RequestLogSpool {
    directory: PathBuf,
    events_path: PathBuf,
    checkpoint_path: PathBuf,
    checkpoint_temp_path: PathBuf,
    writer: Mutex<SpoolWriter>,
    end_offset: AtomicU64,
    checkpoint_offset: AtomicU64,
    synced_offset: AtomicU64,
    compaction_threshold_bytes: u64,
    max_bytes: u64,
    min_free_bytes: u64,
    capacity_pressure: AtomicBool,
    _lock: File,
}

impl RequestLogSpool {
    #[cfg(test)]
    pub(crate) fn open(
        directory: impl AsRef<Path>,
        compaction_threshold_bytes: u64,
    ) -> Result<Self, SpoolError> {
        Self::open_with_limits(
            directory,
            compaction_threshold_bytes,
            1_073_741_824,
            67_108_864,
        )
    }

    pub(crate) fn open_with_limits(
        directory: impl AsRef<Path>,
        compaction_threshold_bytes: u64,
        max_bytes: u64,
        min_free_bytes: u64,
    ) -> Result<Self, SpoolError> {
        let directory = directory.as_ref().to_path_buf();
        create_durable_directory(&directory)?;
        secure_directory(&directory)?;
        let lock_path = directory.join(LOCK_FILE);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        secure_file(&lock)?;
        lock.try_lock_exclusive()
            .map_err(|source| SpoolError::Lock { source })?;

        let events_path = directory.join(EVENTS_FILE);
        let checkpoint_path = directory.join(CHECKPOINT_FILE);
        let checkpoint_temp_path = directory.join(CHECKPOINT_TEMP_FILE);
        let checkpoint_offset = read_checkpoint(&checkpoint_path)?;
        let mut scanner = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&events_path)?;
        secure_file(&scanner)?;
        let scan = scan_and_repair(&mut scanner, checkpoint_offset)?;
        if !scan.checkpoint_is_boundary {
            return Err(SpoolError::InvalidCheckpoint {
                checkpoint: checkpoint_offset,
                end: scan.end_offset,
            });
        }
        let writer = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&events_path)?;
        let (admissions, recovered) = AdmissionStore::open(&directory)?;
        File::open(&directory)?.sync_all()?;

        let spool = Self {
            directory,
            events_path,
            checkpoint_path,
            checkpoint_temp_path,
            writer: Mutex::new(SpoolWriter {
                file: writer,
                end_offset: scan.end_offset,
                failed: false,
                admissions,
            }),
            end_offset: AtomicU64::new(scan.end_offset),
            checkpoint_offset: AtomicU64::new(checkpoint_offset),
            synced_offset: AtomicU64::new(scan.end_offset),
            compaction_threshold_bytes,
            max_bytes,
            min_free_bytes,
            capacity_pressure: AtomicBool::new(false),
            _lock: lock,
        };
        for event in recovered {
            spool.append(&event)?;
        }
        Ok(spool)
    }

    pub(crate) fn admit(&self, intent: &RequestLogIntent) -> Result<(), SpoolError> {
        let mut writer = self.writer.lock().map_err(|_| SpoolError::Poisoned)?;
        if writer.failed {
            return Err(SpoolError::UnavailableAfterWriteFailure);
        }
        let used = writer
            .end_offset
            .saturating_add(writer.admissions.reserved_bytes());
        if used.saturating_add(2 * RESERVATION_BYTES) > self.max_bytes
            || fs2::available_space(&self.directory)?
                < self
                    .min_free_bytes
                    .saturating_add(writer.admissions.terminal_headroom())
                    .saturating_add(2 * RESERVATION_BYTES)
        {
            self.capacity_pressure.store(true, Ordering::Release);
            return Err(SpoolError::Capacity);
        }
        if let Err(error) = writer.admissions.reserve(intent) {
            writer.failed = true;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn append(&self, event: &RequestLogEvent) -> Result<u64, SpoolError> {
        let record = EncodedRequestLog::encode(event)?;
        if record.payload.len() > MAX_PAYLOAD_BYTES {
            return Err(SpoolError::PayloadTooLarge {
                bytes: record.payload.len(),
            });
        }
        let payload_len =
            u32::try_from(record.payload.len()).map_err(|_| SpoolError::PayloadTooLarge {
                bytes: record.payload.len(),
            })?;
        let mut frame = Vec::with_capacity(FRAME_HEADER_BYTES + record.payload.len());
        frame.extend_from_slice(&FRAME_MAGIC);
        frame.extend_from_slice(&record.schema_version.to_le_bytes());
        frame.extend_from_slice(&0_u16.to_le_bytes());
        frame.extend_from_slice(&payload_len.to_le_bytes());
        frame.extend_from_slice(&hash(&record.payload).to_le_bytes());
        frame.extend_from_slice(record.request_log_id.as_bytes());
        frame.extend_from_slice(&record.payload);

        let mut writer = self.writer.lock().map_err(|_| SpoolError::Poisoned)?;
        let reserved = writer.admissions.contains(event.id);
        // Even after an events.log failure, an admitted in-flight request
        // still owns a preallocated slot for its exact terminal evidence.
        if reserved && let Err(error) = writer.admissions.save_terminal(event) {
            writer.failed = true;
            return Err(error);
        }
        if writer.failed {
            return Err(SpoolError::UnavailableAfterWriteFailure);
        }
        if !reserved
            && writer
                .end_offset
                .saturating_add(writer.admissions.reserved_bytes())
                .saturating_add(frame.len() as u64)
                > self.max_bytes
        {
            return Err(SpoolError::Capacity);
        }
        if let Err(error) = writer.file.write_all(&frame) {
            // A partial frame may now exist after the published end offset.
            // Refusing later appends lets startup recovery truncate only that
            // final torn frame instead of interleaving new valid records.
            writer.failed = true;
            return Err(error.into());
        }
        writer.end_offset = writer
            .end_offset
            .checked_add(frame.len() as u64)
            .ok_or(SpoolError::OffsetOverflow)?;
        if reserved && let Err(error) = writer.file.sync_data() {
            writer.failed = true;
            return Err(error.into());
        }
        self.end_offset.store(writer.end_offset, Ordering::Release);
        if reserved && let Err(error) = writer.admissions.retire(event.id) {
            writer.failed = true;
            return Err(error);
        }
        Ok(frame.len() as u64)
    }

    pub(crate) async fn reader(self: &Arc<Self>) -> Result<SpoolReader, SpoolError> {
        let mut file = tokio::fs::File::open(&self.events_path).await?;
        let offset = self.checkpoint_offset();
        file.seek(SeekFrom::Start(offset)).await?;
        Ok(SpoolReader {
            spool: Arc::clone(self),
            file,
            offset,
        })
    }

    pub(crate) fn checkpoint(&self, offset: u64) -> Result<(), SpoolError> {
        let current = self.checkpoint_offset();
        let end = self.end_offset();
        if offset < current || offset > end {
            return Err(SpoolError::InvalidCheckpoint {
                checkpoint: offset,
                end,
            });
        }
        persist_checkpoint(
            &self.checkpoint_path,
            &self.checkpoint_temp_path,
            offset,
            false,
        )?;
        self.checkpoint_offset.store(offset, Ordering::Release);
        Ok(())
    }

    pub(crate) fn sync_data(&self) -> Result<(), SpoolError> {
        let mut writer = self.writer.lock().map_err(|_| SpoolError::Poisoned)?;
        let target = self.end_offset();
        let synced = self.synced_offset.load(Ordering::Acquire);
        if target <= synced {
            return Ok(());
        }
        if self.checkpoint_offset() >= target {
            self.synced_offset.store(target, Ordering::Release);
            return Ok(());
        }
        if let Err(error) = writer.file.sync_data() {
            writer.failed = true;
            return Err(error.into());
        }
        self.synced_offset.store(target, Ordering::Release);
        Ok(())
    }

    pub(crate) fn compact_if_drained(&self) -> Result<bool, SpoolError> {
        let checkpoint = self.checkpoint_offset();
        let end = self.end_offset();
        let pressure = self.capacity_pressure.load(Ordering::Acquire);
        if checkpoint != end || end == 0 || (!pressure && end < self.compaction_threshold_bytes) {
            return Ok(false);
        }

        let mut writer = self.writer.lock().map_err(|_| SpoolError::Poisoned)?;
        if writer.failed {
            return Err(SpoolError::UnavailableAfterWriteFailure);
        }
        let checkpoint = self.checkpoint_offset();
        if checkpoint != writer.end_offset
            || (!pressure && writer.end_offset < self.compaction_threshold_bytes)
        {
            return Ok(false);
        }
        let compact = (|| -> Result<(), SpoolError> {
            writer.file.sync_data()?;
            // Writing zero first is crash-safe: a crash before truncation merely
            // replays already committed idempotent rows.
            persist_checkpoint(&self.checkpoint_path, &self.checkpoint_temp_path, 0, true)?;
            File::open(&self.directory)?.sync_all()?;
            writer.file.set_len(0)?;
            writer.file.seek(SeekFrom::Start(0))?;
            writer.file.sync_data()?;
            Ok(())
        })();
        if let Err(error) = compact {
            writer.failed = true;
            return Err(error);
        }
        writer.end_offset = 0;
        self.checkpoint_offset.store(0, Ordering::Release);
        self.end_offset.store(0, Ordering::Release);
        self.synced_offset.store(0, Ordering::Release);
        self.capacity_pressure.store(false, Ordering::Release);
        Ok(true)
    }

    pub(crate) fn end_offset(&self) -> u64 {
        self.end_offset.load(Ordering::Acquire)
    }

    pub(crate) fn checkpoint_offset(&self) -> u64 {
        self.checkpoint_offset.load(Ordering::Acquire)
    }

    pub(crate) fn pending_bytes(&self) -> u64 {
        self.end_offset().saturating_sub(self.checkpoint_offset())
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }
}

fn create_durable_directory(path: &Path) -> Result<(), io::Error> {
    if path.is_dir() {
        return Ok(());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    create_durable_directory(parent)?;
    fs::create_dir(path)?;
    secure_directory(path)?;
    File::open(path)?.sync_all()?;
    File::open(parent)?.sync_all()
}

pub(crate) struct SpoolReader {
    spool: Arc<RequestLogSpool>,
    file: tokio::fs::File,
    offset: u64,
}

impl SpoolReader {
    pub(crate) async fn read_batch(
        &mut self,
        max_records: usize,
    ) -> Result<SpoolReadBatch, SpoolError> {
        let start_offset = self.offset;
        let available_end = self.spool.end_offset();
        if max_records == 0 || start_offset >= available_end {
            return Ok(SpoolReadBatch {
                records: Vec::new(),
                start_offset,
                end_offset: start_offset,
            });
        }
        self.file.seek(SeekFrom::Start(start_offset)).await?;
        let mut records = Vec::with_capacity(max_records);
        while records.len() < max_records && self.offset < available_end {
            let mut header = [0_u8; FRAME_HEADER_BYTES];
            self.file.read_exact(&mut header).await?;
            let parsed = parse_header(&header)?;
            let frame_end = self
                .offset
                .checked_add(FRAME_HEADER_BYTES as u64)
                .and_then(|offset| offset.checked_add(parsed.payload_len as u64))
                .ok_or(SpoolError::OffsetOverflow)?;
            if frame_end > available_end {
                return Err(SpoolError::Corrupt(
                    "published spool frame extends beyond the durable end offset",
                ));
            }
            let mut payload = vec![0_u8; parsed.payload_len];
            self.file.read_exact(&mut payload).await?;
            if hash(&payload) != parsed.checksum {
                return Err(SpoolError::Corrupt(
                    "request-log spool frame checksum mismatch",
                ));
            }
            records.push(EncodedRequestLog {
                request_log_id: parsed.request_log_id,
                schema_version: parsed.schema_version,
                payload,
            });
            self.offset = frame_end;
        }
        Ok(SpoolReadBatch {
            records,
            start_offset,
            end_offset: self.offset,
        })
    }

    pub(crate) async fn reset(&mut self, offset: u64) -> Result<(), SpoolError> {
        self.file.seek(SeekFrom::Start(offset)).await?;
        self.offset = offset;
        Ok(())
    }
}

pub(crate) struct SpoolReadBatch {
    pub records: Vec<EncodedRequestLog>,
    pub start_offset: u64,
    pub end_offset: u64,
}

struct ParsedHeader {
    schema_version: i16,
    payload_len: usize,
    checksum: u32,
    request_log_id: Uuid,
}

fn parse_header(header: &[u8; FRAME_HEADER_BYTES]) -> Result<ParsedHeader, SpoolError> {
    if header[..4] != FRAME_MAGIC {
        return Err(SpoolError::Corrupt(
            "request-log spool frame magic is invalid",
        ));
    }
    let schema_version = i16::from_le_bytes([header[4], header[5]]);
    let reserved = u16::from_le_bytes([header[6], header[7]]);
    if reserved != 0 {
        return Err(SpoolError::Corrupt(
            "request-log spool frame reserved bits are nonzero",
        ));
    }
    let payload_len = u32::from_le_bytes([header[8], header[9], header[10], header[11]]) as usize;
    if payload_len > MAX_PAYLOAD_BYTES {
        return Err(SpoolError::PayloadTooLarge { bytes: payload_len });
    }
    let checksum = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
    let request_log_id = Uuid::from_slice(&header[16..32])
        .map_err(|_| SpoolError::Corrupt("request-log spool UUID is invalid"))?;
    Ok(ParsedHeader {
        schema_version,
        payload_len,
        checksum,
        request_log_id,
    })
}

struct ScanResult {
    end_offset: u64,
    checkpoint_is_boundary: bool,
}

fn scan_and_repair(file: &mut File, checkpoint: u64) -> Result<ScanResult, SpoolError> {
    let file_len = file.metadata()?.len();
    let mut offset = 0_u64;
    let mut checkpoint_is_boundary = checkpoint == 0;
    while offset < file_len {
        let remaining = file_len - offset;
        if remaining < FRAME_HEADER_BYTES as u64 {
            file.set_len(offset)?;
            file.sync_data()?;
            return Ok(ScanResult {
                end_offset: offset,
                checkpoint_is_boundary,
            });
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0_u8; FRAME_HEADER_BYTES];
        file.read_exact(&mut header)?;
        let parsed = parse_header(&header)?;
        let frame_end = offset
            .checked_add(FRAME_HEADER_BYTES as u64)
            .and_then(|value| value.checked_add(parsed.payload_len as u64))
            .ok_or(SpoolError::OffsetOverflow)?;
        if frame_end > file_len {
            file.set_len(offset)?;
            file.sync_data()?;
            return Ok(ScanResult {
                end_offset: offset,
                checkpoint_is_boundary,
            });
        }
        let mut payload = vec![0_u8; parsed.payload_len];
        file.read_exact(&mut payload)?;
        if hash(&payload) != parsed.checksum {
            return Err(SpoolError::Corrupt(
                "request-log spool frame checksum mismatch",
            ));
        }
        offset = frame_end;
        if offset == checkpoint {
            checkpoint_is_boundary = true;
        }
    }
    Ok(ScanResult {
        end_offset: offset,
        checkpoint_is_boundary,
    })
}

fn read_checkpoint(path: &Path) -> Result<u64, SpoolError> {
    match fs::read(path) {
        Ok(bytes) => {
            if bytes.len() != size_of::<u64>() {
                return Err(SpoolError::Corrupt(
                    "request-log spool checkpoint has an invalid length",
                ));
            }
            Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| {
                SpoolError::Corrupt("request-log checkpoint is invalid")
            })?))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error.into()),
    }
}

fn persist_checkpoint(
    path: &Path,
    temp_path: &Path,
    offset: u64,
    sync: bool,
) -> Result<(), SpoolError> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(temp_path)?;
    file.write_all(&offset.to_le_bytes())?;
    secure_file(&file)?;
    if sync {
        file.sync_all()?;
    }
    fs::rename(temp_path, path)?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn secure_directory(path: &Path) -> Result<(), io::Error> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
pub(crate) fn secure_directory(_: &Path) -> Result<(), io::Error> {
    Ok(())
}

#[cfg(unix)]
pub(crate) fn secure_file(file: &File) -> Result<(), io::Error> {
    use std::os::unix::fs::PermissionsExt;

    file.set_permissions(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
pub(crate) fn secure_file(_: &File) -> Result<(), io::Error> {
    Ok(())
}

#[derive(Debug, Error)]
pub(crate) enum SpoolError {
    #[error("request-log spool I/O failed")]
    Io(#[from] io::Error),
    #[error("request-log spool is already owned by another process")]
    Lock {
        #[source]
        source: io::Error,
    },
    #[error("request-log spool codec failed")]
    Codec(#[from] JournalCodecError),
    #[error("request-log spool payload is too large: {bytes} bytes")]
    PayloadTooLarge { bytes: usize },
    #[error("request-log spool offset overflowed")]
    OffsetOverflow,
    #[error("request-log spool mutex was poisoned")]
    Poisoned,
    #[error("request-log spool is unavailable after an earlier partial write")]
    UnavailableAfterWriteFailure,
    #[error("request-log spool capacity or free-space reserve exhausted")]
    Capacity,
    #[error("request-log spool checkpoint {checkpoint} is invalid for end offset {end}")]
    InvalidCheckpoint { checkpoint: u64, end: u64 },
    #[error("{0}")]
    Corrupt(&'static str),
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, OpenOptions},
        io::{Seek, SeekFrom, Write},
        sync::Arc,
    };

    use chrono::Utc;
    use uuid::Uuid;

    use super::{RequestLogSpool, SpoolError};
    use crate::domain::{
        ApiFormat, ApiOperation, RequestLogEvent, RequestLogOutcome, RequestLogSource,
        RequestProtocol,
    };
    use crate::{application::RequestLogIntent, request_log_admission::RESERVATION_BYTES};

    fn directory() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ai-gateway-spool-test-{}", Uuid::new_v4()))
    }

    fn event() -> RequestLogEvent {
        let now = Utc::now();
        RequestLogEvent {
            id: Uuid::new_v4(),
            started_at: now,
            completed_at: now,
            user_id: Uuid::new_v4(),
            api_key_id: Uuid::new_v4(),
            request_source: RequestLogSource::Client,
            api_format: ApiFormat::OpenAiResponses,
            api_operation: ApiOperation::Responses,
            request_protocol: RequestProtocol::NonStream,
            client_model: "spool-test".into(),
            reasoning_effort: None,
            fast_mode: false,
            upstream_model: None,
            model_rule_id: None,
            channel_group_id: None,
            channel_id: None,
            model_id: None,
            outcome: RequestLogOutcome::Rejected,
            response_status_code: Some(404),
            streamed: false,
            ttft_ms: None,
            total_duration_ms: 1,
            billing: None,
            error_code: Some("model_not_found".into()),
            error_summary: None,
        }
    }

    fn intent(event: &RequestLogEvent) -> RequestLogIntent {
        RequestLogIntent {
            version: 1,
            id: event.id,
            user_id: event.user_id,
            api_key_id: event.api_key_id,
            model_id: Uuid::new_v4(),
            started_at: event.started_at,
            api_operation: event.api_operation,
            request_protocol: event.request_protocol,
        }
    }

    #[tokio::test]
    async fn admission_synchronizes_terminal_and_retires_reservation() {
        let directory = directory();
        let event = event();
        let spool = Arc::new(RequestLogSpool::open(&directory, u64::MAX).unwrap());
        spool.admit(&intent(&event)).unwrap();
        let path = directory
            .join("admissions")
            .join(format!("{}.json", event.id));
        assert!(path.exists());
        assert!(spool.writer.lock().unwrap().admissions.contains(event.id));
        spool.append(&event).unwrap();
        assert!(!path.exists());
        assert!(!spool.writer.lock().unwrap().admissions.contains(event.id));
        let mut reader = spool.reader().await.unwrap();
        assert_eq!(
            reader.read_batch(10).await.unwrap().records[0].request_log_id,
            event.id
        );
        drop(reader);
        drop(spool);
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn admitted_terminal_survives_latched_spool_failure_and_replays_once() {
        let directory = directory();
        let event = event();
        {
            let spool = RequestLogSpool::open(&directory, u64::MAX).unwrap();
            spool.admit(&intent(&event)).unwrap();
            spool.writer.lock().unwrap().failed = true;
            assert!(spool.append(&event).is_err());
            assert!(spool.admit(&intent(&self::event())).is_err());
            assert_eq!(spool.end_offset(), 0);
        }
        {
            let spool = Arc::new(RequestLogSpool::open(&directory, u64::MAX).unwrap());
            let mut reader = spool.reader().await.unwrap();
            let batch = reader.read_batch(10).await.unwrap();
            assert_eq!(batch.records.len(), 1);
            assert_eq!(batch.records[0].decode().unwrap().id, event.id);
            spool.checkpoint(batch.end_offset).unwrap();
        }
        {
            let spool = Arc::new(RequestLogSpool::open(&directory, u64::MAX).unwrap());
            assert!(
                spool
                    .reader()
                    .await
                    .unwrap()
                    .read_batch(10)
                    .await
                    .unwrap()
                    .records
                    .is_empty()
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unknown_intents_are_retained_without_active_reservations_or_fabricated_events() {
        let directory = directory();
        let event = event();
        {
            let spool = RequestLogSpool::open(&directory, u64::MAX).unwrap();
            spool.admit(&intent(&event)).unwrap();
        }
        for _ in 0..3 {
            let spool = RequestLogSpool::open(&directory, u64::MAX).unwrap();
            assert_eq!(spool.end_offset(), 0);
            let writer = spool.writer.lock().unwrap();
            assert!(!writer.admissions.contains(event.id));
            assert!(writer.admissions.reserved_bytes() < RESERVATION_BYTES);
            drop(writer);
            let next = self::event();
            spool.admit(&intent(&next)).unwrap();
            spool.append(&next).unwrap();
            spool.checkpoint(spool.end_offset()).unwrap();
            // Preserve the original unknown intent while draining real terminals.
            spool
                .capacity_pressure
                .store(true, std::sync::atomic::Ordering::Release);
            spool.compact_if_drained().unwrap();
        }
        assert!(
            directory
                .join("admissions")
                .join(format!("{}.json", event.id))
                .exists()
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn capacity_recovers_after_pressure_compaction_below_normal_threshold() {
        let directory = directory();
        let spool = RequestLogSpool::open_with_limits(
            &directory,
            RESERVATION_BYTES,
            2 * RESERVATION_BYTES,
            0,
        )
        .unwrap();
        let first = event();
        spool.admit(&intent(&first)).unwrap();
        assert!(matches!(
            spool.admit(&intent(&event())),
            Err(SpoolError::Capacity)
        ));
        spool.append(&first).unwrap();
        spool.checkpoint(spool.end_offset()).unwrap();
        assert!(spool.compact_if_drained().unwrap());
        spool.admit(&intent(&event())).unwrap();
        drop(spool);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn free_space_floor_denies_without_creating_an_intent() {
        let directory = directory();
        let spool = RequestLogSpool::open_with_limits(&directory, 1, u64::MAX, u64::MAX).unwrap();
        assert!(matches!(
            spool.admit(&intent(&event())),
            Err(SpoolError::Capacity)
        ));
        assert_eq!(
            fs::read_dir(directory.join("admissions")).unwrap().count(),
            0
        );
        drop(spool);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn torn_or_corrupt_terminal_slots_remain_unknown_instead_of_zero_cost() {
        for truncate in [false, true] {
            let directory = directory();
            let event = event();
            {
                let spool = RequestLogSpool::open(&directory, u64::MAX).unwrap();
                spool.admit(&intent(&event)).unwrap();
                spool
                    .writer
                    .lock()
                    .unwrap()
                    .admissions
                    .save_terminal(&event)
                    .unwrap();
            }
            let slot = directory
                .join("admissions")
                .join(format!("{}.slot", event.id));
            let mut file = OpenOptions::new().write(true).open(&slot).unwrap();
            if truncate {
                file.set_len(20).unwrap();
            } else {
                file.seek(SeekFrom::Start(16)).unwrap();
                file.write_all(b"!").unwrap();
            }
            file.sync_all().unwrap();
            drop(file);
            let spool = RequestLogSpool::open(&directory, u64::MAX).unwrap();
            assert_eq!(spool.end_offset(), 0);
            assert!(slot.exists());
            assert!(!spool.writer.lock().unwrap().admissions.contains(event.id));
            spool.admit(&intent(&self::event())).unwrap();
            drop(spool);
            fs::remove_dir_all(directory).unwrap();
        }
    }

    #[tokio::test]
    async fn checkpoint_replays_only_uncommitted_frames_after_reopen() {
        let directory = directory();
        let first = event();
        let second = event();
        {
            let spool = Arc::new(RequestLogSpool::open(&directory, u64::MAX).unwrap());
            spool.append(&first).unwrap();
            spool.append(&second).unwrap();
            spool.sync_data().unwrap();
            let mut reader = spool.reader().await.unwrap();
            let batch = reader.read_batch(1).await.unwrap();
            assert_eq!(batch.records[0].request_log_id, first.id);
            spool.checkpoint(batch.end_offset).unwrap();
        }
        {
            let spool = Arc::new(RequestLogSpool::open(&directory, u64::MAX).unwrap());
            let mut reader = spool.reader().await.unwrap();
            let batch = reader.read_batch(10).await.unwrap();
            assert_eq!(batch.records.len(), 1);
            assert_eq!(batch.records[0].request_log_id, second.id);
            assert_eq!(batch.records[0].decode().unwrap().id, second.id);
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn drained_spool_compacts_without_losing_later_appends() {
        let directory = directory();
        let first = event();
        let second = event();
        let spool = Arc::new(RequestLogSpool::open(&directory, 1).unwrap());
        spool.append(&first).unwrap();
        let mut reader = spool.reader().await.unwrap();
        let batch = reader.read_batch(10).await.unwrap();
        spool.checkpoint(batch.end_offset).unwrap();
        assert!(spool.compact_if_drained().unwrap());
        reader.reset(0).await.unwrap();

        spool.append(&second).unwrap();
        let batch = reader.read_batch(10).await.unwrap();
        assert_eq!(batch.records.len(), 1);
        assert_eq!(batch.records[0].request_log_id, second.id);
        drop(reader);
        drop(spool);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn spool_directory_has_single_process_ownership() {
        let directory = directory();
        let spool = RequestLogSpool::open(&directory, u64::MAX).unwrap();
        assert!(RequestLogSpool::open(&directory, u64::MAX).is_err());
        drop(spool);
        assert!(RequestLogSpool::open(&directory, u64::MAX).is_ok());
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn startup_truncates_only_a_torn_tail_frame() {
        let directory = directory();
        let first = event();
        {
            let spool = RequestLogSpool::open(&directory, u64::MAX).unwrap();
            spool.append(&first).unwrap();
            spool.sync_data().unwrap();
        }
        OpenOptions::new()
            .append(true)
            .open(directory.join(super::EVENTS_FILE))
            .unwrap()
            .write_all(b"AIG")
            .unwrap();

        let spool = Arc::new(RequestLogSpool::open(&directory, u64::MAX).unwrap());
        let mut reader = spool.reader().await.unwrap();
        let batch = reader.read_batch(10).await.unwrap();
        assert_eq!(batch.records.len(), 1);
        assert_eq!(batch.records[0].request_log_id, first.id);
        drop(reader);
        drop(spool);
        fs::remove_dir_all(directory).unwrap();
    }
}
