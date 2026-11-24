//! 拍卖准入的只读门控查询：寄售强占用与既有投稿续办。
//!
//! 这些查询只用于决定是否进入带实物锁的写事务，不构成授权；锁内强占用检查与 lot 守卫
//! 仍然保留。迁移测试会用当前二进制访问 000134 之前的库，缺少 claims 表时保持既有行为。

use auction_application::core::CompetitionTransactionError;
use auction_domain::consignment::ListingEquipmentIdentity;

use super::PostgresCompetitionTransaction;
use super::map_sql_error;

/// 这些规范实物中是否至少一件被寄售预约或 position 持有。
pub(super) async fn any_equipment_held(
    transaction: &mut PostgresCompetitionTransaction,
    equipment: &[ListingEquipmentIdentity],
) -> Result<bool, CompetitionTransactionError> {
    if equipment.is_empty() {
        return Ok(false);
    }
    let transaction = transaction.transaction_mut()?;
    let schema_available: bool =
        sqlx::query_scalar("SELECT to_regclass('public.consignment_equipment_claims') IS NOT NULL")
            .fetch_one(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
    if !schema_available {
        return Ok(false);
    }
    let mut worlds = Vec::with_capacity(equipment.len());
    let mut canonical_eids = Vec::with_capacity(equipment.len());
    for identity in equipment {
        worlds.push(identity.world_id().to_owned());
        canonical_eids.push(identity.external_equipment_id().to_owned());
    }
    sqlx::query_scalar(
        r#"SELECT EXISTS (
               SELECT 1
               FROM public.consignment_equipment_claims AS claim
               INNER JOIN unnest($1::text[], $2::text[])
                   AS wanted(world_id, canonical_eid)
                   ON claim.world_id = wanted.world_id
                  AND claim.canonical_eid = wanted.canonical_eid
               WHERE claim.reservation_id IS NOT NULL
                  OR claim.position_id IS NOT NULL
           )"#,
    )
    .bind(worlds)
    .bind(canonical_eids)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)
}

/// 同一原信键是否已有拍卖投稿。勘误 E1 要求该判定先于实物锁。
pub(super) async fn mail_auction_submission_exists(
    transaction: &mut PostgresCompetitionTransaction,
    mailbox_key: &str,
    external_mail_id: &str,
) -> Result<bool, CompetitionTransactionError> {
    let transaction = transaction.transaction_mut()?;
    sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1
             FROM public.auction_submissions
             WHERE mailbox_key = $1 AND external_mail_id = $2
         )",
    )
    .bind(mailbox_key)
    .bind(external_mail_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)
}
