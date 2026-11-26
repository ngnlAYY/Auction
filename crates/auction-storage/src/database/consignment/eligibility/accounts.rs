//! 在调用方事务中读取卖家资格及解析结果的当前账户归属。

use auction_domain::UserId;
use sqlx::{PgConnection, Postgres};
use uuid::Uuid;

use super::predicates::listing_sql;

/// 已锁定的当前账户及登记时应冻结的拉黑事实计数。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) struct SellerEligibility {
    /// 账户当前的状态是否为 active。
    pub(crate) active: bool,
    /// 是否存在未解禁的拉黑投影。
    pub(crate) active_blacklist: bool,
    /// 追加拉黑事实数。
    pub(crate) blacklist_count: i64,
}

/// 在卖家会话锁及 Serializable 事务中锁定账户。缺失账户返回 None。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) async fn lock_seller_eligibility(
    tx: &mut PgConnection,
    seller_user_id: UserId,
) -> Result<Option<SellerEligibility>, sqlx::Error> {
    // 不变量 2、6：与既有拉黑写入口在相同账户行上串行，历史和当前资格同快照。
    let row = sqlx::query_as::<Postgres, (bool, bool, i64)>(concat!(
        "SELECT ",
        listing_sql!(seller_active),
        ", ",
        listing_sql!(active_blacklist),
        ", ",
        listing_sql!(blacklist_count),
        " FROM public.users s WHERE s.id = $1 FOR UPDATE OF s"
    ))
    .bind(seller_user_id.as_uuid())
    .fetch_optional(tx)
    .await?;
    Ok(row.map(|(active, active_blacklist, blacklist_count)| SellerEligibility {
        active,
        active_blacklist,
        blacklist_count,
    }))
}

/// 在卖家会话锁内锁定与自填游戏名 lower 等值的本站账户（`users` 的 lower 唯一索引保证至多一个）。
pub(crate) async fn lock_same_name_account(
    tx: &mut PgConnection,
    seller_game_name: &str,
) -> Result<Option<(UserId, SellerEligibility)>, sqlx::Error> {
    // 不变量 2、6：与拉黑写入口在同一账户行上串行，匿名登记看到的拉黑事实与冻结基线同快照。
    let row = sqlx::query_as::<Postgres, (Uuid, bool, bool, i64)>(concat!(
        "SELECT s.id, ",
        listing_sql!(seller_active),
        ", ",
        listing_sql!(active_blacklist),
        ", ",
        listing_sql!(blacklist_count),
        " FROM public.users s WHERE lower(s.username) = lower($1) FOR UPDATE OF s"
    ))
    .bind(seller_game_name)
    .fetch_optional(tx)
    .await?;
    Ok(row.map(|(user_id, active, active_blacklist, blacklist_count)| {
        (
            UserId::from_uuid(user_id),
            SellerEligibility { active, active_blacklist, blacklist_count },
        )
    }))
}

/// 锁定挂单卖家的当前资格。匿名挂单只受同名账户的拉黑约束（维护者决定），账户停用不影响；
/// 没有同名账户时视为合格且拉黑计数为零。
/// 参数取自挂单锁键：账户挂单传账户 ID 与空名称，匿名挂单传自填名。
pub(crate) async fn lock_listing_seller(
    tx: &mut PgConnection,
    seller_user_id: Uuid,
    seller_game_name: Option<&str>,
) -> Result<Option<SellerEligibility>, sqlx::Error> {
    let Some(name) = seller_game_name else {
        return lock_seller_eligibility(tx, UserId::from_uuid(seller_user_id)).await;
    };
    let same_name = lock_same_name_account(tx, name).await?;
    Ok(Some(same_name.map_or(
        SellerEligibility { active: true, active_blacklist: false, blacklist_count: 0 },
        |(_, seller)| SellerEligibility { active: true, ..seller },
    )))
}

/// 以 PostgreSQL lower 等值比较两个名称，与 `users` 唯一索引和匿名占用索引同一口径。
pub(crate) async fn same_game_name(
    tx: &mut PgConnection,
    left: &str,
    right: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT lower($1) = lower($2)").bind(left).bind(right).fetch_one(tx).await
}

/// 当前第三方 owner 名在本站账户的匹配事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) enum OwnerMatch {
    /// 唯一命中当前卖家。
    Seller,
    /// 没有本站账户匹配。
    Missing,
    /// 唯一命中别人，不带其身份。
    Other,
    /// 多个账户匹配，失败关闭。
    Ambiguous,
}

/// 发布事务在锁定卖家后，以 PostgreSQL lower 等值核对当前持有人名称。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) async fn match_current_owner(
    tx: &mut PgConnection,
    seller_user_id: UserId,
    owner_name: &str,
) -> Result<OwnerMatch, sqlx::Error> {
    let owners = sqlx::query_scalar::<Postgres, Uuid>(
        "SELECT id FROM public.users WHERE lower(username) = lower($1) LIMIT 2",
    )
    .bind(owner_name)
    .fetch_all(tx)
    .await?;
    Ok(match owners.as_slice() {
        [] => OwnerMatch::Missing,
        [user_id] if *user_id == seller_user_id.as_uuid() => OwnerMatch::Seller,
        [_] => OwnerMatch::Other,
        _ => OwnerMatch::Ambiguous,
    })
}
