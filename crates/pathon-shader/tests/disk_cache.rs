mod common;

use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{TestDevice, lock};
use pathon_rhi_types::DeviceApi;
use pathon_shader::{CacheError, CacheStore, FnCacheStore, PersistentPipelineCache, SaveOutcome};

// A store that keeps its bytes in memory and can be told to fail.
#[derive(Default)]
struct MemoryStore {
    bytes: Mutex<Option<Vec<u8>>>,
    stores: Mutex<usize>,
    fail_load: bool,
    fail_store: bool,
}

impl MemoryStore {
    fn holding(bytes: Vec<u8>) -> Arc<Self> {
        Arc::new(Self {
            bytes: Mutex::new(Some(bytes)),
            ..Self::default()
        })
    }

    fn contents(&self) -> Option<Vec<u8>> {
        lock(&self.bytes).clone()
    }

    fn writes(&self) -> usize {
        *lock(&self.stores)
    }
}

impl CacheStore for MemoryStore {
    fn load(&self) -> io::Result<Option<Vec<u8>>> {
        if self.fail_load {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
        }
        Ok(self.contents())
    }

    fn store(&self, bytes: &[u8]) -> io::Result<()> {
        if self.fail_store {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "read-only"));
        }
        *lock(&self.bytes) = Some(bytes.to_vec());
        *lock(&self.stores) += 1;
        Ok(())
    }
}

fn device_holding(blob: &[u8]) -> TestDevice {
    let device = TestDevice::new(Duration::ZERO);
    *lock(&device.cache_blob) = Some(blob.to_vec());
    device
}

fn saved_file(blob: &[u8]) -> Vec<u8> {
    let store = Arc::new(MemoryStore::default());
    let cache = PersistentPipelineCache::new(Arc::<MemoryStore>::clone(&store));
    cache.save(&device_holding(blob)).expect("save");
    store.contents().expect("written")
}

fn rejection_of(file: Vec<u8>) -> String {
    let seed = PersistentPipelineCache::new(MemoryStore::holding(file)).load();
    assert!(seed.data().is_none(), "a damaged file must not seed the device");
    assert_eq!(seed.warnings().len(), 1, "{:?}", seed.warnings());
    assert_eq!(seed.warnings()[0].code, "S0201");
    seed.warnings()[0].message.clone()
}

#[test]
fn nothing_stored_yet_is_an_empty_seed_without_a_warning() {
    let seed = PersistentPipelineCache::new(Arc::new(MemoryStore::default())).load();

    assert!(seed.data().is_none());
    assert_eq!(seed.warnings(), []);
}

#[test]
fn a_saved_blob_comes_back_unchanged() {
    let store = Arc::new(MemoryStore::default());
    let blob: Vec<u8> = (0..=255).cycle().take(10_000).collect();

    let outcome = PersistentPipelineCache::new(Arc::<MemoryStore>::clone(&store))
        .save(&device_holding(&blob))
        .expect("save");
    let seed = PersistentPipelineCache::new(Arc::<MemoryStore>::clone(&store)).load();

    assert!(matches!(outcome, SaveOutcome::Saved { bytes } if bytes > blob.len()));
    assert_eq!(seed.data().expect("a seed").as_bytes(), blob);
    assert_eq!(seed.warnings(), []);
}

#[test]
fn saving_an_unchanged_blob_does_not_write_again() {
    let store = Arc::new(MemoryStore::default());
    let cache = PersistentPipelineCache::new(Arc::<MemoryStore>::clone(&store));
    let device = device_holding(&[1, 2, 3]);

    assert!(matches!(cache.save(&device), Ok(SaveOutcome::Saved { .. })));
    assert!(matches!(cache.save(&device), Ok(SaveOutcome::Unchanged)));
    assert_eq!(store.writes(), 1);

    *lock(&device.cache_blob) = Some(vec![1, 2, 3, 4]);
    assert!(matches!(cache.save(&device), Ok(SaveOutcome::Saved { .. })));
    assert_eq!(store.writes(), 2);
}

#[test]
fn a_blob_equal_to_the_loaded_one_is_not_written_back() {
    let store = MemoryStore::holding(saved_file(&[5, 6, 7]));
    let cache = PersistentPipelineCache::new(Arc::<MemoryStore>::clone(&store));
    assert!(cache.load().data().is_some());

    assert!(matches!(
        cache.save(&device_holding(&[5, 6, 7])),
        Ok(SaveOutcome::Unchanged)
    ));
    assert_eq!(store.writes(), 0);
}

#[test]
fn a_device_without_pipelines_or_without_a_cache_leaves_the_stored_file_alone() {
    let store = MemoryStore::holding(vec![42; 8]);
    let cache = PersistentPipelineCache::new(Arc::<MemoryStore>::clone(&store));

    assert!(matches!(cache.save(&device_holding(&[])), Ok(SaveOutcome::Empty)));
    assert!(matches!(
        cache.save(&TestDevice::new(Duration::ZERO)),
        Ok(SaveOutcome::Unsupported)
    ));
    assert_eq!(store.contents(), Some(vec![42; 8]));
}

#[test]
fn damaged_files_become_a_warning_and_an_empty_cache_never_an_error() {
    let good = saved_file(&[9; 256]);

    assert!(rejection_of(Vec::new()).contains("empty"));
    assert!(rejection_of(good[..good.len() / 2].to_vec()).contains("length"));
    assert!(rejection_of(good[..10].to_vec()).contains("truncated"));
    let mut flipped = good.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 0x40;
    assert!(rejection_of(flipped).contains("checksum"));
    assert!(rejection_of((0..=255u8).cycle().take(4096).collect()).contains("not a pathon pipeline cache"));
}

