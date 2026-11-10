//! 恢复后外部核对的本地读取与隔离写入。
//!
//! 读取只返回冻结的发送预期、已知入站邮件标识与论坛游标；没有当前 fence 冻结尝试的
//! pending/claimed Send 投递改用恢复库 payload 重建世界/收件人/资产身份。两种情况都不展开
//! 正文、Cookie 或凭据。
//! 隔离在单事务内重新锁定投递、核对 fence 与当前发送身份，然后把 pending/claimed 改为
//! `manual_reconcile` 并追加隔离事实与审计；无法唯一归属 Sent 的未决投递只做本地阻塞。
//! 绝不重发、领取或改写资金/结算事实。

use std::collections::BTreeSet;

use auction_application::core::ApplicationError;
use auction_application::delivery::DeliverySendExpectation;
use auction_application::recovery::{
    EXTERNAL_RECOVERY_RECEIPT_FORMAT, ExternalRecoveryDeliveryExpectation,
    ExternalRecoveryForumThread, ExternalRecoveryQuarantineMatch,
    ExternalRecoveryQuarantineOutcome, ExternalRecoverySendEvidence, ExternalRecoveryStore,
    ExternalRecoveryUnresolvedBlock, MAX_EXTERNAL_RECOVERY_DELIVERIES,
};
use chrono::Utc;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::{TrackedPool, storage_error, storage_error_from};

/// 恢复核对隔离的审计动作；只表示本地状态收口，不证明发送、领取或收款。
pub const EXTERNAL_RECOVERY_QUARANTINE_ACTION: &str = "delivery.external_recovery_quarantine";
/// 未决投递阻塞的审计动作；没有唯一外部证据，只是本地人工对账标记。
pub const EXTERNAL_RECOVERY_BLOCK_ACTION: &str = "delivery.external_recovery_block";
/// 冻结发送身份与 Sent 精确匹配后的失败码；Admin 仍可按既有 manual_reconcile 动作人工处置。
pub const EXTERNAL_RECOVERY_QUARANTINE_FAILURE_CODE: &str = "external_recovery_sent_match";
/// 只用恢复库 payload 身份匹配到唯一 Sent 详情时的失败码；不声称标题或正文证明。
pub const EXTERNAL_RECOVERY_PAYLOAD_QUARANTINE_FAILURE_CODE: &str =
    "external_recovery_payload_match";
/// 无法唯一归属 Sent 时显式阻塞的失败码。
pub const EXTERNAL_RECOVERY_UNRESOLVED_FAILURE_CODE: &str = "external_recovery_unresolved_send";

mod quarantine;

use quarantine::{block_unresolved_one, quarantine_one};

impl ExternalRecoveryStore for DatabaseStore {
    fn pending_claimed_send_expectations(
        &self,
    ) -> auction_application::recovery::ExternalRecoveryFuture<
        '_,
        Vec<ExternalRecoveryDeliveryExpectation>,
    > {
        Box::pin(async move {
            let store = self;
            read_expectations(store.pool()).await
        })
    }

