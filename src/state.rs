use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::{RwLock, Semaphore};

use crate::services::{generator::QuestionRecord, security::LoginThrottle};

const MAX_CACHED_POOLS: usize = 16;

struct CachedPool {
    /// The pool's `updated_at` when it was parsed; a replaced pool no longer matches.
    version: DateTime<Utc>,
    parsed: Arc<Vec<QuestionRecord>>,
    last_used: AtomicU64,
}

/// Parsed pools keyed by id and version. Lookups only need a shared reference so callers can
/// hold a read lock; recency is tracked atomically so eviction drops the least recently used pool.
///
/// Keying on the version means a request that parsed the old CSV while the pool was being
/// replaced cannot leave stale questions behind: its entry simply never matches again.
#[derive(Default)]
pub struct ParsedPoolCache {
    pools: HashMap<i32, CachedPool>,
    clock: AtomicU64,
}

impl ParsedPoolCache {
    fn tick(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::Relaxed)
    }

    pub fn get(&self, pool_id: i32, version: DateTime<Utc>) -> Option<Arc<Vec<QuestionRecord>>> {
        let entry = self.pools.get(&pool_id).filter(|e| e.version == version)?;
        entry.last_used.store(self.tick(), Ordering::Relaxed);
        Some(entry.parsed.clone())
    }

    pub fn insert(
        &mut self,
        pool_id: i32,
        version: DateTime<Utc>,
        parsed: Arc<Vec<QuestionRecord>>,
    ) {
        if self.pools.len() >= MAX_CACHED_POOLS && !self.pools.contains_key(&pool_id) {
            let least_recent = self
                .pools
                .iter()
                .min_by_key(|(_, entry)| entry.last_used.load(Ordering::Relaxed))
                .map(|(id, _)| *id);
            if let Some(id) = least_recent {
                self.pools.remove(&id);
            }
        }
        let last_used = AtomicU64::new(self.tick());
        self.pools.insert(
            pool_id,
            CachedPool {
                version,
                parsed,
                last_used,
            },
        );
    }

    pub fn remove(&mut self, pool_id: i32) {
        self.pools.remove(&pool_id);
    }
}

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub parsed_pools: Arc<RwLock<ParsedPoolCache>>,
    pub generation_slots: Arc<Semaphore>,
    /// Whether to believe `X-Forwarded-For`. Only enable behind a reverse proxy that sets it.
    pub trust_proxy: bool,
    pub login_throttle: Arc<LoginThrottle>,
}

impl AppState {
    pub fn new(pool: PgPool, generation_concurrency: usize, trust_proxy: bool) -> Self {
        Self {
            pool,
            parsed_pools: Arc::default(),
            generation_slots: Arc::new(Semaphore::new(generation_concurrency)),
            trust_proxy,
            login_throttle: Arc::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn v(n: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(n, 0).unwrap()
    }

    fn pool_with(question: &str) -> Arc<Vec<QuestionRecord>> {
        Arc::new(vec![QuestionRecord {
            question: question.to_string(),
            qtype: "General".to_string(),
            reference: "John 1:1".to_string(),
            answer: "answer".to_string(),
        }])
    }

    #[test]
    fn cache_replaces_existing_pool_without_growing() {
        let mut cache = ParsedPoolCache::default();
        cache.insert(1, v(1), Arc::new(Vec::new()));
        cache.insert(1, v(2), pool_with("updated"));

        assert_eq!(cache.pools.len(), 1);
        assert_eq!(cache.get(1, v(2)).unwrap()[0].question, "updated");
    }

    #[test]
    fn cache_misses_when_the_pool_version_changed() {
        let mut cache = ParsedPoolCache::default();
        cache.insert(1, v(1), pool_with("old"));

        assert!(cache.get(1, v(2)).is_none());
        assert!(cache.get(1, v(1)).is_some());
    }

    #[test]
    fn cache_evicts_when_capacity_is_reached() {
        let mut cache = ParsedPoolCache::default();
        for pool_id in 1..=17 {
            cache.insert(pool_id, v(0), Arc::new(Vec::new()));
        }

        assert_eq!(cache.pools.len(), MAX_CACHED_POOLS);
        assert!(cache.get(17, v(0)).is_some());
    }

    #[test]
    fn cache_evicts_least_recently_used_pool() {
        let mut cache = ParsedPoolCache::default();
        for pool_id in 1..=16 {
            cache.insert(pool_id, v(0), Arc::new(Vec::new()));
        }
        // Touch the oldest entry so pool 2 becomes the least recently used.
        assert!(cache.get(1, v(0)).is_some());
        cache.insert(17, v(0), Arc::new(Vec::new()));

        assert!(cache.get(1, v(0)).is_some());
        assert!(cache.get(2, v(0)).is_none());
        assert!(cache.get(17, v(0)).is_some());
    }
}
