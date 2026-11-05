//! 已提交历史自寄修复记录的运行期证明；只从追加修复表与原投递完整身份相互匹配的事实解除历史阻塞，
//! 不相信单独 metadata。修复命令本身已下线，本模块只读，不写入修复表。
//!
//! 原投递与原义务的整行快照比较先把 `timestamptz` 列归一化为 UTC 瞬间再转 JSON。
//! `to_jsonb(行)` 本身会把时间戳渲染成会话 `TimeZone` 的文本，旧写法因此只在
//! `TimeZone=UTC` 会话下成立，其他会话时区下同一历史事实会被判为不匹配。

use crate::database::TrackedTransaction as Transaction;
use std::sync::LazyLock;

use auction_application::FundingError;
use sqlx::Postgres;
use uuid::Uuid;

use super::settlement_funding::storage as sql_error;

/// 调用方固定使用 r/d 别名；该条件不包含付款满足或资金用途授权。
///
/// 整行比较覆盖 `deliveries` 全部列：先把每个 `timestamptz` 列按 `AT TIME ZONE 'UTC'`
/// 归一化再转 JSON，再按原来的排除列剔除已单独断言的变动列；迁移 `000094` 的
/// `payload_world`/`payload_operation`/`payload_template` 是从 `payload` 派生的生成列，
/// 也在两侧一起剔除。两侧使用同一归一化函数，
/// 因此结论只取决于时刻本身，不取决于会话 `TimeZone`；快照里出现非 UTC 偏移的时间文本
/// 也按同一瞬间比较。时间键在快照中缺失时先由 `?&` 拒绝，避免把缺失键当成 `null`；
/// 快照时间值不是可解析的时间文本时求值报错，不静默判为相等。
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
    -- 会话时区无关的整行比较：时间列统一按 UTC 瞬间渲染。
    AND (r.original_snapshot->'delivery') ?& ARRAY[
        'lease_expires_at','claimed_at','sending_at','completed_at','activated_at','created_at']
    AND ((to_jsonb(d) || jsonb_build_object(
            'lease_expires_at',coalesce(to_jsonb(d.lease_expires_at AT TIME ZONE 'UTC'),'null'::jsonb),
            'claimed_at',coalesce(to_jsonb(d.claimed_at AT TIME ZONE 'UTC'),'null'::jsonb),
            'sending_at',coalesce(to_jsonb(d.sending_at AT TIME ZONE 'UTC'),'null'::jsonb),
            'completed_at',coalesce(to_jsonb(d.completed_at AT TIME ZONE 'UTC'),'null'::jsonb),
            'activated_at',coalesce(to_jsonb(d.activated_at AT TIME ZONE 'UTC'),'null'::jsonb),
            'created_at',coalesce(to_jsonb(d.created_at AT TIME ZONE 'UTC'),'null'::jsonb)))
        -'status'-'fencing_version'-'updated_at'-'result_metadata'-'payload_world'-'payload_operation'-'payload_template')=
        ((r.original_snapshot->'delivery' || jsonb_build_object(
            'lease_expires_at',coalesce(to_jsonb((r.original_snapshot#>>'{delivery,lease_expires_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
            'claimed_at',coalesce(to_jsonb((r.original_snapshot#>>'{delivery,claimed_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
            'sending_at',coalesce(to_jsonb((r.original_snapshot#>>'{delivery,sending_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
            'completed_at',coalesce(to_jsonb((r.original_snapshot#>>'{delivery,completed_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
            'activated_at',coalesce(to_jsonb((r.original_snapshot#>>'{delivery,activated_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
            'created_at',coalesce(to_jsonb((r.original_snapshot#>>'{delivery,created_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb)))
        -'status'-'fencing_version'-'updated_at'-'result_metadata'-'payload_world'-'payload_operation'-'payload_template')
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

/// `settlement_obligations` 的整行快照比较；排除列与原整行差值一致。
pub(crate) const TRUSTED_OBLIGATION_EQUALITY: &str = r#"
    ((to_jsonb(o) || jsonb_build_object(
        'created_at',coalesce(to_jsonb(o.created_at AT TIME ZONE 'UTC'),'null'::jsonb)))
     -'state_version'-'updated_at'-'business_state')=
    ((r.original_snapshot->'obligation' || jsonb_build_object(
        'created_at',coalesce(to_jsonb((r.original_snapshot#>>'{obligation,created_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb)))
     -'state_version'-'updated_at'-'business_state')
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
           AND {TRUSTED_OBLIGATION_EQUALITY}"
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

/// 集成测试入口：只返回静态 SQL 文本，不暴露任何证明结果、UID 或凭据。
#[cfg(feature = "test-db")]
pub fn trusted_delivery_predicate_sql() -> &'static str {
    TRUSTED_DELIVERY_PREDICATE
}

/// 集成测试入口：只返回静态 SQL 文本，别名固定为 `o`、`r`。
#[cfg(feature = "test-db")]
pub fn trusted_obligation_equality_sql() -> &'static str {
    TRUSTED_OBLIGATION_EQUALITY
}
