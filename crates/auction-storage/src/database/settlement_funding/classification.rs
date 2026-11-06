//! 对账只读分类例外：永久修复事实或已严格解码的无资产通知，不改旧投递结果。

use std::{collections::HashSet, sync::LazyLock};

use futures_util::TryStreamExt;

use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement_funding::storage;
use auction_application::FundingError;
use sqlx::{Postgres, Row};
use uuid::Uuid;

/// 仅同UID、完整原投递身份匹配的追加修复证明解除旧结果未知门。
pub(super) async fn repaired_deliveries(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
) -> Result<Vec<Uuid>, FundingError> {
    // 只缓存固定 SQL 文本，不缓存任一 UID 的修复结果。
    static QUERY: LazyLock<String> = LazyLock::new(|| {
        "SELECT d.id FROM public.deliveries d
        JOIN public.settlement_legacy_self_reconciliations r ON r.delivery_id=d.id
        WHERE r.uid=$1 AND (/* trusted_legacy_self */) ORDER BY d.id"
            .replace(
                "/* trusted_legacy_self */",
                crate::database::legacy_self_proof::TRUSTED_DELIVERY_PREDICATE,
            )
    });
    // 替换只来自已审查的编译期常量，不拼接UID、业务载荷或管理员文本。
    sqlx::query_scalar(sqlx::AssertSqlSafe(QUERY.as_str()))
        .bind(uid)
        .fetch_all(&mut **tx)
        .await
        .map_err(storage)
}

/// 复用附件owner的闭集decoder；正文只在适配器内分类，不放资金预览或日志。
pub(super) async fn assetless_notifications(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<Vec<Uuid>, FundingError> {
    // 这四个字段在 AttachmentPayload 中均为无 default/alias 的必填字段，SQL 只筛必要条件；
    // 历史 origin_id 后缀仍由完整闭集解码后的 owner 校验，不把字段缺失当成零资产，
    // 也不把历史正文整批加载到内存。
    let mut rows = sqlx::query(
        "SELECT id,origin_id,command_id,payload FROM public.deliveries
         WHERE origin_kind='mail_intake'
           AND payload_operation='send'
           AND payload->'credits'='0'::jsonb
           AND payload->'cod'='0'::jsonb
           AND payload->'attachment_ids'='[]'::jsonb
         ORDER BY id",
    )
    .fetch(&mut **tx);
    let mut ids = Vec::new();
    while let Some(row) = rows.try_next().await.map_err(storage)? {
        if crate::database::fulfillment_scheduler::assetless_mail_notification(
            &row.try_get::<serde_json::Value, _>("payload").map_err(storage)?,
            row.try_get::<&str, _>("origin_id").map_err(storage)?,
            row.try_get("command_id").map_err(storage)?,
        ) {
            ids.push(row.try_get("id").map_err(storage)?);
        }
    }
    Ok(ids)
}

pub(super) fn ids(values: &[Uuid]) -> HashSet<String> {
    values.iter().map(Uuid::to_string).collect()
}