#[test]
fn a_file_written_by_another_pathon_version_is_not_used() {
    let mut file = saved_file(&[1; 64]);
    let version = env!("CARGO_PKG_VERSION").as_bytes();
    let at = file
        .windows(version.len())
        .position(|window| window == version)
        .expect("the version is in the header");
    file[at] = b'9';

    assert!(rejection_of(file).contains("another pathon version"));
}

#[test]
fn an_unreadable_store_is_a_warning_too() {
    let store = Arc::new(MemoryStore {
        fail_load: true,
        ..MemoryStore::default()
    });

    let seed = PersistentPipelineCache::new(store).load();

    assert!(seed.data().is_none());
    assert_eq!(seed.warnings()[0].code, "S0201");
    assert!(seed.warnings()[0].message.contains("denied"), "{:?}", seed.warnings());
}

#[test]
fn a_failing_write_keeps_the_previous_file_and_reports_a_warning() {
    let store = Arc::new(MemoryStore {
        bytes: Mutex::new(Some(vec![1, 2, 3])),
        fail_store: true,
        ..MemoryStore::default()
    });

    let error = PersistentPipelineCache::new(Arc::<MemoryStore>::clone(&store))
        .save(&device_holding(&[7; 32]))
        .expect_err("the store is read-only");

    assert!(matches!(error, CacheError::Io(_)));
    assert_eq!(store.contents(), Some(vec![1, 2, 3]));
    let diagnostic = error.diagnostic();
    assert_eq!(diagnostic.code, "S0202");
    assert!(diagnostic.message.contains("read-only"), "{diagnostic}");
}

#[test]
fn a_blob_saved_for_another_adapter_is_explained_once_the_device_exists() {
    let device = device_holding(&[3; 16]);
    let store = MemoryStore::holding(saved_file(&[3; 16]));
    let seed = PersistentPipelineCache::new(store).load();

    assert!(seed.adapter_mismatch(device.caps()).is_none(), "same adapter");
    let mut other = device.caps().clone();
    other.adapter.name = "Another GPU".to_owned();
    let warning = seed.adapter_mismatch(&other).expect("a different adapter");
    assert_eq!(warning.code, "S0201");
    assert!(warning.message.contains("Another GPU"), "{warning}");
}

#[test]
fn periodic_saving_starts_its_clock_on_the_first_call_and_then_waits_for_the_interval() {
    let store = Arc::new(MemoryStore::default());
    let cache =
        PersistentPipelineCache::new(Arc::<MemoryStore>::clone(&store)).with_save_interval(Duration::from_secs(10));
    let device = device_holding(&[8; 16]);
    let start = Instant::now();

    assert!(
        cache.save_if_due(&device, start).is_none(),
        "the first call only starts the clock"
    );
    assert!(cache.save_if_due(&device, start + Duration::from_secs(9)).is_none());
    let due = cache.save_if_due(&device, start + Duration::from_secs(10));
    assert!(matches!(due, Some(Ok(SaveOutcome::Saved { .. }))), "{due:?}");
    assert!(cache.save_if_due(&device, start + Duration::from_secs(15)).is_none());
    assert_eq!(store.writes(), 1);
}

#[test]
fn closures_can_stand_in_for_a_store() {
    let slot: Arc<Mutex<Option<Vec<u8>>>> = Arc::default();
    let (reader, writer) = (Arc::clone(&slot), Arc::clone(&slot));
    let store = FnCacheStore::new(
        move || Ok(lock(&reader).clone()),
        move |bytes| {
            *lock(&writer) = Some(bytes.to_vec());
            Ok(())
        },
    );
    let cache = PersistentPipelineCache::new(Arc::new(store));

    cache.save(&device_holding(&[4; 20])).expect("save");

    assert_eq!(cache.load().data().expect("seed").as_bytes(), [4; 20]);
}

#[cfg(feature = "fs")]
mod file {
    use std::fs;
    use std::path::PathBuf;

    use pathon_shader::FileCacheStore;

    use super::*;

    fn scratch_directory(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        std::env::temp_dir().join(format!("pathon-cache-test-{}-{nanos}-{name}", std::process::id()))
    }

    #[test]
    fn the_file_store_creates_directories_replaces_atomically_and_leaves_no_temporary_file() {
        let directory = scratch_directory("replace");
        let path = directory.join("nested").join("pipelines.bin");
        let store = FileCacheStore::new(&path);

        assert!(store.load().expect("a missing file is not an error").is_none());
        store.store(&[1, 2, 3]).expect("first write");
        store.store(&[4, 5]).expect("replacement");

        assert_eq!(store.load().expect("load"), Some(vec![4, 5]));
        let leftovers: Vec<_> = fs::read_dir(path.parent().expect("parent"))
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(leftovers, ["pipelines.bin"], "the temporary file is renamed away");
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn a_cache_saved_to_a_file_seeds_the_next_run() {
        let directory = scratch_directory("seed");
        let path = directory.join("pipelines.bin");

        PersistentPipelineCache::new(Arc::new(FileCacheStore::new(&path)))
            .save(&device_holding(&[11; 512]))
            .expect("save");
        let seed = PersistentPipelineCache::new(Arc::new(FileCacheStore::new(&path))).load();

        assert_eq!(seed.data().expect("seed").as_bytes(), [11; 512]);
        fs::write(&path, b"").expect("truncate");
        let after_crash = PersistentPipelineCache::new(Arc::new(FileCacheStore::new(&path))).load();
        assert!(after_crash.data().is_none());
        assert_eq!(after_crash.warnings()[0].code, "S0201");
        fs::remove_dir_all(directory).expect("cleanup");
    }
}
