//! Single-writer, durable monetary admission. No database access on the request path.

use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::domain::codex_sharing::{SharingGroup, SharingWindow};

const MAX_LEDGER_BYTES: u64 = 64 * 1024 * 1024;
const CHECKPOINT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PENDING: usize = 1000;
const MAX_OBSERVATION_AGE_SECONDS: i64 = 180;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, thiserror::Error)]
pub enum SharingError {
    #[error(
        "Standalone search does not provide monetary usage and is not available to sharing members."
    )]
    UnsupportedOperation,
    #[error("Sharing is paused or this user has no assigned seat.")]
    Membership,
    #[error("Sharing window information is unavailable or awaiting refresh.")]
    WindowUnavailable,
    #[error("The sharing window allowance is exhausted or fully reserved.")]
    QuotaExceeded,
    #[error("The sharing request rate limit was reached.")]
    RateLimited,
    #[error("The sharing concurrency limit was reached.")]
    ConcurrentLimited,
    #[error("An earlier sharing request is awaiting usage reconciliation.")]
    Uncertain,
    #[error("The single-writer sharing ledger is unavailable.")]
    Unavailable,
}

impl SharingError {
    pub fn code(self) -> &'static str {
        match self {
            Self::UnsupportedOperation => "sharing_operation_unmetered",
            Self::Membership => "sharing_membership_required",
            Self::WindowUnavailable => "sharing_window_unavailable",
            Self::QuotaExceeded => "sharing_quota_exceeded",
            Self::RateLimited => "sharing_rate_limited",
            Self::ConcurrentLimited => "sharing_concurrent_limited",
            Self::Uncertain => "sharing_usage_pending",
            Self::Unavailable => "sharing_unavailable",
        }
    }
}

#[derive(Clone, Default)]
pub struct SharingRuntime {
    inner: Option<Arc<RuntimeInner>>,
}

struct RuntimeInner {
    commands: mpsc::Sender<Command>,
    settlements: mpsc::Sender<(Uuid, Option<Decimal>)>,
    healthy: Arc<AtomicBool>,
    ledger_id: Uuid,
}

pub struct SharingLease {
    runtime: SharingRuntime,
    id: Option<Uuid>,
}

impl SharingLease {
    pub fn settle(mut self, cost: Option<Decimal>) {
        if let Some(id) = self.id.take() {
            self.runtime.finish(id, cost);
        }
    }
}

