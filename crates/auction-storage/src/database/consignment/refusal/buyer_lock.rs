//! 买家互斥：拒收、解禁与购买共用同一把买家用户锁；本模块校验锁已持有，并提供购买的阻断读。
//!
//! 保护不变量 2：阻断从拒收事务提交起生效，购买必须先取同一把锁、再开快照、再读阻断，
//! 因此不会在拒收提交之后仍读到旧的“无阻断”。

use auction_domain::UserId;
use sqlx::PgConnection;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::user_lock_key;

/// 校验当前事务所在连接已持有该买家的用户锁；未持有则失败关闭（不变量 2、6）。
///
/// 锁由调用方在开 Serializable 快照之前取得（`LockKeys.sellers` 含买家）；本函数只读 `pg_locks`，
/// 不取锁，避免在快照内补锁造成顺序反转。
pub(super) async fn ensure_buyer_lock_held(
    conn: &mut PgConnection,
    buyer: UserId,
) -> Result<(), WriteError> {
    let (namespace, key) = user_lock_key(buyer.as_uuid());
    let held: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM pg_catalog.pg_locks AS lock
             WHERE lock.locktype = 'advisory'
               AND lock.pid = pg_catalog.pg_backend_pid()
               AND lock.granted
               AND lock.database = (
                   SELECT database.oid FROM pg_catalog.pg_database AS database
                   WHERE database.datname = pg_catalog.current_database())
               AND lock.classid = $1::int4::oid
               AND lock.objid = $2::int4::oid
               AND lock.objsubid = 2
         )",
    )
    .bind(namespace)
    .bind(key)
    .fetch_one(conn)
    .await?;
    if held { Ok(()) } else { Err(WriteError::invariant("consignment_refusal_buyer_lock_missing")) }
}

/// 买家是否有待执行处罚（新购阻断）；购买事务在同一买家锁下重验。
///
/// 与数据库 `c6_sale_buyer_sanction` 触发器互为兜底：应用层先拒，触发器兜住漏网的写入。
pub(crate) async fn buyer_sanction_pending(
    conn: &mut PgConnection,
    buyer: UserId,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.consignment_sanctions
             WHERE buyer_user_id = $1 AND state = 'pending')",
    )
    .bind(buyer.as_uuid())
    .fetch_one(conn)
    .await
}
