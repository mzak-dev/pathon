use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use pathon_rhi_types::{Caps, DeviceApi, PipelineCacheData};

use crate::diagnostic::{CACHE_NOT_SAVED, CACHE_REJECTED, ShaderDiagnostic};

const MAGIC: &[u8; 8] = b"PTHNPCAC";
const FORMAT_VERSION: u32 = 1;
const PATHON_VERSION: &str = env!("CARGO_PKG_VERSION");
// A driver cache is megabytes; a file this big is not one.
const MAX_FILE_BYTES: usize = 512 << 20;
const DEFAULT_SAVE_INTERVAL: Duration = Duration::from_secs(60);

/// Where a host keeps the pipeline cache between runs; the renderer core knows no file system (ADR-0019).
///
/// One store holds one blob. `store` must replace it atomically (write beside it, then rename): a crash while saving
/// leaves the previous blob, never half of a new one.
pub trait CacheStore: Send + Sync {
    /// The bytes of the last `store`, or `None` when nothing was stored yet.
    fn load(&self) -> io::Result<Option<Vec<u8>>>;

    fn store(&self, bytes: &[u8]) -> io::Result<()>;
}

type LoadFn = dyn Fn() -> io::Result<Option<Vec<u8>>> + Send + Sync;
type StoreFn = dyn Fn(&[u8]) -> io::Result<()> + Send + Sync;

/// A [`CacheStore`] made of two closures, for hosts that keep the blob somewhere other than a file.
pub struct FnCacheStore {
    load: Box<LoadFn>,
    store: Box<StoreFn>,
}

impl FnCacheStore {
    pub fn new(
        load: impl Fn() -> io::Result<Option<Vec<u8>>> + Send + Sync + 'static,
        store: impl Fn(&[u8]) -> io::Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            load: Box::new(load),
            store: Box::new(store),
        }
    }
}

impl std::fmt::Debug for FnCacheStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FnCacheStore").finish_non_exhaustive()
    }
}

impl CacheStore for FnCacheStore {
    fn load(&self) -> io::Result<Option<Vec<u8>>> {
        (self.load)()
    }

    fn store(&self, bytes: &[u8]) -> io::Result<()> {
        (self.store)(bytes)
    }
}

/// A file; the host gives the path (typically in the application's cache directory). Not available on wasm.
#[cfg(all(feature = "fs", not(target_arch = "wasm32")))]
#[derive(Clone, Debug)]
pub struct FileCacheStore {
    path: std::path::PathBuf,
}

#[cfg(all(feature = "fs", not(target_arch = "wasm32")))]
impl FileCacheStore {
    #[must_use]
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    // A temporary name per process and store, so two processes saving at once do not write the same file.
    fn temporary_path(&self) -> std::path::PathBuf {
        let mut name = self.path.file_name().map_or_default(std::ffi::OsStr::to_os_string);
        name.push(format!(".{}.tmp", std::process::id()));
        self.path.with_file_name(name)
    }
}

#[cfg(all(feature = "fs", not(target_arch = "wasm32")))]
impl CacheStore for FileCacheStore {
    fn load(&self) -> io::Result<Option<Vec<u8>>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn store(&self, bytes: &[u8]) -> io::Result<()> {
        if let Some(directory) = self.path.parent().filter(|directory| !directory.as_os_str().is_empty()) {
            std::fs::create_dir_all(directory)?;
        }
        let temporary = self.temporary_path();
        let written = std::fs::write(&temporary, bytes).and_then(|()| std::fs::rename(&temporary, &self.path));
        if written.is_err() {
            drop(std::fs::remove_file(&temporary));
        }
        written
    }
}

/// What a previous run left behind, after the checks that need no device.
#[derive(Debug)]
pub struct CacheSeed {
    data: Option<PipelineCacheData>,
    identity: Option<String>,
    warnings: Vec<ShaderDiagnostic>,
}

impl CacheSeed {
    /// The blob for `DeviceRequest::pipeline_cache`; `None` when there was nothing usable.
    #[must_use]
    pub fn data(&self) -> Option<&PipelineCacheData> {
        self.data.as_ref()
    }

    /// Why a stored blob was not used (S0201), or why it could not be read; empty when it was used or absent.
    #[must_use]
    pub fn warnings(&self) -> &[ShaderDiagnostic] {
        &self.warnings
    }

    /// Call once the device exists: a blob saved for another adapter or driver is ignored by the backend (ADR-0019),
    /// and the seed knows which one wrote it, so this explains why the first start was not faster.
    #[must_use]
    pub fn adapter_mismatch(&self, caps: &Caps) -> Option<ShaderDiagnostic> {
        let saved_for = self.identity.as_deref()?;
        let running_on = adapter_identity(caps);
        (saved_for != running_on).then(|| {
            ShaderDiagnostic::warning(
                CACHE_REJECTED,
                format!("the pipeline cache was saved for `{saved_for}`, but this device is `{running_on}`"),
            )
            .with_hint("the backend starts with an empty cache and the file is replaced on the next save")
        })
    }

    fn empty() -> Self {
        Self {
            data: None,
            identity: None,
            warnings: Vec::new(),
        }
    }

