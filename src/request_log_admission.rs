//! Synchronously durable dispatch intents and preallocated terminal recovery slots.

use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use fs2::FileExt;
use uuid::Uuid;

use crate::{
    application::RequestLogIntent,
    domain::RequestLogEvent,
    request_log_journal::EncodedRequestLog,
    request_log_spool::{MAX_PAYLOAD_BYTES, SpoolError, secure_directory, secure_file},
};

const HEADER_BYTES: usize = 16;
const MAX_INTENT_BYTES: usize = 16_384;
pub(crate) const RESERVATION_BYTES: u64 =
    (HEADER_BYTES + MAX_PAYLOAD_BYTES + MAX_INTENT_BYTES) as u64;

pub(crate) struct AdmissionStore {
    directory: PathBuf,
    directory_file: File,
    ids: BTreeSet<Uuid>,
    retained_bytes: u64,
}

impl AdmissionStore {
    pub(crate) fn open(parent: &Path) -> Result<(Self, Vec<RequestLogEvent>), SpoolError> {
        let directory = parent.join("admissions");
        fs::create_dir_all(&directory)?;
        secure_directory(&directory)?;
        File::open(parent)?.sync_all()?;
        let mut store = Self {
            directory_file: File::open(&directory)?,
            directory,
            ids: BTreeSet::new(),
            retained_bytes: 0,
        };
        for entry in fs::read_dir(&store.directory)? {
            let entry = entry?;
            let path = entry.path();
            let id = path
                .file_stem()
                .and_then(|name| name.to_str())
                .and_then(|name| Uuid::parse_str(name).ok())
                .filter(|id| {
                    matches!(
                        path.extension().and_then(|v| v.to_str()),
                        Some("json" | "slot")
                    ) && path.file_stem().unwrap() == id.to_string().as_str()
                })
                .ok_or(SpoolError::Corrupt("unexpected request-log admission file"))?;
            if !entry.file_type()?.is_file() {
                return Err(SpoolError::Corrupt("admission entry is not a regular file"));
            }
            store.ids.insert(id);
        }
        let mut recovered = Vec::new();
        for id in store.ids.clone() {
            if let Some(event) = store.read_terminal(id)? {
                recovered.push(event);
            } else {
                store.retain_unknown(id)?;
                store.ids.remove(&id);
                // An intent is not a terminal outcome: never project it as a
                // failed/cancelled (zero-cost) request after an abrupt restart.
                tracing::error!(
                    request_log_id = %id,
                    reason = "request_log_reconciliation_required",
                    "request may have dispatched without a durable terminal event; retained for reconciliation"
                );
            }
        }
        Ok((store, recovered))
    }

    pub(crate) fn reserved_bytes(&self) -> u64 {
        (self.ids.len() as u64)
            .saturating_mul(2 * RESERVATION_BYTES)
            .saturating_add(self.retained_bytes)
    }

    pub(crate) fn terminal_headroom(&self) -> u64 {
        (self.ids.len() as u64).saturating_mul(RESERVATION_BYTES)
    }

