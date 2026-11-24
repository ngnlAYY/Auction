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
