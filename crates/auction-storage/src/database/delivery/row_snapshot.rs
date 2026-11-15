//! 投递整行快照证明的共享 SQL 片段：历史自寄修复与装备损失两个只读证明共用同一整行比较。

/// `deliveries` 当前行（别名固定为 `d`）与冻结快照 `<快照别名>.original_snapshot->'delivery'`
/// 的整行相等条件，前面不带 `AND`。
///
/// 两侧先把每个 `timestamptz` 列用同一 `AT TIME ZONE 'UTC'` 归一化再转 JSON：`to_jsonb(行)`
/// 按会话 `TimeZone` 渲染时间文本，直接整行比较会随会话时区翻转结论；归一化后结论只取决于
/// 时刻本身，非 UTC 偏移的快照时间文本也按同一瞬间比较。快照缺失归一化时间键时先由 `?&`
/// 拒绝，避免当成 `null`；时间值不可解析时求值报错，不静默判为相等。
///
/// 两侧一起剔除各证明单独断言或允许变动的列：公共的 `status`、`fencing_version`、`updated_at`、
/// `result_metadata`，迁移 `000094` 新增的 `payload` 派生生成列 `payload_world`/
/// `payload_operation`/`payload_template`（升级前写入的快照没有这些键），以及调用方以第二个
/// 参数追加的剔除键（形如 `-'failure_code'`）。整行比较覆盖 `deliveries` 的全部其余列：表新增
/// 任何列都必须在这里归一化或剔除，否则缺该键的历史快照会让两个证明一起静默失效。
macro_rules! delivery_row_snapshot_equality {
    ($snapshot:literal, $extra_excluded:literal) => {
        concat!(
            "(",
            $snapshot,
            r#".original_snapshot->'delivery') ?& ARRAY[
        'lease_expires_at','claimed_at','sending_at','completed_at','activated_at','created_at']
    AND ((to_jsonb(d) || jsonb_build_object(
            'lease_expires_at',coalesce(to_jsonb(d.lease_expires_at AT TIME ZONE 'UTC'),'null'::jsonb),
            'claimed_at',coalesce(to_jsonb(d.claimed_at AT TIME ZONE 'UTC'),'null'::jsonb),
            'sending_at',coalesce(to_jsonb(d.sending_at AT TIME ZONE 'UTC'),'null'::jsonb),
            'completed_at',coalesce(to_jsonb(d.completed_at AT TIME ZONE 'UTC'),'null'::jsonb),
            'activated_at',coalesce(to_jsonb(d.activated_at AT TIME ZONE 'UTC'),'null'::jsonb),
            'created_at',coalesce(to_jsonb(d.created_at AT TIME ZONE 'UTC'),'null'::jsonb)))
        -'status'-'fencing_version'-'updated_at'-'result_metadata'"#,
            $extra_excluded,
            r#"-'payload_world'-'payload_operation'-'payload_template')=
        (("#,
            $snapshot,
            r#".original_snapshot->'delivery' || jsonb_build_object(
            'lease_expires_at',coalesce(to_jsonb(("#,
            $snapshot,
            r#".original_snapshot#>>'{delivery,lease_expires_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
            'claimed_at',coalesce(to_jsonb(("#,
            $snapshot,
            r#".original_snapshot#>>'{delivery,claimed_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
            'sending_at',coalesce(to_jsonb(("#,
            $snapshot,
            r#".original_snapshot#>>'{delivery,sending_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
            'completed_at',coalesce(to_jsonb(("#,
            $snapshot,
            r#".original_snapshot#>>'{delivery,completed_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
            'activated_at',coalesce(to_jsonb(("#,
            $snapshot,
            r#".original_snapshot#>>'{delivery,activated_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb),
            'created_at',coalesce(to_jsonb(("#,
            $snapshot,
            r#".original_snapshot#>>'{delivery,created_at}')::timestamptz AT TIME ZONE 'UTC'),'null'::jsonb)))
        -'status'-'fencing_version'-'updated_at'-'result_metadata'"#,
            $extra_excluded,
            "-'payload_world'-'payload_operation'-'payload_template')"
        )
    };
}
pub(super) use delivery_row_snapshot_equality;