    fn retain_unknown(&mut self, id: Uuid) -> Result<(), SpoolError> {
        // A dead process cannot finish these reservations. Keep all nonzero
        // evidence, but reclaim unused allocation rather than freezing future
        // dispatch solely because old requests have unknown usage.
        match OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.path(id, "slot"))
        {
            Ok(mut file) => {
                if file.metadata()?.len() > (HEADER_BYTES + MAX_PAYLOAD_BYTES) as u64 {
                    return Err(SpoolError::Corrupt("admission slot exceeds size limit"));
                }
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)?;
                let len = bytes
                    .iter()
                    .rposition(|byte| *byte != 0)
                    .map_or(0, |i| i + 1);
                file.set_len(len as u64)?;
                file.sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        for extension in ["json", "slot"] {
            match fs::metadata(self.path(id, extension)) {
                Ok(metadata) => {
                    self.retained_bytes =
                        self.retained_bytes.saturating_add(metadata.len().max(4096))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    pub(crate) fn contains(&self, id: Uuid) -> bool {
        self.ids.contains(&id)
    }

    pub(crate) fn reserve(&mut self, intent: &RequestLogIntent) -> Result<(), SpoolError> {
        let bytes = serde_json::to_vec(intent)
            .map_err(|_| SpoolError::Corrupt("cannot encode request-log intent"))?;
        if bytes.len() > MAX_INTENT_BYTES {
            return Err(SpoolError::PayloadTooLarge { bytes: bytes.len() });
        }
        if !self.ids.insert(intent.id) {
            return Err(SpoolError::Corrupt("duplicate request-log admission UUID"));
        }
        let slot = self.create(intent.id, "slot")?;
        // Allocation, not set_len/statvfs alone, reserves blocks for the
        // largest accepted terminal payload. Unsupported filesystems fail closed.
        slot.allocate((HEADER_BYTES + MAX_PAYLOAD_BYTES) as u64)?;
        slot.sync_all()?;
        let mut file = self.create(intent.id, "json")?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        self.directory_file.sync_all()?;
        Ok(())
    }

    pub(crate) fn save_terminal(&self, event: &RequestLogEvent) -> Result<(), SpoolError> {
        let record = EncodedRequestLog::encode(event)?;
        if record.payload.len() > MAX_PAYLOAD_BYTES {
            return Err(SpoolError::PayloadTooLarge {
                bytes: record.payload.len(),
            });
        }
        let mut file = OpenOptions::new()
            .write(true)
            .open(self.path(event.id, "slot"))?;
        file.seek(SeekFrom::Start(HEADER_BYTES as u64))?;
        file.write_all(&record.payload)?;
        let mut header = Vec::with_capacity(HEADER_BYTES);
        header.extend_from_slice(b"AIGA");
        header.extend_from_slice(&record.schema_version.to_le_bytes());
        header.extend_from_slice(&0_u16.to_le_bytes());
        header.extend_from_slice(&(record.payload.len() as u32).to_le_bytes());
        header.extend_from_slice(&crc32fast::hash(&record.payload).to_le_bytes());
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header)?;
        file.sync_all()?;
        Ok(())
    }

    fn read_terminal(&self, id: Uuid) -> Result<Option<RequestLogEvent>, SpoolError> {
        let mut file = match File::open(self.path(id, "slot")) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut header = [0_u8; HEADER_BYTES];
        if file.read_exact(&mut header).is_err() || &header[..4] != b"AIGA" {
            return Ok(None);
        }
        let len = u32::from_le_bytes(header[8..12].try_into().unwrap()) as usize;
        if len > MAX_PAYLOAD_BYTES || header[6..8] != [0, 0] {
            return Ok(None);
        }
        let mut payload = vec![0; len];
        if file.read_exact(&mut payload).is_err()
            || crc32fast::hash(&payload) != u32::from_le_bytes(header[12..16].try_into().unwrap())
        {
            return Ok(None);
        }
        let event = EncodedRequestLog {
            request_log_id: id,
            schema_version: i16::from_le_bytes(header[4..6].try_into().unwrap()),
            payload,
        }
        .decode()?;
        if event.id != id {
            return Err(SpoolError::Corrupt("admission terminal UUID mismatch"));
        }
        Ok(Some(event))
    }

    pub(crate) fn retire(&mut self, id: Uuid) -> Result<(), SpoolError> {
        // Retain the terminal slot until removing the intent is itself durable.
        // A crash between these steps replays the slot with the same UUID.
        for extension in ["json", "slot"] {
            match fs::remove_file(self.path(id, extension)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            self.directory_file.sync_all()?;
        }
        self.ids.remove(&id);
        Ok(())
    }

    fn path(&self, id: Uuid, extension: &str) -> PathBuf {
        self.directory.join(format!("{id}.{extension}"))
    }

    fn create(&self, id: Uuid, extension: &str) -> Result<File, SpoolError> {
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(self.path(id, extension))?;
        secure_file(&file)?;
        Ok(file)
    }
}
