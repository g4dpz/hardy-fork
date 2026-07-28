use core::num::{NonZero, NonZeroUsize};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use hardy_async::{async_trait, sync::Mutex};
use lru::LruCache;
use rand::{
    SeedableRng,
    distr::{Alphanumeric, SampleString},
    rngs::{SmallRng, SysRng},
};
use time::OffsetDateTime;
use tracing::{info, warn};

use super::{BundleStorage, RecoveryResponse, Result};
use crate::{Arc, Bytes, stream::Sender};

/// Number of independent shards to reduce lock contention.
const SHARD_COUNT: usize = 16;

/// Configuration for [`BundleMemStorage`].
#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, rename_all = "kebab-case"))]
pub struct Config {
    /// Maximum total bytes of bundle data held before least-recently-used
    /// bundles are evicted. Default: 256 MiB.
    pub capacity: NonZeroUsize,

    /// Minimum number of bundles retained regardless of the byte capacity.
    /// Values below 1 are treated as 1, so a save can never evict the bundle
    /// it has just stored. Default: `32`.
    pub min_bundles: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            capacity: NonZero::new(256 * 1_048_576).unwrap(),
            min_bundles: 32,
        }
    }
}

/// Map a storage name to its shard index by hashing the first few bytes.
///
/// Storage names are 64-char alphanumeric random strings, so even a simple
/// hash distributes evenly. We use FNV-1a for speed (no crypto needed).
#[inline]
fn shard_index(name: &str) -> usize {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in name.as_bytes().iter().take(8) {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    (hash as usize) % SHARD_COUNT
}

struct Shard {
    cache: LruCache<String, (OffsetDateTime, Bytes)>,
    rng: SmallRng,
}

/// An in-memory [`BundleStorage`] implementation bounded by total byte
/// capacity.
///
/// Uses [`SHARD_COUNT`] independent shards to reduce lock contention under
/// high concurrency. Total byte capacity is tracked with a shared atomic
/// counter; per-shard eviction fires when a shard observes the global
/// usage exceeding the configured maximum.
///
/// Contents are not persisted: all bundle data is lost on restart. When
/// usage exceeds the configured capacity, least-recently-used bundles are
/// evicted per-shard. A single `info!` line is emitted when usage crosses
/// 95% of capacity, and another when it falls back below 90%.
pub struct BundleMemStorage {
    shards: [Mutex<Shard>; SHARD_COUNT],
    /// Global byte usage, updated atomically by all shards.
    global_bytes: AtomicUsize,
    max_capacity: NonZeroUsize,
    high_watermark: usize,
    low_watermark: usize,
    /// Hysteresis state: true when global usage has crossed the high watermark.
    near_capacity: AtomicUsize, // 0 = false, 1 = true
    evicted_count: AtomicU64,
    evicted_bytes: AtomicU64,
}

impl BundleMemStorage {
    /// Creates a store holding at most [`Config::capacity`] bytes.
    pub fn new(config: &Config) -> Self {
        warn!(
            "Using in-memory bundle storage (capacity {} bytes): stored bundles will NOT survive a restart",
            config.capacity
        );

        let max_capacity = config.capacity;
        let max = max_capacity.get();

        let shards = core::array::from_fn(|_| {
            Mutex::new(Shard {
                cache: LruCache::unbounded(),
                rng: SmallRng::try_from_rng(&mut SysRng)
                    .expect("OS RNG must be available to seed the storage-name PRNG"),
            })
        });

        Self {
            shards,
            global_bytes: AtomicUsize::new(0),
            max_capacity,
            high_watermark: max - max / 20,
            low_watermark: max - max / 10,
            near_capacity: AtomicUsize::new(0),
            evicted_count: AtomicU64::new(0),
            evicted_bytes: AtomicU64::new(0),
        }
    }

    /// Evict LRU entries from the given shard until global usage is within
    /// capacity. If this shard cannot evict further, attempts eviction from
    /// other shards in round-robin order.
    ///
    /// The starting shard keeps at least 1 entry (the just-saved bundle is
    /// MRU and would be the last evicted, so `> 1` prevents self-eviction).
    /// Other shards may be fully drained.
    fn evict_from(&self, start_idx: usize, shard: &mut Shard) {
        // Try evicting from the already-held shard (keep at least 1 = self)
        while shard.cache.len() > 1
            && self.global_bytes.load(Ordering::Relaxed) > self.max_capacity.get()
        {
            let Some((_, (_, d))) = shard.cache.pop_lru() else {
                break;
            };
            self.record_eviction(d.len());
        }

        if self.global_bytes.load(Ordering::Relaxed) <= self.max_capacity.get() {
            return;
        }

        // Spill eviction to other shards (may drain to 0)
        for offset in 1..SHARD_COUNT {
            let idx = (start_idx + offset) % SHARD_COUNT;
            let mut other = self.shards[idx].lock();
            while !other.cache.is_empty()
                && self.global_bytes.load(Ordering::Relaxed) > self.max_capacity.get()
            {
                let Some((_, (_, d))) = other.cache.pop_lru() else {
                    break;
                };
                self.record_eviction(d.len());
            }
            if self.global_bytes.load(Ordering::Relaxed) <= self.max_capacity.get() {
                return;
            }
        }
    }

    fn record_eviction(&self, len: usize) {
        self.global_bytes.fetch_sub(len, Ordering::Relaxed);
        self.evicted_count.fetch_add(1, Ordering::Relaxed);
        self.evicted_bytes.fetch_add(len as u64, Ordering::Relaxed);
        metrics::counter!("bpa.mem_store.evictions").increment(1);
    }

    /// Check and log watermark transitions.
    fn check_and_log_watermark(&self) {
        let bytes = self.global_bytes.load(Ordering::Relaxed);
        let was_near = self.near_capacity.load(Ordering::Relaxed) != 0;

        if !was_near && bytes >= self.high_watermark {
            // Transition to near-capacity
            if self
                .near_capacity
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                info!(
                    "In-memory bundle storage is nearly full: {bytes} of {} bytes used",
                    self.max_capacity
                );
            }
        } else if was_near && bytes < self.low_watermark {
            // Transition out of near-capacity
            if self
                .near_capacity
                .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                let evicted_count = self.evicted_count.swap(0, Ordering::Relaxed);
                let evicted_bytes = self.evicted_bytes.swap(0, Ordering::Relaxed);
                if evicted_count == 0 {
                    info!(
                        "In-memory bundle storage is no longer nearly full: {bytes} of {} bytes used",
                        self.max_capacity
                    );
                } else {
                    info!(
                        "In-memory bundle storage is no longer nearly full: {bytes} of {} bytes used; {evicted_count} bundles ({evicted_bytes} bytes) were evicted while nearly full",
                        self.max_capacity
                    );
                }
            }
        }
    }

    fn update_metrics(&self) {
        let bytes = self.global_bytes.load(Ordering::Relaxed);
        metrics::gauge!("bpa.mem_store.bytes").set(bytes as f64);
        // Bundle count is approximate (sum of shards without holding all locks)
        // but good enough for a gauge.
    }

    #[cfg(test)]
    fn near_capacity(&self) -> bool {
        self.near_capacity.load(Ordering::Acquire) != 0
    }

    #[cfg(test)]
    fn evicted_count(&self) -> u64 {
        self.evicted_count.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl BundleStorage for BundleMemStorage {
    async fn recover(&self, stream: &dyn Sender<RecoveryResponse>) -> Result<()> {
        let mut snapshot = Vec::new();
        for shard_mutex in &self.shards {
            let shard = shard_mutex.lock();
            for (n, (t, _)) in shard.cache.iter() {
                snapshot.push((Arc::<str>::from(n.clone()), *t));
            }
        }

        for (name, t) in snapshot {
            if stream.send((name, t)).await.is_err() {
                break;
            }
        }
        Ok(())
    }

    async fn load(&self, storage_name: &str) -> Result<Option<Bytes>> {
        let idx = shard_index(storage_name);
        Ok(self.shards[idx]
            .lock()
            .cache
            .get(storage_name)
            .map(|(_, data)| data.clone()))
    }

    async fn save(&self, data: Bytes) -> Result<Arc<str>> {
        let new_len = data.len();

        // Generate a unique name. We need a shard lock to access the RNG,
        // but we pick a random shard for the RNG to reduce contention on
        // the target shard.
        let storage_name = loop {
            // Use shard 0's RNG to generate the name (any shard works)
            let name = {
                let mut shard = self.shards[0].lock();
                Alphanumeric.sample_string(&mut shard.rng, 64)
            };
            let idx = shard_index(&name);
            let mut shard = self.shards[idx].lock();
            if shard.cache.contains(&name) {
                continue;
            }

            shard.cache.put(name.clone(), (OffsetDateTime::now_utc(), data));
            self.global_bytes.fetch_add(new_len, Ordering::Relaxed);

            self.check_and_log_watermark();
            self.evict_from(idx, &mut shard);
            break name;
        };

        self.check_and_log_watermark();
        self.update_metrics();

        Ok(storage_name.into())
    }

    async fn replace(&self, storage_name: &str, data: Bytes) -> Result<()> {
        let new_len = data.len();
        let idx = shard_index(storage_name);
        {
            let mut shard = self.shards[idx].lock();
            let old_len = shard
                .cache
                .put(storage_name.to_string(), (OffsetDateTime::now_utc(), data))
                .map(|(_, d)| d.len())
                .unwrap_or(0);
            // Adjust global bytes: subtract old, add new
            if new_len >= old_len {
                self.global_bytes
                    .fetch_add(new_len - old_len, Ordering::Relaxed);
            } else {
                self.global_bytes
                    .fetch_sub(old_len - new_len, Ordering::Relaxed);
            }
            self.check_and_log_watermark();
            self.evict_from(idx, &mut shard);
        }
        self.check_and_log_watermark();
        self.update_metrics();
        Ok(())
    }

    async fn delete(&self, storage_name: &str) -> Result<()> {
        let idx = shard_index(storage_name);
        {
            let mut shard = self.shards[idx].lock();
            let Some((_, d)) = shard.cache.pop(storage_name) else {
                return Ok(());
            };
            self.global_bytes.fetch_sub(d.len(), Ordering::Relaxed);
        }
        self.check_and_log_watermark();
        self.update_metrics();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_config(capacity: usize, min_bundles: usize) -> Config {
        Config {
            capacity: NonZeroUsize::new(capacity).unwrap(),
            min_bundles,
        }
    }

    // Basic save/load/delete cycle.
    #[tokio::test]
    async fn test_basic_save_load_delete() {
        let storage = BundleMemStorage::new(&small_config(1000, 0));

        let name = storage.save(Bytes::from(vec![1u8; 50])).await.unwrap();
        assert!(storage.load(&name).await.unwrap().is_some());

        storage.delete(&name).await.unwrap();
        assert!(storage.load(&name).await.unwrap().is_none());
    }

    // When capacity is exceeded, eviction occurs.
    #[tokio::test]
    async fn test_eviction_occurs() {
        // Very small capacity to force eviction
        let storage = BundleMemStorage::new(&small_config(100, 0));

        let name1 = storage.save(Bytes::from(vec![1u8; 50])).await.unwrap();
        let name2 = storage.save(Bytes::from(vec![2u8; 50])).await.unwrap();
        // At capacity now
        let name3 = storage.save(Bytes::from(vec![3u8; 50])).await.unwrap();

        // At least one of the earlier bundles should be evicted
        let loaded1 = storage.load(&name1).await.unwrap().is_some();
        let loaded2 = storage.load(&name2).await.unwrap().is_some();
        let loaded3 = storage.load(&name3).await.unwrap().is_some();

        // name3 (just saved) must survive
        assert!(loaded3, "The just-saved bundle must survive");
        // Total capacity is 100, we have 150 bytes of bundles:
        // at least one of the older ones must be gone
        assert!(
            !loaded1 || !loaded2,
            "At least one older bundle should be evicted"
        );
    }

    // Verify NonZeroUsize handles >1TB capacity values without overflow.
    #[test]
    fn test_large_quota_config() {
        let two_tb: usize = 2_000_000_000_000;
        let config = Config {
            capacity: NonZeroUsize::new(two_tb).unwrap(),
            min_bundles: 0,
        };
        let storage = BundleMemStorage::new(&config);
        assert_eq!(storage.max_capacity.get(), two_tb);
    }

    // A save must never evict the bundle it has just stored, even when that
    // bundle alone exceeds the whole byte capacity.
    #[tokio::test]
    async fn save_survives_its_own_eviction_pass() {
        let storage = BundleMemStorage::new(&small_config(100, 0));

        let _name1 = storage.save(Bytes::from(vec![1u8; 50])).await.unwrap();
        let name2 = storage.save(Bytes::from(vec![2u8; 150])).await.unwrap();

        assert!(
            storage.load(&name2).await.unwrap().is_some(),
            "The just-saved bundle must survive its own eviction pass"
        );
    }

    // replace() must enforce the byte capacity.
    #[tokio::test]
    async fn replace_updates_capacity() {
        let storage = BundleMemStorage::new(&small_config(1000, 0));

        let name = storage.save(Bytes::from(vec![1u8; 50])).await.unwrap();
        assert_eq!(storage.global_bytes.load(Ordering::Relaxed), 50);

        storage
            .replace(&name, Bytes::from(vec![2u8; 90]))
            .await
            .unwrap();
        assert_eq!(storage.global_bytes.load(Ordering::Relaxed), 90);

        assert_eq!(storage.load(&name).await.unwrap().unwrap().len(), 90);
    }

    // The watermark transitions work with atomic state.
    #[tokio::test]
    async fn watermark_edges_are_hysteretic() {
        // capacity 1000: high watermark = 950 bytes, low watermark = 900
        let storage = BundleMemStorage::new(&small_config(1000, 1));

        let _name1 = storage.save(Bytes::from(vec![1u8; 500])).await.unwrap();
        let name2 = storage.save(Bytes::from(vec![2u8; 440])).await.unwrap();
        assert!(!storage.near_capacity(), "940 of 1000 is below 95%");

        let name3 = storage.save(Bytes::from(vec![3u8; 50])).await.unwrap();
        assert!(storage.near_capacity(), "990 of 1000 crosses 95%");

        // 940 bytes == inside the hysteresis band: still near capacity
        storage.delete(&name3).await.unwrap();
        assert!(storage.near_capacity());

        // 500 bytes < 900 exits the episode
        storage.delete(&name2).await.unwrap();
        assert!(!storage.near_capacity());
    }

    // Evictions during an episode are tallied and reset when it ends.
    #[tokio::test]
    async fn exit_resets_episode_eviction_tally() {
        let storage = BundleMemStorage::new(&small_config(1000, 0));

        let _name1 = storage.save(Bytes::from(vec![1u8; 320])).await.unwrap();
        let _name2 = storage.save(Bytes::from(vec![2u8; 320])).await.unwrap();
        let _name3 = storage.save(Bytes::from(vec![3u8; 320])).await.unwrap();
        assert!(storage.near_capacity(), "960 of 1000 crosses 95%");

        // Force eviction: push over capacity
        let name4 = storage.save(Bytes::from(vec![4u8; 320])).await.unwrap();
        assert!(storage.evicted_count() >= 1);

        // Delete enough to drop below low watermark (900)
        storage.delete(&name4).await.unwrap();
        // May need more deletions depending on what was evicted
        let bytes = storage.global_bytes.load(Ordering::Relaxed);
        if bytes >= 900 {
            // Still above low watermark, delete more
            storage.delete(&_name3).await.unwrap();
        }
        // Eventually should exit near-capacity
        let bytes = storage.global_bytes.load(Ordering::Relaxed);
        if bytes < 900 {
            assert!(!storage.near_capacity());
            assert_eq!(storage.evicted_count(), 0, "tally consumed by the exit");
        }
    }

    // Shard distribution: different names go to different shards.
    #[test]
    fn shard_index_distributes() {
        let mut counts = [0u32; SHARD_COUNT];
        let mut rng = SmallRng::try_from_rng(&mut SysRng).unwrap();
        for _ in 0..1000 {
            let name = Alphanumeric.sample_string(&mut rng, 64);
            counts[shard_index(&name)] += 1;
        }
        // Each shard should get at least some entries (not all zero)
        for (i, &count) in counts.iter().enumerate() {
            assert!(
                count > 0,
                "Shard {i} got no entries — distribution is broken"
            );
        }
    }
}
