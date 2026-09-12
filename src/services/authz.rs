use axum_extra::extract::cookie::CookieJar;
use sqlx::PgPool;

use crate::services::auth;

/// Rank granted to anonymous visitors; matches the seeded `public` tier.
pub const ANONYMOUS_TIER_RANK: i32 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Actor {
    pub user_id: Option<i32>,
    pub tier_rank: i32,
    pub is_admin: bool,
    pub can_upload_pools: bool,
}

impl Actor {
    pub fn anonymous() -> Self {
        Self {
            user_id: None,
            tier_rank: ANONYMOUS_TIER_RANK,
            is_admin: false,
            can_upload_pools: false,
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PoolAccess {
    pub id: i32,
    pub name: String,
    pub tier_id: i32,
    pub tier_name: String,
    pub tier_rank: i32,
}

/// A pool is readable when its tier sits at or below the actor's tier.
pub fn can_access_pool(actor: &Actor, pool_tier_rank: i32) -> bool {
    actor.is_admin || pool_tier_rank <= actor.tier_rank
}

/// Uploading requires the explicit permission; the chosen tier may not exceed the uploader's own.
pub fn can_assign_pool_tier(actor: &Actor, target_tier_rank: i32) -> bool {
    if actor.is_admin {
        return true;
    }
    actor.can_upload_pools && target_tier_rank <= actor.tier_rank
}

pub fn can_upload(actor: &Actor) -> bool {
    actor.is_admin || actor.can_upload_pools
}

/// Pools may be removed by an admin or by the uploader who created them.
pub fn can_delete_pool(actor: &Actor, pool_created_by: Option<i32>) -> bool {
    if actor.is_admin {
        return true;
    }
    match (actor.user_id, pool_created_by) {
        (Some(actor_id), Some(owner_id)) => actor.can_upload_pools && actor_id == owner_id,
        _ => false,
    }
}

pub async fn load_actor(pool: &PgPool, jar: &CookieJar) -> Actor {
    let Some(user_id) = auth::get_user_id_from_jar(pool, jar).await else {
        return Actor::anonymous();
    };

    let row = sqlx::query_as::<_, (bool, bool, i32)>(
        "SELECT u.is_admin, u.can_upload_pools, t.rank
         FROM users u
         JOIN tiers t ON u.tier_id = t.id
         WHERE u.id = $1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await;

    match row {
        Ok(Some((is_admin, can_upload_pools, tier_rank))) => Actor {
            user_id: Some(user_id),
            tier_rank,
            is_admin,
            can_upload_pools,
        },
        _ => Actor::anonymous(),
    }
}

pub async fn load_pool_access(
    pool: &PgPool,
    pool_id: i32,
) -> Result<Option<PoolAccess>, sqlx::Error> {
    sqlx::query_as::<_, PoolAccess>(
        "SELECT qp.id, qp.name, qp.tier_id, t.name AS tier_name, t.rank AS tier_rank
         FROM question_pools qp
         JOIN tiers t ON qp.tier_id = t.id
         WHERE qp.id = $1",
    )
    .bind(pool_id)
    .fetch_optional(pool)
    .await
}

pub async fn tier_rank(pool: &PgPool, tier_id: i32) -> Result<Option<i32>, sqlx::Error> {
    sqlx::query_scalar::<_, i32>("SELECT rank FROM tiers WHERE id = $1")
        .bind(tier_id)
        .fetch_optional(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(tier_rank: i32, is_admin: bool, can_upload_pools: bool) -> Actor {
        Actor {
            user_id: Some(1),
            tier_rank,
            is_admin,
            can_upload_pools,
        }
    }

    #[test]
    fn anonymous_actor_has_public_rank_and_no_privileges() {
        let anon = Actor::anonymous();
        assert_eq!(anon.tier_rank, ANONYMOUS_TIER_RANK);
        assert!(!anon.is_admin);
        assert!(!anon.can_upload_pools);
        assert!(anon.user_id.is_none());
    }

    #[test]
    fn anonymous_can_read_public_tier_only() {
        let anon = Actor::anonymous();
        assert!(can_access_pool(&anon, 0));
        assert!(!can_access_pool(&anon, 1));
        assert!(!can_access_pool(&anon, 3));
    }

    #[test]
    fn user_can_read_own_tier_and_every_tier_below() {
        let regional = actor(2, false, false);
        assert!(can_access_pool(&regional, 0));
        assert!(can_access_pool(&regional, 1));
        assert!(can_access_pool(&regional, 2));
    }

    #[test]
    fn user_cannot_read_higher_tier() {
        let district = actor(1, false, false);
        assert!(!can_access_pool(&district, 2));
        assert!(!can_access_pool(&district, 99));
    }

    #[test]
    fn admin_can_read_any_tier() {
        let admin = actor(0, true, false);
        assert!(can_access_pool(&admin, 0));
        assert!(can_access_pool(&admin, 7));
    }

    #[test]
    fn upload_requires_explicit_permission() {
        assert!(!can_upload(&actor(5, false, false)));
        assert!(can_upload(&actor(0, false, true)));
        assert!(can_upload(&actor(0, true, false)));
    }

    #[test]
    fn uploader_cannot_assign_tier_above_own() {
        let uploader = actor(1, false, true);
        assert!(can_assign_pool_tier(&uploader, 0));
        assert!(can_assign_pool_tier(&uploader, 1));
        assert!(!can_assign_pool_tier(&uploader, 2));
    }

    #[test]
    fn user_without_upload_permission_cannot_assign_any_tier() {
        let reader = actor(5, false, false);
        assert!(!can_assign_pool_tier(&reader, 0));
        assert!(!can_assign_pool_tier(&reader, 5));
    }

    #[test]
    fn admin_can_assign_any_tier() {
        let admin = actor(0, true, false);
        assert!(can_assign_pool_tier(&admin, 42));
    }

    #[test]
    fn tier_count_is_not_fixed() {
        let top = actor(9, false, true);
        for rank in 0..=9 {
            assert!(can_access_pool(&top, rank));
            assert!(can_assign_pool_tier(&top, rank));
        }
        assert!(!can_access_pool(&top, 10));
    }

    #[test]
    fn uploader_may_delete_only_own_pool() {
        let uploader = actor(1, false, true);
        assert!(can_delete_pool(&uploader, Some(1)));
        assert!(!can_delete_pool(&uploader, Some(2)));
        assert!(!can_delete_pool(&uploader, None));
    }

    #[test]
    fn admin_may_delete_any_pool() {
        let admin = actor(0, true, false);
        assert!(can_delete_pool(&admin, Some(2)));
        assert!(can_delete_pool(&admin, None));
    }

    #[test]
    fn anonymous_may_not_delete_pools() {
        assert!(!can_delete_pool(&Actor::anonymous(), Some(1)));
    }
}
