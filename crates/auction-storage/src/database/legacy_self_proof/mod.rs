//! 已提交历史自寄修复记录的运行期证明；只从追加修复表与原投递完整身份相互匹配的事实解除历史阻塞，
//! 不相信单独 metadata。修复命令本身已下线，本模块只读，不写入修复表。

use crate::database::TrackedTransaction as Transaction;
use std::sync::LazyLock;

use auction_application::FundingError;
use sqlx::Postgres;
use uuid::Uuid;

use super::settlement_funding::storage as sql_error;

/// 调用方固定使用 r/d 别名；该条件不包含付款满足或资金用途授权。
pub(crate) const TRUSTED_DELIVERY_PREDICATE: &str = r#"
    r.delivery_id=d.id AND d.status='failed'
    AND d.origin_kind='settlement' AND d.origin_id=r.obligation_id::text
    AND d.fencing_version=r.expected_delivery_fencing+1
    AND d.command_id::text=r.original_snapshot#>>'{delivery,command_id}'
    AND to_jsonb(d.request_digest)=r.original_snapshot#>'{delivery,request_digest}'
    AND coalesce(to_jsonb(d.activation_command_id),'null'::jsonb)=r.original_snapshot#>'{delivery,activation_command_id}'
    AND coalesce(to_jsonb(d.activation_request_digest),'null'::jsonb)=r.original_snapshot#>'{delivery,activation_request_digest}'
    AND d.payload=r.original_snapshot#>'{delivery,payload}'
    AND d.recipient=r.original_snapshot#>>'{delivery,recipient}'
    AND d.failure_code=r.original_snapshot#>>'{delivery,failure_code}'
    AND d.external_reference IS NULL
    AND (to_jsonb(d)-'status'-'fencing_version'-'updated_at'-'result_metadata')=
        ((r.original_snapshot->'delivery')-'status'-'fencing_version'-'updated_at'-'result_metadata')
    AND (d.result_metadata-'legacy_self_reconciliation')=r.original_snapshot#>'{delivery,result_metadata}'
    AND d.result_metadata->'legacy_self_reconciliation'->>'command_id'=r.command_id::text
    AND r.evidence->'identity'->>'uid'=r.uid
    AND r.evidence->'identity'->>'player_name'=d.recipient
    AND r.original_snapshot#>>'{obligation,id}'=r.obligation_id::text
    AND r.original_snapshot#>>'{fulfillment,obligation_id}'=r.obligation_id::text
    AND r.original_snapshot#>>'{fulfillment,delivery_id}'=r.delivery_id::text
    AND r.original_snapshot#>>'{fulfillment,execution_state}'='manual_reconcile'
    AND r.original_snapshot#>>'{delivery,status}'='manual_reconcile'
    AND (r.action='keep_unsold' OR (r.action='reopen_payment'
        AND d.failure_code='recipient_is_sender'
        AND r.original_snapshot#>>'{delivery,result_metadata,compose_discard,outcome}'='discarded'))
"#;

pub(crate) async fn legacy_self_uid(
    tx: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
) -> Result<Option<String>, FundingError> {
    // 与其他冲突门一致，仅缓存编译期 SQL 文本，不缓存任何义务或凭据的证明结果。
    static QUERY: LazyLock<String> = LazyLock::new(|| {
        format!(
            "SELECT r.uid, ({TRUSTED_DELIVERY_PREDICATE}) IS TRUE AS trusted FROM public.settlement_legacy_self_reconciliations r
         JOIN public.deliveries d ON d.id=r.delivery_id
         JOIN public.settlement_obligations o ON o.id=r.obligation_id
         WHERE r.obligation_id=$1 AND r.action='reopen_payment'
           AND (to_jsonb(o)-'state_version'-'updated_at'-'business_state')=
               ((r.original_snapshot->'obligation')-'state_version'-'updated_at'-'business_state')"
        )
    });
    use sqlx::Row;
    // 仅拼接本模块编译期常量；业务值始终通过 bind 传入。
    let row = sqlx::query(sqlx::AssertSqlSafe(QUERY.as_str()))
        .bind(obligation_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(sql_error)?;
    let Some(row) = row else {
        let exists: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.settlement_legacy_self_reconciliations WHERE obligation_id=$1 AND action='reopen_payment')")
            .bind(obligation_id).fetch_one(&mut **tx).await.map_err(sql_error)?;
        return if exists { Err(FundingError::Conflict) } else { Ok(None) };
    };
    if !row.try_get::<bool, _>("trusted").map_err(sql_error)? {
        return Err(FundingError::Unproven);
    }
    row.try_get("uid").map(Some).map_err(sql_error)
}