    fn rejected(reason: &str) -> Self {
        Self {
            warnings: vec![
                ShaderDiagnostic::warning(
                    CACHE_REJECTED,
                    format!("the stored pipeline cache is not used: {reason}"),
                )
                .with_hint("the cache starts empty and the file is replaced on the next save"),
            ],
            ..Self::empty()
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum SaveOutcome {
    Saved {
        bytes: usize,
    },
    /// The blob is what the store already holds.
    Unchanged,
    /// The device has no pipelines in its cache yet (or a driver that keeps none, like software ones); the stored
    /// blob is left alone.
    Empty,
    /// The device has no pipeline cache (`Features::PIPELINE_CACHE` is off, or the null backend).
    Unsupported,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CacheError {
    #[error("the pipeline cache could not be written: {0}")]
    Io(#[from] io::Error),
}

impl CacheError {
    #[must_use]
    pub fn diagnostic(&self) -> ShaderDiagnostic {
        ShaderDiagnostic::warning(CACHE_NOT_SAVED, self.to_string())
            .with_hint("the next start compiles pipelines from scratch; check that the cache directory is writable")
    }
}

/// Keeps a device's pipeline cache between runs (M3-05, ADR-0019): [`Self::load`] before the device exists,
/// [`Self::save`] while it runs and when it is closed.
///
/// ```ignore
/// let cache = PersistentPipelineCache::new(Arc::new(FileCacheStore::new(app_cache_dir.join("pipelines.bin"))));
/// let seed = cache.load();
/// let device = Device::build(&DeviceRequest { pipeline_cache: seed.data().cloned(), ..request })?;
/// // every frame, or every few seconds from a helper thread:
/// cache.save_if_due(&device, Instant::now());
/// // on shutdown:
/// cache.save(&device)?;
/// ```
///
/// The file is a header (magic, format version, pathon version, the adapter that wrote it, length and checksum of the
/// payload) followed by the backend's blob. An empty, truncated, corrupted or foreign file is a warning and an empty
/// cache, never an error. The blob itself is validated again by the backend against adapter, driver and wgpu version.
pub struct PersistentPipelineCache {
    store: Arc<dyn CacheStore>,
    interval: Duration,
    state: Mutex<SaveState>,
}

#[derive(Default)]
struct SaveState {
    last_payload_checksum: Option<u64>,
    last_attempt: Option<Instant>,
}

impl std::fmt::Debug for PersistentPipelineCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentPipelineCache")
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

impl PersistentPipelineCache {
    #[must_use]
    pub fn new(store: Arc<dyn CacheStore>) -> Self {
        Self {
            store,
            interval: DEFAULT_SAVE_INTERVAL,
            state: Mutex::new(SaveState::default()),
        }
    }

    /// How often [`Self::save_if_due`] writes; 60 seconds by default.
    #[must_use]
    pub fn with_save_interval(self, interval: Duration) -> Self {
        Self { interval, ..self }
    }

    /// Reads and checks the stored blob. Never fails: problems are warnings in the seed and the device starts with an
    /// empty cache.
    #[must_use]
    pub fn load(&self) -> CacheSeed {
        let bytes = match self.store.load() {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return CacheSeed::empty(),
            Err(error) => return CacheSeed::rejected(&format!("it could not be read ({error})")),
        };
        match decode(&bytes) {
            Ok(decoded) => {
                self.lock().last_payload_checksum = Some(checksum(&decoded.payload));
                CacheSeed {
                    // SAFETY: the payload passed the header, version, length and checksum checks, so it is the very
                    // bytes `pipeline_cache_data` returned when this file was saved; the backend validates its own
                    // header again before the driver sees them (ADR-0019).
                    data: Some(unsafe { PipelineCacheData::from_bytes(decoded.payload) }),
                    identity: Some(decoded.identity),
                    warnings: Vec::new(),
                }
            }
            Err(reason) => CacheSeed::rejected(reason),
        }
    }

    /// Writes the device's current blob to the store unless it is empty or what the store already holds.
    pub fn save(&self, device: &dyn DeviceApi) -> Result<SaveOutcome, CacheError> {
        let Some(blob) = device.pipeline_cache_data() else {
            return Ok(SaveOutcome::Unsupported);
        };
        let payload = blob.as_bytes();
        if payload.is_empty() {
            return Ok(SaveOutcome::Empty);
        }
        let payload_checksum = checksum(payload);
        if self.lock().last_payload_checksum == Some(payload_checksum) {
            return Ok(SaveOutcome::Unchanged);
        }
        let file = encode(&adapter_identity(device.caps()), payload, payload_checksum);
        self.store.store(&file)?;
        self.lock().last_payload_checksum = Some(payload_checksum);
        Ok(SaveOutcome::Saved { bytes: file.len() })
    }

    /// [`Self::save`] when the save interval has passed since the last attempt, for calling every frame: the first
    /// call only starts the clock. `now` is the host's, so the cache never reads a clock (`Instant::now` panics on
    /// `wasm32-unknown-unknown`). Returns `None` when nothing was due.
    pub fn save_if_due(&self, device: &dyn DeviceApi, now: Instant) -> Option<Result<SaveOutcome, CacheError>> {
        {
            let mut state = self.lock();
            match state.last_attempt {
                None => {
                    state.last_attempt = Some(now);
                    return None;
                }
                Some(previous) if now.saturating_duration_since(previous) < self.interval => return None,
                Some(_) => state.last_attempt = Some(now),
            }
        }
        Some(self.save(device))
    }

    fn lock(&self) -> MutexGuard<'_, SaveState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Names the adapter, driver and backend a blob belongs to.
fn adapter_identity(caps: &Caps) -> String {
    format!(
        "{:?} {} (driver {}, {:?})",
        caps.backend, caps.adapter.name, caps.adapter.driver, caps.adapter.device_type
    )
}

// FNV-1a: detects truncation and flipped bits, which is all a local file needs; the backend validates the rest.
fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn encode(identity: &str, payload: &[u8], payload_checksum: u64) -> Vec<u8> {
    let mut file = Vec::with_capacity(payload.len() + 128);
    file.extend_from_slice(MAGIC);
    file.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    push_text(&mut file, PATHON_VERSION);
    push_text(&mut file, identity);
    file.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    file.extend_from_slice(&payload_checksum.to_le_bytes());
    file.extend_from_slice(payload);
    file
}

fn push_text(file: &mut Vec<u8>, text: &str) {
    let bytes = &text.as_bytes()[..text.len().min(usize::from(u16::MAX))];
    file.extend_from_slice(&u16::try_from(bytes.len()).unwrap_or(u16::MAX).to_le_bytes());
    file.extend_from_slice(bytes);
}

struct Decoded {
    identity: String,
    payload: Vec<u8>,
}

struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.rest.split_at_checked(length)?;
        self.rest = rest;
        Some(head)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    fn text(&mut self) -> Option<String> {
        let length = usize::from(u16::from_le_bytes(self.array()?));
        String::from_utf8(self.take(length)?.to_vec()).ok()
    }
}

fn decode(file: &[u8]) -> Result<Decoded, &'static str> {
    if file.is_empty() {
        return Err("the file is empty");
    }
    if file.len() > MAX_FILE_BYTES {
        return Err("the file is implausibly large");
    }
    let mut reader = Reader { rest: file };
    if reader.take(MAGIC.len()) != Some(MAGIC.as_slice()) {
        return Err("it is not a pathon pipeline cache");
    }
    let truncated = "the file is truncated";
    let version = u32::from_le_bytes(reader.array().ok_or(truncated)?);
    if version != FORMAT_VERSION {
        return Err("it was written in another file format version");
    }
    if reader.text().ok_or(truncated)? != PATHON_VERSION {
        return Err("it was written by another pathon version");
    }
    let identity = reader.text().ok_or(truncated)?;
    let length = u64::from_le_bytes(reader.array().ok_or(truncated)?);
    let expected_checksum = u64::from_le_bytes(reader.array().ok_or(truncated)?);
    if usize::try_from(length).ok() != Some(reader.rest.len()) {
        return Err("the payload length does not match the file (truncated or extended)");
    }
    if length == 0 {
        return Err("the payload is empty");
    }
    if checksum(reader.rest) != expected_checksum {
        return Err("the payload is corrupted (checksum mismatch)");
    }
    Ok(Decoded {
        identity,
        payload: reader.rest.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(payload: &[u8]) -> Vec<u8> {
        encode("Vulkan Test GPU (driver 1.0, DiscreteGpu)", payload, checksum(payload))
    }

    #[test]
    fn a_saved_file_decodes_to_its_payload_and_adapter() {
        let decoded = decode(&file(&[1, 2, 3, 4])).expect("valid");
        assert_eq!(decoded.payload, [1, 2, 3, 4]);
        assert_eq!(decoded.identity, "Vulkan Test GPU (driver 1.0, DiscreteGpu)");
    }

    #[test]
    fn damaged_files_are_rejected_with_a_reason() {
        let good = file(&[9; 64]);
        let mut flipped = good.clone();
        *flipped.last_mut().expect("payload") ^= 1;
        let mut extended = good.clone();
        extended.push(0);
        let mut other_version = good.clone();
        other_version[MAGIC.len()] = 99;
        let cases: [(&[u8], &str); 6] = [
            (&[], "empty"),
            (&good[..good.len() - 10], "length"),
            (&flipped, "checksum"),
            (&extended, "length"),
            (&other_version, "format version"),
            (b"not a cache at all, just text", "not a pathon"),
        ];
        for (bytes, expected) in cases {
            let reason = decode(bytes).err().expect("rejected");
            assert!(reason.contains(expected), "{reason} should mention {expected}");
        }
        assert!(
            decode(&good[..MAGIC.len() + 2])
                .err()
                .expect("rejected")
                .contains("truncated")
        );
    }

    #[test]
    fn checksum_differs_for_different_payloads() {
        assert_ne!(checksum(&[1, 2, 3]), checksum(&[1, 2, 4]));
        assert_ne!(checksum(&[]), checksum(&[0]));
    }
}
