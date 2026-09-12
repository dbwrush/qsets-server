use std::{collections::HashMap, sync::Arc};

use sqlx::PgPool;
use tokio::sync::{RwLock, Semaphore};

use crate::services::generator::QuestionRecord;

const MAX_CACHED_POOLS: usize = 16;

#[derive(Default)]
pub struct ParsedPoolCache {
    pools: HashMap<i32, Arc<Vec<QuestionRecord>>>,
}

impl ParsedPoolCache {
    pub fn get(&self, pool_id: i32) -> Option<Arc<Vec<QuestionRecord>>> {
        self.pools.get(&pool_id).cloned()
    }

    pub fn insert(&mut self, pool_id: i32, parsed: Arc<Vec<QuestionRecord>>) {
        if self.pools.len() >= MAX_CACHED_POOLS && !self.pools.contains_key(&pool_id) {
            if let Some(oldest_id) = self.pools.keys().next().copied() {
                self.pools.remove(&oldest_id);
            }
        }
        self.pools.insert(pool_id, parsed);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_replaces_existing_pool_without_growing() {
        let mut cache = ParsedPoolCache::default();
        cache.insert(1, Arc::new(Vec::new()));
        cache.insert(
            1,
            Arc::new(vec![QuestionRecord {
                question: "updated".to_string(),
                qtype: "General".to_string(),
                reference: "John 1:1".to_string(),
                answer: "answer".to_string(),
            }]),
        );

        assert_eq!(cache.get(1).unwrap()[0].question, "updated");
    }

    #[test]
    fn cache_evicts_when_capacity_is_reached() {
        let mut cache = ParsedPoolCache::default();
        for pool_id in 1..=17 {
            cache.insert(pool_id, Arc::new(Vec::new()));
        }

        assert_eq!(cache.pools.len(), MAX_CACHED_POOLS);
        assert!(cache.get(17).is_some());
    }
}