    fn known_inbound_mail_ids(
        &self,
        candidates: Vec<(String, String)>,
    ) -> auction_application::recovery::ExternalRecoveryFuture<'_, Vec<(String, String)>> {
        Box::pin(async move {
            if candidates.len() > MAX_EXTERNAL_RECOVERY_DELIVERIES * 4 {
                return Err(storage_error("已知入站邮件核对候选过多"));
            }
            let mut worlds = Vec::with_capacity(candidates.len());
            let mut mail_ids = Vec::with_capacity(candidates.len());
            for (world, mail_id) in &candidates {
                if !matches!(world.as_str(), "main" | "isekai")
                    || !auction_domain::valid_positive_decimal(mail_id, 64)
                {
                    return Err(storage_error("已知入站邮件核对候选无效"));
                }
                worlds.push(world.clone());
                mail_ids.push(mail_id.clone());
            }
            let store = self;
            let rows = sqlx::query(
                "WITH wanted(world_id, external_mail_id) AS (
                     SELECT * FROM unnest($1::text[], $2::text[])
                 )
                 SELECT DISTINCT wanted.world_id, wanted.external_mail_id
                 FROM wanted
                 WHERE EXISTS (
                         SELECT 1 FROM public.inbound_mail AS mail
                         WHERE mail.world_id = wanted.world_id
                           AND mail.external_mail_id = wanted.external_mail_id)
                    OR EXISTS (
                         SELECT 1 FROM public.mail_poll_pending_reads AS pending
                         WHERE pending.world_id = wanted.world_id
                           AND pending.external_mail_id = wanted.external_mail_id)
                    OR EXISTS (
                         SELECT 1
                         FROM public.auction_submissions AS submission
                         JOIN public.auctions AS auction ON auction.id = submission.auction_id
                         WHERE auction.world_id = wanted.world_id
                           AND submission.external_mail_id = wanted.external_mail_id)",
            )
            .bind(&worlds)
            .bind(&mail_ids)
            .fetch_all(store.pool())
            .await
            .map_err(|error| storage_error_from(error, "读取已知入站邮件失败"))?;
            let mut known = Vec::with_capacity(rows.len());
            for row in rows {
                let world: String = row
                    .try_get("world_id")
                    .map_err(|error| storage_error_from(error, "读取已知入站邮件世界失败"))?;
                let mail_id: String = row
                    .try_get("external_mail_id")
                    .map_err(|error| storage_error_from(error, "读取已知入站邮件标识失败"))?;
                known.push((world, mail_id));
            }
            Ok(known)
        })
    }

    fn published_forum_threads(
        &self,
        limit: u16,
    ) -> auction_application::recovery::ExternalRecoveryFuture<'_, Vec<ExternalRecoveryForumThread>>
    {
        Box::pin(async move {
            if limit == 0
                || limit > auction_application::recovery::MAX_EXTERNAL_RECOVERY_FORUM_THREADS
            {
                return Err(storage_error("论坛恢复核对主题上限无效"));
            }
            let store = self;
            let rows = sqlx::query(
                "SELECT sync.auction_id, auction.world_id, sync.thread_id,
                        sync.processed_through_post_number
                 FROM public.auction_forum_sync AS sync
                 JOIN public.auctions AS auction ON auction.id = sync.auction_id
                 WHERE sync.status = 'published'
                   AND sync.thread_id IS NOT NULL
                 ORDER BY sync.updated_at DESC, sync.auction_id
                 LIMIT $1",
            )
            .bind(i64::from(limit))
            .fetch_all(store.pool())
            .await
            .map_err(|error| storage_error_from(error, "读取已发布论坛主题失败"))?;
            let mut threads = Vec::with_capacity(rows.len());
            for row in rows {
                let thread = ExternalRecoveryForumThread {
                    auction_id: row
                        .try_get("auction_id")
                        .map_err(|error| storage_error_from(error, "读取论坛主题拍卖标识失败"))?,
                    world_id: row
                        .try_get("world_id")
                        .map_err(|error| storage_error_from(error, "读取论坛主题世界失败"))?,
                    thread_id: row
                        .try_get("thread_id")
                        .map_err(|error| storage_error_from(error, "读取论坛主题标识失败"))?,
                    after_post_number: u32::try_from(
                        row.try_get::<i32, _>("processed_through_post_number")
                            .map_err(|error| storage_error_from(error, "读取论坛已处理楼层失败"))?,
                    )
                    .map_err(|_: std::num::TryFromIntError| {
                        storage_error("论坛已处理楼层超出范围")
                    })?,
                };
                if !thread.validate() {
                    return Err(storage_error("已发布论坛主题投影无效"));
                }
                threads.push(thread);
            }
            Ok(threads)
        })
    }

    fn quarantine_external_sent_matches(
        &self,
        scan_id: Uuid,
        actor_id: String,
        matches: Vec<ExternalRecoveryQuarantineMatch>,
    ) -> auction_application::recovery::ExternalRecoveryFuture<
        '_,
        Vec<ExternalRecoveryQuarantineOutcome>,
    > {
        Box::pin(async move {
            if scan_id.is_nil() {
                return Err(storage_error("恢复扫描标识不能为空"));
            }
            if actor_id.is_empty()
                || actor_id.len() > 128
                || actor_id != actor_id.trim()
                || actor_id.chars().any(char::is_control)
            {
                return Err(storage_error("恢复核对操作者标识无效"));
            }
            if matches.len() > MAX_EXTERNAL_RECOVERY_DELIVERIES {
                return Err(storage_error("恢复核对隔离批次过大"));
            }
            if matches.iter().any(|m| !m.validate()) {
                return Err(storage_error("恢复核对隔离匹配无效"));
            }
            let mut seen = BTreeSet::new();
            if matches.iter().any(|m| !seen.insert((m.delivery_id, m.fencing_version))) {
                return Err(storage_error("恢复核对隔离匹配重复"));
            }
            let store = self;
            let now = Utc::now();
            let mut transaction = store
                .pool()
                .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
                .await
                .map_err(|error| storage_error_from(error, "开启恢复隔离事务失败"))?;
            let mut outcomes = Vec::with_capacity(matches.len());
            for candidate in matches {
                let outcome =
                    quarantine_one(&mut transaction, scan_id, &actor_id, &candidate, now).await?;
                outcomes.push(outcome);
            }
            transaction
                .commit()
                .await
                .map_err(|error| storage_error_from(error, "提交恢复隔离事务失败"))?;
            Ok(outcomes)
        })
    }

    fn block_unresolved_send_deliveries(
        &self,
        scan_id: Uuid,
        actor_id: String,
        blocks: Vec<ExternalRecoveryUnresolvedBlock>,
    ) -> auction_application::recovery::ExternalRecoveryFuture<
        '_,
        Vec<ExternalRecoveryQuarantineOutcome>,
    > {
        Box::pin(async move {
            if scan_id.is_nil() {
                return Err(storage_error("恢复扫描标识不能为空"));
            }
            if actor_id.is_empty()
                || actor_id.len() > 128
                || actor_id != actor_id.trim()
                || actor_id.chars().any(char::is_control)
            {
                return Err(storage_error("恢复核对操作者标识无效"));
            }
            if blocks.len() > MAX_EXTERNAL_RECOVERY_DELIVERIES {
                return Err(storage_error("恢复核对阻塞批次过大"));
            }
            if blocks.iter().any(|block| !block.validate()) {
                return Err(storage_error("恢复核对未决阻塞无效"));
            }
            let mut seen = BTreeSet::new();
            if blocks.iter().any(|block| !seen.insert((block.delivery_id, block.fencing_version))) {
                return Err(storage_error("恢复核对未决阻塞重复"));
            }
            let store = self;
            let now = Utc::now();
            let mut transaction = store
                .pool()
                .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
                .await
                .map_err(|error| storage_error_from(error, "开启恢复阻塞事务失败"))?;
            let mut outcomes = Vec::with_capacity(blocks.len());
            for block in blocks {
                let outcome =
                    block_unresolved_one(&mut transaction, scan_id, &actor_id, &block, now).await?;
                outcomes.push(outcome);
            }
            transaction
                .commit()
                .await
                .map_err(|error| storage_error_from(error, "提交恢复阻塞事务失败"))?;
            Ok(outcomes)
        })
    }
}

