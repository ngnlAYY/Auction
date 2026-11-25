//! 寄售尝试的 Sent MID 绑定：与拍卖共用 `delivery_sent_bindings`，一个 world/MID 只能证明一个投递。
//!
//! 保护不变量 4、6：跨来源唯一，寄售尝试绑定的 MID 若已被拍卖投递（或另一寄售尝试）占用，
//! 或旧数据里已有同一外部引用，则绑定失败，调用方转证据冲突，不得覆盖旧绑定。

use sqlx::Postgres;
use uuid::Uuid;

use crate::database::TrackedTransaction;
use crate::database::delivery::scheduler::attempts::bind_sent;

/// 在调用方事务内绑定；返回 `false` 表示该 MID 已属于别的投递或 fence。
///
/// 复用拍卖的 `bind_sent`：先排除旧数据的外部引用碰撞，再 `ON CONFLICT DO NOTHING` 插入，
/// 最后读回确认绑定的恰是本尝试，重放同一绑定不产生第二条事实。
pub(super) async fn bind_dispatch_sent(
    tx: &mut TrackedTransaction<'_, Postgres>,
    delivery_id: Uuid,
    fence: i64,
    world_id: &str,
    external_mail_id: &str,
) -> Result<bool, sqlx::Error> {
    bind_sent(tx, delivery_id, fence, world_id, external_mail_id, true).await
}