impl Drop for SharingLease {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            self.runtime.finish(id, None);
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SharingWindowBalance {
    pub window_id: Uuid,
    pub window_kind: String,
    pub reset_at: DateTime<Utc>,
    pub limit_amount: Decimal,
    pub used_amount: Decimal,
    pub reserved_amount: Decimal,
    pub remaining_amount: Decimal,
    pub group_remaining_amount: Decimal,
    pub provider_used_percent: i32,
    pub checked_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct SharingUsage {
    pub available: bool,
    pub seat_number: Option<usize>,
    pub pending_requests: usize,
    pub uncertain: bool,
    pub windows: Vec<SharingWindowBalance>,
}

enum Command {
    Flush(oneshot::Sender<()>),
    Sync {
        groups: Vec<SharingGroup>,
        windows: Vec<SharingWindow>,
        reply: oneshot::Sender<Result<(), SharingError>>,
    },
    Reserve {
        group: Box<SharingGroup>,
        user: Uuid,
        request: Uuid,
        reply: oneshot::Sender<Result<(), SharingError>>,
    },
    Wake,
    Inspect {
        group: Box<SharingGroup>,
        user: Uuid,
        reply: oneshot::Sender<SharingUsage>,
    },
    Pending(oneshot::Sender<Vec<Uuid>>),
}

impl SharingRuntime {
    pub async fn open(directory: PathBuf) -> io::Result<Self> {
        let store = tokio::task::spawn_blocking(move || LedgerStore::open(&directory))
            .await
            .map_err(io::Error::other)??;
        let ledger_id = store.state.ledger_id;
        let (commands, receiver) = mpsc::channel(1024);
        let (settlements, settlement_receiver) = mpsc::channel(MAX_PENDING * 2);
        let healthy = Arc::new(AtomicBool::new(true));
        let actor_health = Arc::clone(&healthy);
        thread::Builder::new()
            .name("codex-sharing-ledger".into())
            .spawn(move || Actor::new(store, actor_health).run(receiver, settlement_receiver))?;
        Ok(Self {
            inner: Some(Arc::new(RuntimeInner {
                commands,
                settlements,
                healthy,
                ledger_id,
            })),
        })
    }

    pub fn ledger_id(&self) -> Option<Uuid> {
        self.inner.as_ref().map(|inner| inner.ledger_id)
    }

    pub fn poison(&self) {
        if let Some(inner) = &self.inner {
            inner.healthy.store(false, Ordering::SeqCst);
        }
    }

    pub fn available(&self) -> bool {
        self.inner.as_ref().is_some_and(|inner| {
            inner.healthy.load(Ordering::SeqCst) && !inner.commands.is_closed()
        })
    }

    async fn send(&self, command: Command) -> Result<(), SharingError> {
        let inner = self.inner.as_ref().ok_or(SharingError::Unavailable)?;
        if !inner.healthy.load(Ordering::SeqCst) {
            return Err(SharingError::Unavailable);
        }
        match tokio::time::timeout(COMMAND_TIMEOUT, inner.commands.send(command)).await {
            Ok(Ok(())) => Ok(()),
            _ => {
                self.poison();
                Err(SharingError::Unavailable)
            }
        }
    }

    async fn receive<T>(&self, receiver: oneshot::Receiver<T>) -> Result<T, SharingError> {
        match tokio::time::timeout(COMMAND_TIMEOUT, receiver).await {
            Ok(Ok(value)) => Ok(value),
            _ => {
                self.poison();
                Err(SharingError::Unavailable)
            }
        }
    }

    pub async fn sync(
        &self,
        groups: Vec<SharingGroup>,
        windows: Vec<SharingWindow>,
    ) -> Result<(), SharingError> {
        let (reply, receiver) = oneshot::channel();
        self.send(Command::Sync {
            groups,
            windows,
            reply,
        })
        .await?;
        self.receive(receiver).await?
    }

    pub fn publish(&self, registry: &crate::domain::codex_sharing::SharingRegistry) {
        let Some(inner) = &self.inner else { return };
        let (reply, _) = oneshot::channel();
        if inner
            .commands
            .try_send(Command::Sync {
                groups: registry.groups().cloned().collect(),
                windows: registry.windows().to_vec(),
                reply,
            })
            .is_err()
        {
            self.poison();
        }
    }

    pub async fn reserve(
        &self,
        group: &SharingGroup,
        user: Uuid,
        request: Uuid,
    ) -> Result<SharingLease, SharingError> {
        let (reply, receiver) = oneshot::channel();
        self.send(Command::Reserve {
            group: Box::new(group.clone()),
            user,
            request,
            reply,
        })
        .await?;
        // If this future is cancelled, the actor retains an uncertain reservation.
        // Cancellation cannot turn an upstream-dispatch intent into free usage.
        self.receive(receiver).await??;
        Ok(SharingLease {
            runtime: self.clone(),
            id: Some(request),
        })
    }

    pub async fn inspect(&self, group: &SharingGroup, user: Uuid) -> SharingUsage {
        let unavailable = SharingUsage {
            seat_number: group
                .policy
                .seats
                .iter()
                .position(|id| *id == Some(user))
                .map(|index| index + 1),
            ..Default::default()
        };
        let (reply, receiver) = oneshot::channel();
        if self
            .send(Command::Inspect {
                group: Box::new(group.clone()),
                user,
                reply,
            })
            .await
            .is_err()
        {
            return unavailable;
        }
        self.receive(receiver).await.unwrap_or(unavailable)
    }

    pub async fn pending(&self) -> Vec<Uuid> {
        let (reply, receiver) = oneshot::channel();
        if self.send(Command::Pending(reply)).await.is_err() {
            return Vec::new();
        }
        self.receive(receiver).await.unwrap_or_default()
    }

    pub async fn flush(&self) -> Result<(), SharingError> {
        if self.inner.is_none() {
            return Ok(());
        }
        let (reply, receiver) = oneshot::channel();
        self.send(Command::Flush(reply)).await?;
        self.receive(receiver).await?;
        if self.available() {
            Ok(())
        } else {
            Err(SharingError::Unavailable)
        }
    }

    pub fn finish(&self, id: Uuid, cost: Option<Decimal>) {
        let Some(inner) = &self.inner else { return };
        if inner.settlements.try_send((id, cost)).is_err() {
            self.poison();
        } else {
            // A full read/admission queue must not discard a completed charge.
            // An existing queued command also wakes the priority drain.
            let _ = inner.commands.try_send(Command::Wake);
        }
    }
}

#[derive(Serialize, Deserialize)]
struct LedgerState {
    version: u32,
    sequence: u64,
    ledger_id: Uuid,
    periods: BTreeMap<Uuid, PeriodLedger>,
    pending: BTreeMap<Uuid, Reservation>,
    epochs: BTreeMap<Uuid, Uuid>,
}

#[derive(Serialize, Deserialize)]
struct LedgerEnvelope {
    payload: String,
    checksum: [u8; 32],
}

#[derive(Clone, Serialize, Deserialize)]
struct PeriodLedger {
    group_id: Uuid,
    seat_limit: Decimal,
    seats: Vec<Decimal>,
    users: BTreeMap<Uuid, Decimal>,
    reset_at: DateTime<Utc>,
    anchored: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Reservation {
    group_id: Uuid,
    user_id: Uuid,
    seat: usize,
    amount: Decimal,
    periods: Vec<Uuid>,
    uncertain: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct LedgerChange {
    sequence: u64,
    periods: BTreeMap<Uuid, PeriodLedger>,
    pending: BTreeMap<Uuid, Reservation>,
    epochs: BTreeMap<Uuid, Uuid>,
    removed_pending: Vec<Uuid>,
    removed_periods: Vec<Uuid>,
    removed_epochs: Vec<Uuid>,
}

impl LedgerChange {
    fn replay(self, state: &mut LedgerState) {
        state.sequence = self.sequence;
        state.periods.extend(self.periods);
        state.pending.extend(self.pending);
        state.epochs.extend(self.epochs);
        for id in self.removed_pending {
            state.pending.remove(&id);
        }
        for id in self.removed_periods {
            state.periods.remove(&id);
        }
        for id in self.removed_epochs {
            state.epochs.remove(&id);
        }
    }

    fn is_empty(&self) -> bool {
        self.periods.is_empty()
            && self.pending.is_empty()
            && self.epochs.is_empty()
            && self.removed_pending.is_empty()
            && self.removed_periods.is_empty()
            && self.removed_epochs.is_empty()
    }
}

fn encode_frame(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let payload = serde_json::to_string(value).map_err(io::Error::other)?;
    let checksum = Sha256::digest(payload.as_bytes()).into();
    let mut bytes =
        serde_json::to_vec(&LedgerEnvelope { payload, checksum }).map_err(io::Error::other)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_LEDGER_BYTES {
        return Err(io::Error::other("sharing ledger exceeds its size limit"));
    }
    Ok(bytes)
}

fn decode_frame<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    let envelope: LedgerEnvelope = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    let checksum: [u8; 32] = Sha256::digest(envelope.payload.as_bytes()).into();
    if checksum != envelope.checksum {
        return Err(io::Error::other("sharing ledger checksum mismatch"));
    }
    serde_json::from_str(&envelope.payload).map_err(io::Error::other)
}

struct LedgerStore {
    state: LedgerState,
    directory: PathBuf,
    _lock: File,
    wal: File,
    wal_bytes: u64,
}

impl LedgerStore {
    fn open(directory: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("writer.lock"))?;
        lock.try_lock().map_err(io::Error::other)?;
        let path = directory.join("ledger.json");
        let has_checkpoint = path.try_exists()?;
        let mut state = match File::open(&path) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(MAX_LEDGER_BYTES + 1).read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_LEDGER_BYTES {
                    return Err(io::Error::other("sharing ledger exceeds its size limit"));
                }
                let state: LedgerState = decode_frame(&bytes)?;
                if state.version != 1 {
                    return Err(io::Error::other("unsupported sharing ledger version"));
                }
                state
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => LedgerState {
                version: 1,
                sequence: 0,
                ledger_id: Uuid::new_v4(),
                periods: BTreeMap::new(),
                pending: BTreeMap::new(),
                epochs: BTreeMap::new(),
            },
            Err(error) => return Err(error),
        };
        let journal_path = directory.join("ledger.wal");
        if has_checkpoint && !journal_path.try_exists()? {
            return Err(io::Error::other("sharing journal is missing"));
        }
        let mut options = OpenOptions::new();
        options.create(true).read(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let wal = options.open(journal_path)?;
        let mut journal = Vec::new();
        (&wal)
            .take(MAX_LEDGER_BYTES + 1)
            .read_to_end(&mut journal)?;
        if journal.len() as u64 > MAX_LEDGER_BYTES {
            return Err(io::Error::other("sharing journal exceeds its size limit"));
        }
        if !has_checkpoint && !journal.is_empty() {
            return Err(io::Error::other("sharing checkpoint is missing"));
        }
        let checkpoint_sequence = state.sequence;
        for bytes in journal.split_inclusive(|byte| *byte == b'\n') {
            // A torn final frame was never acknowledged as durable. Keeping the
            // earlier reservation is conservative if the lost frame was settlement.
            if !bytes.ends_with(b"\n")
                && serde_json::from_slice::<LedgerEnvelope>(bytes)
                    .is_err_and(|error| error.is_eof())
            {
                break;
            }
            let change: LedgerChange = decode_frame(bytes)?;
            if change.sequence <= checkpoint_sequence {
                continue;
            }
            if state.sequence.checked_add(1) != Some(change.sequence) {
                return Err(io::Error::other("sharing journal sequence gap"));
            }
            change.replay(&mut state);
        }
        let mut store = Self {
            state,
            directory: directory.into(),
            _lock: lock,
            wal,
            wal_bytes: journal.len() as u64,
        };
        for reservation in store.state.pending.values_mut() {
            reservation.uncertain = true;
        }
        store.checkpoint()?;
        Ok(store)
    }

    fn checkpoint(&mut self) -> io::Result<()> {
        let bytes = encode_frame(&self.state)?;
        let temporary = self.directory.join("ledger.next");
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(temporary, self.directory.join("ledger.json"))?;
        File::open(&self.directory)?.sync_all()?;
        // The durable checkpoint must precede journal truncation; replay skips
        // already-checkpointed sequences if a crash happens between these steps.
        self.wal.set_len(0)?;
        self.wal.sync_all()?;
        self.wal_bytes = 0;
        Ok(())
    }

    fn commit(&mut self, mut change: LedgerChange) -> io::Result<()> {
        change.sequence = self
            .state
            .sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("sharing journal sequence exhausted"))?;
        let bytes = encode_frame(&change)?;
        if self.wal_bytes + bytes.len() as u64 > MAX_LEDGER_BYTES {
            return Err(io::Error::other("sharing journal exceeds its size limit"));
        }
        self.wal.write_all(&bytes)?;
        self.wal.sync_all()?;
        self.state.sequence = change.sequence;
        self.wal_bytes += bytes.len() as u64;
        if self.wal_bytes >= CHECKPOINT_BYTES {
            self.checkpoint()?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct Rate {
    minute: i64,
    requests: u32,
}

struct Actor {
    store: LedgerStore,
    windows: Vec<SharingWindow>,
    healthy: Arc<AtomicBool>,
    rates: BTreeMap<(Uuid, Option<Uuid>), Rate>,
}

impl Actor {
    fn new(store: LedgerStore, healthy: Arc<AtomicBool>) -> Self {
        Self {
            store,
            windows: Vec::new(),
            healthy,
            rates: BTreeMap::new(),
        }
    }

    fn persist(&mut self, change: LedgerChange) -> Result<(), SharingError> {
        self.store.commit(change).map_err(|error| {
            self.healthy.store(false, Ordering::SeqCst);
            tracing::error!(kind = ?error.kind(), "Codex sharing ledger persistence failed; admission disabled");
            SharingError::Unavailable
        })
    }

    fn run(
        mut self,
        mut receiver: mpsc::Receiver<Command>,
        mut settlements: mpsc::Receiver<(Uuid, Option<Decimal>)>,
    ) {
        while let Some(command) = receiver.blocking_recv() {
            while let Ok((id, cost)) = settlements.try_recv() {
                self.finish(id, cost);
            }
            match command {
                Command::Flush(reply) => {
                    let _ = reply.send(());
                }
                Command::Sync {
                    groups,
                    windows,
                    reply,
                } => {
                    let result = self.sync(&groups, windows);
                    let _ = reply.send(result);
                }
                Command::Reserve {
                    group,
                    user,
                    request,
                    reply,
                } => {
                    let result = self.reserve(&group, user, request, Utc::now());
                    if reply.send(result).is_err() {
                        self.finish(request, None);
                    }
                }
                Command::Wake => {}
                Command::Inspect { group, user, reply } => {
                    let _ = reply.send(self.inspect(&group, user, Utc::now()));
                }
                Command::Pending(reply) => {
                    let _ = reply.send(
                        self.store
                            .state
                            .pending
                            .iter()
                            .filter(|(_, reservation)| {
                                reservation.uncertain
                                    && reservation.periods.iter().any(|id| {
                                        self.windows.iter().any(|window| window.id == *id)
                                    })
                            })
                            .take(MAX_PENDING)
                            .map(|(id, _)| *id)
                            .collect(),
                    );
                }
            }
        }
        while let Ok((id, cost)) = settlements.try_recv() {
            self.finish(id, cost);
        }
    }

    fn sync(
        &mut self,
        groups: &[SharingGroup],
        mut windows: Vec<SharingWindow>,
    ) -> Result<(), SharingError> {
        if !self.healthy.load(Ordering::SeqCst) {
            return Err(SharingError::Unavailable);
        }
        if groups.iter().any(|group| !group.policy.valid()) {
            return Err(SharingError::Membership);
        }
        windows.sort_by(|a, b| {
            (a.credential_id, &a.window_kind).cmp(&(b.credential_id, &b.window_kind))
        });
        let mut change = LedgerChange::default();
        for window in &mut windows {
            let Some(group) = groups
                .iter()
                .find(|g| g.policy.credential_id == window.credential_id)
            else {
                continue;
            };
            let amount = if window.window_kind == "primary" {
                group.policy.primary_limit_amount
            } else {
                group.policy.secondary_limit_amount
            };
            let next_limit = (amount / Decimal::from(group.policy.seats.len()))
                .round_dp_with_strategy(8, RoundingStrategy::ToZero);
            let pending_policy = |period: &PeriodLedger| {
                period.seats.len() != group.policy.seats.len() || period.seat_limit != next_limit
            };
            let source_id = window.id;
            let current_id = self
                .store
                .state
                .epochs
                .get(&source_id)
                .copied()
                .unwrap_or(source_id);
            let fresh = (Utc::now() - window.checked_at).num_seconds();
            // A rounded 0% window can slide forever without changing the provider
            // history ID. Freeze on first admission or a pending allocation change;
            // otherwise an entirely vacant car could never activate new seats.
            // Advance only after the deadline AND a fresh post-boundary 0% observation.
            let advance = self
                .store
                .state
                .periods
                .get(&current_id)
                .is_some_and(|period| {
                    (period.anchored || pending_policy(period))
                        && window.used_percent == 0
                        && window.checked_at >= period.reset_at
                        && window.scheduled_reset_at > window.checked_at
                        && (-30..=MAX_OBSERVATION_AGE_SECONDS).contains(&fresh)
                });
            window.id = if advance { Uuid::new_v4() } else { current_id };
            if self.store.state.epochs.insert(source_id, window.id) != Some(window.id) {
                change.epochs.insert(source_id, window.id);
            }
            let mut changed = false;
            if let std::collections::btree_map::Entry::Vacant(entry) =
                self.store.state.periods.entry(window.id)
            {
                entry.insert(PeriodLedger {
                    group_id: group.id,
                    seat_limit: next_limit,
                    seats: vec![Decimal::ZERO; group.policy.seats.len()],
                    users: BTreeMap::new(),
                    reset_at: window.scheduled_reset_at,
                    anchored: window.used_percent > 0,
                });
                changed = true;
            }
            if let Some(period) = self.store.state.periods.get_mut(&window.id) {
                if !period.anchored && pending_policy(period) {
                    period.anchored = true;
                    changed = true;
                }
                if (!period.anchored || window.used_percent > 0)
                    && period.reset_at != window.scheduled_reset_at
                {
                    period.reset_at = window.scheduled_reset_at;
                    changed = true;
                }
                window.scheduled_reset_at = period.reset_at;
                if changed {
                    change.periods.insert(window.id, period.clone());
                }
            }
        }
        self.expire_history(&windows, &mut change);
        if !change.is_empty() {
            self.persist(change)?;
        }
        self.windows = windows;
        Ok(())
    }

    fn expire_history(&mut self, windows: &[SharingWindow], change: &mut LedgerChange) {
        let cutoff = Utc::now() - chrono::Duration::days(90);
        let current = windows
            .iter()
            .map(|window| window.id)
            .collect::<std::collections::HashSet<_>>();
        let expired = self
            .store
            .state
            .pending
            .iter()
            .filter(|(_, reservation)| {
                reservation.uncertain
                    && reservation.periods.iter().all(|id| {
                        !current.contains(id)
                            && self
                                .store
                                .state
                                .periods
                                .get(id)
                                .is_some_and(|period| period.reset_at < cutoff)
                    })
            })
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in expired {
            self.store.state.pending.remove(&id);
            change.removed_pending.push(id);
        }
        let referenced = current
            .into_iter()
            .chain(
                self.store
                    .state
                    .pending
                    .values()
                    .flat_map(|reservation| reservation.periods.iter().copied()),
            )
            .collect::<std::collections::HashSet<_>>();
        self.store.state.periods.retain(|id, period| {
            let keep = period.reset_at >= cutoff || referenced.contains(id);
            if !keep {
                change.removed_periods.push(*id);
            }
            keep
        });
        self.store.state.epochs.retain(|id, target| {
            let keep = self.store.state.periods.contains_key(target);
            if !keep {
                change.removed_epochs.push(*id);
            }
            keep
        });
    }

    fn windows_ready(&self, group: &SharingGroup, now: DateTime<Utc>) -> bool {
        let windows = self
            .windows
            .iter()
            .filter(|w| w.credential_id == group.policy.credential_id)
            .collect::<Vec<_>>();
        !windows.is_empty()
            && windows.iter().all(|window| {
                window.scheduled_reset_at > now
                    && window.checked_at <= now + chrono::Duration::seconds(30)
                    && now.signed_duration_since(window.checked_at).num_seconds()
                        <= MAX_OBSERVATION_AGE_SECONDS
            })
    }

    fn inspect(&self, group: &SharingGroup, user: Uuid, now: DateTime<Utc>) -> SharingUsage {
        let seat = group.policy.seats.iter().position(|id| *id == Some(user));
        let mut result = SharingUsage {
            available: self.healthy.load(Ordering::SeqCst)
                && group.policy.enabled
                && seat.is_some()
                && self.windows_ready(group, now),
            seat_number: seat.map(|seat| seat + 1),
            ..Default::default()
        };
        let Some(seat) = seat else { return result };
        for window in self
            .windows
            .iter()
            .filter(|w| w.credential_id == group.policy.credential_id)
        {
            let Some(period) = self.store.state.periods.get(&window.id) else {
                result.available = false;
                continue;
            };
            let Some(seat_used) = period.seats.get(seat) else {
                result.available = false;
                continue;
            };
            let user_used = period.users.get(&user).copied().unwrap_or_default();
            let mut seat_reserved = Decimal::ZERO;
            let mut user_reserved = Decimal::ZERO;
            let mut group_reserved = Decimal::ZERO;
            for reservation in self
                .store
                .state
                .pending
                .values()
                .filter(|r| r.periods.contains(&window.id))
            {
                group_reserved += reservation.amount;
                if reservation.seat == seat {
                    seat_reserved += reservation.amount;
                    result.uncertain |= reservation.uncertain;
                }
                if reservation.user_id == user {
                    user_reserved += reservation.amount;
                    result.uncertain |= reservation.uncertain;
                }
            }
            let used = (*seat_used).max(user_used);
            let effective_reserved_total = seat_used
                .checked_add(seat_reserved)
                .unwrap_or(Decimal::MAX)
                .max(user_used.checked_add(user_reserved).unwrap_or(Decimal::MAX));
            let remaining = period
                .seat_limit
                .checked_sub(effective_reserved_total)
                .unwrap_or(Decimal::MIN)
                .max(Decimal::ZERO);
            let group_used = period.seats.iter().fold(Decimal::ZERO, |sum, cost| {
                sum.checked_add(*cost).unwrap_or(Decimal::MAX)
            });
            let group_remaining = (period.seat_limit * Decimal::from(period.seats.len()))
                .checked_sub(group_used)
                .and_then(|value| value.checked_sub(group_reserved))
                .unwrap_or(Decimal::MIN)
                .max(Decimal::ZERO);
            result.windows.push(SharingWindowBalance {
                window_id: window.id,
                window_kind: window.window_kind.clone(),
                reset_at: window.scheduled_reset_at,
                limit_amount: period.seat_limit,
                used_amount: used,
                reserved_amount: effective_reserved_total - used,
                remaining_amount: remaining.max(Decimal::ZERO),
                group_remaining_amount: group_remaining,
                provider_used_percent: window.used_percent,
                checked_at: window.checked_at,
            });
        }
        result.pending_requests = self
            .store
            .state
            .pending
            .values()
            .filter(|r| {
                (r.user_id == user || r.seat == seat)
                    && r.group_id == group.id
                    && r.periods
                        .iter()
                        .any(|id| result.windows.iter().any(|w| w.window_id == *id))
            })
            .count();
        result
    }

    fn reserve(
        &mut self,
        group: &SharingGroup,
        user: Uuid,
        request: Uuid,
        now: DateTime<Utc>,
    ) -> Result<(), SharingError> {
        if !self.healthy.load(Ordering::SeqCst) {
            return Err(SharingError::Unavailable);
        }
        let seat = group
            .policy
            .seats
            .iter()
            .position(|id| *id == Some(user))
            .filter(|_| group.policy.enabled)
            .ok_or(SharingError::Membership)?;
        let usage = self.inspect(group, user, now);
        if !usage.available {
            return Err(SharingError::WindowUnavailable);
        }
        if usage.uncertain {
            return Err(SharingError::Uncertain);
        }
        if usage.windows.iter().any(|w| {
            w.remaining_amount.min(w.group_remaining_amount)
                < group.policy.request_reservation_amount
        }) {
            return Err(SharingError::QuotaExceeded);
        }
        if self
            .store
            .state
            .pending
            .values()
            .filter(|r| {
                !r.uncertain
                    || r.periods
                        .iter()
                        .any(|id| self.windows.iter().any(|w| w.id == *id))
            })
            .count()
            >= MAX_PENDING
            || self.store.state.pending.contains_key(&request)
        {
            return Err(SharingError::Unavailable);
        }
        let active = self
            .store
            .state
            .pending
            .values()
            .filter(|r| r.group_id == group.id && !r.uncertain);
        if active.clone().count() >= group.policy.group_max_concurrent_requests as usize
            || active
                .filter(|r| r.user_id == user || r.seat == seat)
                .count()
                >= group.policy.user_max_concurrent_requests as usize
        {
            return Err(SharingError::ConcurrentLimited);
        }
        let minute = now.timestamp().div_euclid(60);
        for (who, limit) in [
            (None, group.policy.group_requests_per_minute),
            (Some(user), group.policy.user_requests_per_minute),
        ] {
            if self
                .rates
                .get(&(group.id, who))
                .is_some_and(|r| r.minute >= minute && r.requests >= limit)
            {
                return Err(SharingError::RateLimited);
            }
        }
        self.store.state.pending.insert(
            request,
            Reservation {
                group_id: group.id,
                user_id: user,
                seat,
                amount: group.policy.request_reservation_amount,
                periods: usage.windows.iter().map(|w| w.window_id).collect(),
                uncertain: false,
            },
        );
        let mut change = LedgerChange::default();
        change
            .pending
            .insert(request, self.store.state.pending[&request].clone());
        for window in &usage.windows {
            if let Some(period) = self.store.state.periods.get_mut(&window.window_id)
                && !period.anchored
            {
                period.anchored = true;
                change.periods.insert(window.window_id, period.clone());
            }
        }
        self.persist(change)?;
        self.rates.retain(|_, rate| rate.minute >= minute);
        for who in [None, Some(user)] {
            let rate = self.rates.entry((group.id, who)).or_default();
            if rate.minute < minute {
                rate.minute = minute;
                rate.requests = 0;
            }
            rate.requests += 1;
        }
        Ok(())
    }

    fn finish(&mut self, id: Uuid, cost: Option<Decimal>) {
        if !self.healthy.load(Ordering::SeqCst) {
            return;
        }
        if !self.healthy.load(Ordering::SeqCst) {
            return;
        }
        let Some(mut reservation) = self.store.state.pending.remove(&id) else {
            return;
        };
        let mut change = LedgerChange::default();
        if let Some(cost) = cost.filter(|amount| *amount >= Decimal::ZERO) {
            for id in &reservation.periods {
                let Some(period) = self.store.state.periods.get_mut(id) else {
                    self.healthy.store(false, Ordering::SeqCst);
                    return;
                };
                let Some(seat) = period.seats.get_mut(reservation.seat) else {
                    self.healthy.store(false, Ordering::SeqCst);
                    return;
                };
                let user = period.users.entry(reservation.user_id).or_default();
                let (Some(next_seat), Some(next_user)) =
                    (seat.checked_add(cost), user.checked_add(cost))
                else {
                    self.healthy.store(false, Ordering::SeqCst);
                    return;
                };
                *seat = next_seat;
                *user = next_user;
                change.periods.insert(*id, period.clone());
            }
            change.removed_pending.push(id);
        } else {
            reservation.uncertain = true;
            change.pending.insert(id, reservation.clone());
            self.store.state.pending.insert(id, reservation);
        }
        let _ = self.persist(change);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::codex_sharing::SharingGroupInput;

    fn fixture() -> (tempfile::TempDir, Actor, SharingGroup, Vec<SharingWindow>) {
        let directory = tempfile::tempdir().unwrap();
        let actor = Actor::new(
            LedgerStore::open(directory.path()).unwrap(),
            Arc::new(AtomicBool::new(true)),
        );
        let group = SharingGroup {
            id: Uuid::new_v4(),
            updated_at: Utc::now(),
            policy: SharingGroupInput {
                credential_id: Uuid::new_v4(),
                name: "Shared credential".into(),
                enabled: true,
                seats: vec![Some(Uuid::new_v4()), Some(Uuid::new_v4())],
                primary_limit_amount: Decimal::from(10),
                secondary_limit_amount: Decimal::from(40),
                request_reservation_amount: Decimal::ONE,
                user_requests_per_minute: 30,
                group_requests_per_minute: 60,
                user_max_concurrent_requests: 2,
                group_max_concurrent_requests: 4,
            },
        };
        let windows = ["primary", "secondary"]
            .into_iter()
            .map(|kind| SharingWindow {
                id: Uuid::new_v4(),
                credential_id: group.policy.credential_id,
                window_kind: kind.into(),
                scheduled_reset_at: Utc::now() + chrono::Duration::hours(1),
                checked_at: Utc::now(),
                used_percent: 10,
            })
            .collect();
        (directory, actor, group, windows)
    }

    #[test]
    fn money_is_shared_across_keys_but_isolated_between_users_and_windows() {
        let (_dir, mut actor, group, windows) = fixture();
        actor
            .sync(std::slice::from_ref(&group), windows.clone())
            .unwrap();
        let user = group.policy.seats[0].unwrap();
        let request = Uuid::new_v4();
        actor.reserve(&group, user, request, Utc::now()).unwrap();
        assert_eq!(
            actor.inspect(&group, user, Utc::now()).windows[0].reserved_amount,
            Decimal::ONE
        );
        actor.finish(request, Some(Decimal::from(6)));
        actor.finish(request, Some(Decimal::from(6)));
        assert!(matches!(
            actor.reserve(&group, user, Uuid::new_v4(), Utc::now()),
            Err(SharingError::QuotaExceeded)
        ));
        assert_eq!(
            actor
                .inspect(&group, group.policy.seats[1].unwrap(), Utc::now())
                .windows[0]
                .used_amount,
            Decimal::ZERO
        );
        let mut next = windows;
        next[0].id = Uuid::new_v4();
        actor.sync(std::slice::from_ref(&group), next).unwrap();
        let usage = actor.inspect(&group, user, Utc::now());
        assert_eq!(usage.windows[0].used_amount, Decimal::ZERO);
        assert_eq!(usage.windows[1].used_amount, Decimal::from(6));
    }

    #[test]
    fn refresh_and_seat_changes_do_not_reissue_money() {
        let (_dir, mut actor, mut group, windows) = fixture();
        actor
            .sync(std::slice::from_ref(&group), windows.clone())
            .unwrap();
        let user = group.policy.seats[0].unwrap();
        let request = Uuid::new_v4();
        actor.reserve(&group, user, request, Utc::now()).unwrap();
        actor.finish(request, Some(Decimal::from(4)));
        let newcomer = Uuid::new_v4();
        group.policy.seats[0] = Some(newcomer);
        group.policy.seats[1] = Some(user);
        group.policy.seats.push(Some(Uuid::new_v4()));
        group.policy.primary_limit_amount = Decimal::from(100);
        actor.sync(std::slice::from_ref(&group), windows).unwrap();
        assert_eq!(
            actor.inspect(&group, newcomer, Utc::now()).windows[0].remaining_amount,
            Decimal::ONE
        );
        assert_eq!(
            actor.inspect(&group, user, Utc::now()).windows[0].remaining_amount,
            Decimal::ONE
        );
        assert!(
            !actor
                .inspect(&group, group.policy.seats[2].unwrap(), Utc::now())
                .available
        );
    }

    #[test]
    fn restart_preserves_spend_and_pending_intents_are_not_refunded() {
        let (directory, mut actor, group, windows) = fixture();
        actor
            .sync(std::slice::from_ref(&group), windows.clone())
            .unwrap();
        let user = group.policy.seats[0].unwrap();
        let request = Uuid::new_v4();
        actor.reserve(&group, user, request, Utc::now()).unwrap();
        drop(actor);
        let mut recovered = Actor::new(
            LedgerStore::open(directory.path()).unwrap(),
            Arc::new(AtomicBool::new(true)),
        );
        recovered
            .sync(std::slice::from_ref(&group), windows)
            .unwrap();
        assert!(matches!(
            recovered.reserve(&group, user, Uuid::new_v4(), Utc::now()),
            Err(SharingError::Uncertain)
        ));
        recovered.finish(request, Some(Decimal::from(3)));
        assert_eq!(
            recovered.inspect(&group, user, Utc::now()).windows[0].used_amount,
            Decimal::from(3)
        );
        assert!(LedgerStore::open(directory.path()).is_err());
    }

    #[test]
    fn pending_concurrency_stale_windows_and_paused_membership_fail_closed() {
        let (_dir, mut actor, mut group, mut windows) = fixture();
        actor
            .sync(std::slice::from_ref(&group), windows.clone())
            .unwrap();
        let user = group.policy.seats[0].unwrap();
        for _ in 0..2 {
            actor
                .reserve(&group, user, Uuid::new_v4(), Utc::now())
                .unwrap();
        }
        assert!(matches!(
            actor.reserve(&group, user, Uuid::new_v4(), Utc::now()),
            Err(SharingError::ConcurrentLimited)
        ));
        windows[0].checked_at = Utc::now() - chrono::Duration::seconds(181);
        actor.sync(std::slice::from_ref(&group), windows).unwrap();
        assert!(matches!(
            actor.reserve(&group, user, Uuid::new_v4(), Utc::now()),
            Err(SharingError::WindowUnavailable)
        ));
        group.policy.enabled = false;
        assert!(matches!(
            actor.reserve(&group, user, Uuid::new_v4(), Utc::now()),
            Err(SharingError::Membership)
        ));
    }

    #[test]
    fn zero_percent_drift_cannot_reissue_money_or_permanently_lock_a_seat() {
        let (_dir, mut actor, group, mut windows) = fixture();
        windows.truncate(1);
        let first = Utc::now() - chrono::Duration::hours(2);
        windows[0].used_percent = 0;
        windows[0].checked_at = first;
        windows[0].scheduled_reset_at = first + chrono::Duration::hours(1);
        actor
            .sync(std::slice::from_ref(&group), windows.clone())
            .unwrap();
        let user = group.policy.seats[0].unwrap();
        let id = Uuid::new_v4();
        actor.reserve(&group, user, id, first).unwrap();
        actor.finish(id, Some(Decimal::from(5)));
        windows[0].checked_at = first + chrono::Duration::minutes(1);
        windows[0].scheduled_reset_at += chrono::Duration::minutes(1);
        actor
            .sync(std::slice::from_ref(&group), windows.clone())
            .unwrap();
        let before = actor.inspect(&group, user, windows[0].checked_at);
        assert_eq!(before.windows[0].used_amount, Decimal::from(5));
        assert_eq!(
            before.windows[0].reset_at,
            first + chrono::Duration::hours(1)
        );
        windows[0].checked_at = Utc::now();
        windows[0].scheduled_reset_at = Utc::now() + chrono::Duration::hours(1);
        actor
            .sync(std::slice::from_ref(&group), windows.clone())
            .unwrap();
        let after = actor.inspect(&group, user, Utc::now());
        assert_eq!(after.windows[0].used_amount, Decimal::ZERO);
        assert_ne!(after.windows[0].window_id, before.windows[0].window_id);
        actor.sync(std::slice::from_ref(&group), windows).unwrap();
        assert_eq!(
            actor.inspect(&group, user, Utc::now()).windows[0].window_id,
            after.windows[0].window_id
        );
    }

    #[test]
    fn pending_expansion_of_an_unused_zero_window_eventually_activates() {
        let (_directory, mut actor, mut group, mut windows) = fixture();
        windows.truncate(1);
        group.policy.seats = vec![None, None];
        let first = Utc::now() - chrono::Duration::hours(2);
        windows[0].used_percent = 0;
        windows[0].checked_at = first;
        windows[0].scheduled_reset_at = first + chrono::Duration::hours(1);
        actor
            .sync(std::slice::from_ref(&group), windows.clone())
            .unwrap();
        let new_user = Uuid::new_v4();
        group.policy.seats.push(Some(new_user));
        windows[0].checked_at = first + chrono::Duration::minutes(1);
        windows[0].scheduled_reset_at += chrono::Duration::minutes(1);
        actor
            .sync(std::slice::from_ref(&group), windows.clone())
            .unwrap();
        assert!(
            !actor
                .inspect(&group, new_user, windows[0].checked_at)
                .available
        );
        assert_eq!(
            actor.store.state.periods[&windows[0].id].reset_at,
            first + chrono::Duration::hours(1)
        );
        windows[0].checked_at = Utc::now();
        windows[0].scheduled_reset_at = Utc::now() + chrono::Duration::hours(1);
        actor.sync(std::slice::from_ref(&group), windows).unwrap();
        actor
            .reserve(&group, new_user, Uuid::new_v4(), Utc::now())
            .unwrap();
    }

    #[test]
    fn journal_recovers_torn_tail_and_checkpoint_overlap_without_free_or_double_usage() {
        let (directory, mut actor, group, windows) = fixture();
        actor
            .sync(std::slice::from_ref(&group), windows.clone())
            .unwrap();
        let user = group.policy.seats[0].unwrap();
        let settled = Uuid::new_v4();
        actor.reserve(&group, user, settled, Utc::now()).unwrap();
        actor.finish(settled, Some(Decimal::ONE));
        let pending = Uuid::new_v4();
        actor.reserve(&group, user, pending, Utc::now()).unwrap();
        let journal = std::fs::read(directory.path().join("ledger.wal")).unwrap();
        actor.store.checkpoint().unwrap();
        let mut wal = OpenOptions::new()
            .append(true)
            .open(directory.path().join("ledger.wal"))
            .unwrap();
        wal.write_all(&journal).unwrap();
        wal.write_all(br#"{"payload":"interrupted"#).unwrap();
        wal.sync_all().unwrap();
        drop(wal);
        drop(actor);
        let store = LedgerStore::open(directory.path()).unwrap();
        assert_eq!(store.state.periods[&windows[0].id].seats[0], Decimal::ONE);
        assert!(store.state.pending[&pending].uncertain);
        assert!(!store.state.pending.contains_key(&settled));
        assert_eq!(store.wal.metadata().unwrap().len(), 0);
    }

    #[test]
    fn corrupt_or_discontinuous_journal_fails_closed() {
        for corrupt_checksum in [false, true] {
            let (directory, mut actor, group, windows) = fixture();
            actor.sync(std::slice::from_ref(&group), windows).unwrap();
            let bytes = encode_frame(&LedgerChange {
                sequence: 500,
                ..Default::default()
            })
            .unwrap();
            let mut envelope: LedgerEnvelope = serde_json::from_slice(&bytes).unwrap();
            if corrupt_checksum {
                envelope.checksum[0] ^= 1;
            }
            let mut bytes = serde_json::to_vec(&envelope).unwrap();
            bytes.push(b'\n');
            actor.store.wal.write_all(&bytes).unwrap();
            actor.store.wal.sync_all().unwrap();
            drop(actor);
            let error = LedgerStore::open(directory.path()).err().unwrap();
            assert!(error.to_string().contains(if corrupt_checksum {
                "checksum"
            } else {
                "sequence gap"
            }));
        }
    }

    #[test]
    fn missing_checkpoint_does_not_reinitialize_a_surviving_journal() {
        let (directory, mut actor, group, windows) = fixture();
        actor.sync(std::slice::from_ref(&group), windows).unwrap();
        drop(actor);
        std::fs::remove_file(directory.path().join("ledger.json")).unwrap();
        assert!(
            LedgerStore::open(directory.path())
                .err()
                .unwrap()
                .to_string()
                .contains("checkpoint is missing")
        );
    }

    #[test]
    fn missing_journal_does_not_reissue_uncheckpointed_money() {
        let (directory, mut actor, group, windows) = fixture();
        actor.sync(std::slice::from_ref(&group), windows).unwrap();
        let id = Uuid::new_v4();
        actor
            .reserve(&group, group.policy.seats[0].unwrap(), id, Utc::now())
            .unwrap();
        actor.finish(id, Some(Decimal::ONE));
        drop(actor);
        std::fs::remove_file(directory.path().join("ledger.wal")).unwrap();
        assert!(
            LedgerStore::open(directory.path())
                .err()
                .unwrap()
                .to_string()
                .contains("journal is missing")
        );
    }

    #[test]
    fn a_full_read_queue_cannot_discard_settlement() {
        let (directory, mut actor, group, windows) = fixture();
        actor
            .sync(std::slice::from_ref(&group), windows.clone())
            .unwrap();
        let id = Uuid::new_v4();
        actor
            .reserve(&group, group.policy.seats[0].unwrap(), id, Utc::now())
            .unwrap();
        let (commands, receiver) = mpsc::channel(1);
        let (settlements, settlement_receiver) = mpsc::channel(2);
        let (reply, _) = oneshot::channel();
        assert!(commands.try_send(Command::Flush(reply)).is_ok());
        let runtime = SharingRuntime {
            inner: Some(Arc::new(RuntimeInner {
                commands,
                settlements,
                healthy: actor.healthy.clone(),
                ledger_id: actor.store.state.ledger_id,
            })),
        };
        runtime.finish(id, Some(Decimal::ONE));
        assert!(runtime.available());
        drop(runtime);
        actor.run(receiver, settlement_receiver);
        let store = LedgerStore::open(directory.path()).unwrap();
        assert!(!store.state.pending.contains_key(&id));
        assert_eq!(store.state.periods[&windows[0].id].seats[0], Decimal::ONE);
    }

    #[test]
    fn slot_replacement_inherits_uncertainty_and_rpm_is_shared_across_keys() {
        let (_directory, mut actor, mut group, windows) = fixture();
        group.policy.user_requests_per_minute = 2;
        group.policy.group_requests_per_minute = 3;
        actor.sync(std::slice::from_ref(&group), windows).unwrap();
        let now = Utc::now();
        let user = group.policy.seats[0].unwrap();
        for _ in 0..2 {
            let id = Uuid::new_v4();
            actor.reserve(&group, user, id, now).unwrap();
            actor.finish(id, Some(Decimal::ZERO));
        }
        assert!(matches!(
            actor.reserve(&group, user, Uuid::new_v4(), now),
            Err(SharingError::RateLimited)
        ));
        let other = group.policy.seats[1].unwrap();
        let id = Uuid::new_v4();
        actor.reserve(&group, other, id, now).unwrap();
        actor.finish(id, None);
        let replacement = Uuid::new_v4();
        group.policy.seats[1] = Some(replacement);
        assert!(matches!(
            actor.reserve(&group, replacement, Uuid::new_v4(), now),
            Err(SharingError::Uncertain)
        ));
    }
}