async fn read_expectations(
    pool: &TrackedPool<Postgres>,
) -> Result<Vec<ExternalRecoveryDeliveryExpectation>, ApplicationError> {
    let sql = expectation_sql("ORDER BY delivery.created_at, delivery.id LIMIT $1");
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(i64::try_from(MAX_EXTERNAL_RECOVERY_DELIVERIES).unwrap_or(i64::MAX))
        .fetch_all(pool)
        .await
        .map_err(|error| storage_error_from(error, "读取待核对投递发送身份失败"))?;
    rows.iter().map(expectation_from_row).collect()
}

/// 单条投递的恢复身份；调用方必须已在同一事务内锁定该投递。
pub(super) async fn load_expectation(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<Option<ExternalRecoveryDeliveryExpectation>, ApplicationError> {
    let sql = expectation_sql("AND delivery.id = $1");
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(delivery_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(|error| storage_error_from(error, "读取投递恢复发送身份失败"))?;
    row.as_ref().map(expectation_from_row).transpose()
}

/// pending/claimed Send 投递的恢复身份查询；批量读取与单条复核共用同一语句。
///
/// 当前 fence 存在冻结发送尝试时优先冻结身份；恢复库没有这次 `sending` 写入时（备份点仍是
/// pending/claimed，旧 Worker 在快照之后才进入 sending），用恢复库自己的 payload 重建世界、
/// 收件人、金额与装备身份。payload 重建身份没有渲染标题与正文摘要，不是精确证明。
fn expectation_sql(tail: &str) -> String {
    format!(
        r#"SELECT delivery.id,
                  delivery.fencing_version,
                  delivery.recipient,
                  COALESCE(delivery.payload_world, 'main') AS world_id,
                  attempt.expectation AS frozen_expectation,
                  COALESCE((delivery.payload ->> 'credits')::bigint, 0) AS payload_credits,
                  COALESCE((delivery.payload ->> 'cod')::bigint, 0) AS payload_cod,
                  CASE
                      WHEN delivery.origin_kind = 'settlement' THEN (
                          SELECT CASE
                              WHEN count(equipment.id) = jsonb_array_length(
                                       COALESCE(delivery.payload -> 'equipment_secret_refs',
                                                '[]'::jsonb))
                              THEN COALESCE(
                                  jsonb_agg(equipment.external_equipment_id
                                            ORDER BY equipment.external_equipment_id)
                                  FILTER (WHERE equipment.id IS NOT NULL), '[]'::jsonb)
                              ELSE 'null'::jsonb END
                          FROM jsonb_array_elements(
                                   COALESCE(delivery.payload -> 'equipment_secret_refs',
                                            '[]'::jsonb)) AS reference
                          LEFT JOIN public.equipment AS equipment
                              ON equipment.id::text = reference ->> 'id'
                             AND reference ->> 'kind' = 'auction_equipment_key'
                             AND equipment.world_id = COALESCE(delivery.payload_world, 'main')
                      )
                      ELSE COALESCE(delivery.payload -> 'attachment_ids', '[]'::jsonb)
                  END AS payload_equipment_ids
           FROM public.deliveries AS delivery
           LEFT JOIN LATERAL public.delivery_current_send_attempt(delivery.id) AS attempt ON true
           WHERE delivery.status IN ('pending', 'claimed')
             AND delivery.payload_operation = 'send'
           {tail}"#
    )
}

/// 单行恢复身份；payload 无法完整重建资产时降级为“资产未知”，不拒绝整次核对。
fn expectation_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<ExternalRecoveryDeliveryExpectation, ApplicationError> {
    let delivery_id: Uuid =
        row.try_get("id").map_err(|error| storage_error_from(error, "读取投递标识失败"))?;
    let fencing_version: i64 = row
        .try_get("fencing_version")
        .map_err(|error| storage_error_from(error, "读取投递 fence 失败"))?;
    let recipient: String = row
        .try_get("recipient")
        .map_err(|error| storage_error_from(error, "读取投递收件人失败"))?;
    let world_id: String =
        row.try_get("world_id").map_err(|error| storage_error_from(error, "读取投递世界失败"))?;
    let frozen: Option<serde_json::Value> = row
        .try_get("frozen_expectation")
        .map_err(|error| storage_error_from(error, "读取投递发送预期失败"))?;
    let (evidence, subject, credits, cod, equipment_ids) = match frozen {
        Some(value) => {
            let expectation: DeliverySendExpectation = serde_json::from_value(value)
                .map_err(|_: serde_json::Error| storage_error("投递发送预期格式无效"))?;
            if !expectation.validate() {
                return Err(storage_error("投递发送预期违反发送不变量"));
            }
            if !recipient.eq_ignore_ascii_case(&expectation.recipient)
                || expectation.world_id != world_id
            {
                return Err(storage_error("投递收件人或世界与冻结发送预期不一致"));
            }
            (
                ExternalRecoverySendEvidence::Frozen,
                Some(expectation.subject),
                expectation.credits,
                expectation.cod,
                Some(expectation.equipment_ids),
            )
        }
        None => {
            let payload_credits: i64 = row
                .try_get("payload_credits")
                .map_err(|error| storage_error_from(error, "读取投递恢复金额失败"))?;
            let payload_cod: i64 = row
                .try_get("payload_cod")
                .map_err(|error| storage_error_from(error, "读取投递恢复 COD 失败"))?;
            let payload_equipment: serde_json::Value = row
                .try_get("payload_equipment_ids")
                .map_err(|error| storage_error_from(error, "读取投递恢复装备身份失败"))?;
            (
                ExternalRecoverySendEvidence::RestoredPayload,
                None,
                payload_credits,
                payload_cod,
                serde_json::from_value::<Vec<String>>(payload_equipment).ok(),
            )
        }
    };
    let expectation = ExternalRecoveryDeliveryExpectation {
        delivery_id,
        fencing_version,
        evidence,
        world_id,
        recipient,
        subject,
        credits,
        cod,
        equipment_ids,
    };
    if expectation.validate() {
        return Ok(expectation);
    }
    // 装备引用无法完整解析时只失去资产区分能力：仍然阻塞，而不是继续可领取。
    if expectation.evidence == ExternalRecoverySendEvidence::RestoredPayload
        && expectation.equipment_ids.is_some()
    {
        let degraded = ExternalRecoveryDeliveryExpectation { equipment_ids: None, ..expectation };
        if degraded.validate() {
            return Ok(degraded);
        }
    }
    Err(storage_error("投递恢复发送身份违反发送不变量"))
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{
        ExternalRecoveryDeliveryExpectation, ExternalRecoverySendEvidence, expectation_sql,
    };

    /// 恢复库没有冻结尝试是常态（claim 之后、sending 之前都在当前 fence 没有 attempt）。
    /// 这种投递必须仍然产出候选，不能把整次核对打回失败。
    #[test]
    fn restored_payload_expectation_is_accepted_without_attempt() {
        let expectation = ExternalRecoveryDeliveryExpectation {
            delivery_id: Uuid::from_u128(1),
            fencing_version: 3,
            evidence: ExternalRecoverySendEvidence::RestoredPayload,
            world_id: "main".to_owned(),
            recipient: "Restia﹒Ashdoll".to_owned(),
            subject: None,
            credits: 300_000,
            cod: 0,
            equipment_ids: Some(vec!["317632875".to_owned()]),
        };
        assert!(expectation.validate());
    }

    #[test]
    fn expectation_sql_does_not_require_a_sending_attempt() {
        let sql = expectation_sql("ORDER BY delivery.created_at, delivery.id LIMIT $1");
        assert!(sql.contains("delivery.status IN ('pending', 'claimed')"));
        assert!(sql.contains("delivery.payload_operation = 'send'"));
        // 备份点之后才进入 sending 的投递在恢复库没有 attempt 行；内连接会整体漏检。
        assert!(!sql.contains("JOIN public.delivery_send_attempts"));
        assert!(sql.contains("payload_equipment_ids"));
    }
}
