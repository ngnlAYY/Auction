//! 人工证明准入与原 owner 快照；预览不返回邮件正文或历史私有快照。

use super::*;
use auction_application::SentAdmissionSnapshot;
use serde::Deserialize;

pub(super) struct Snapshot {
    /// 原始事实：随命令永久保存为 `original_snapshot`，不通过预览或 CLI 泄露。
    pub data: Value,
    /// 缺少目标投递时为 `None`；此时预览已含 `delivery_not_found`。
    pub admission: Option<SentAdmissionSnapshot>,
    pub preview: SentConfirmationPreview,
    pub expectation: Option<DeliverySendExpectation>,
    pub sending_fencing_version: i64,
}

impl Snapshot {
    /// 仅在已确认 `eligible` 后取用；缺失快照属于内部矛盾。
    pub fn admission(&self) -> Result<&SentAdmissionSnapshot, Error> {
        self.admission.as_ref().ok_or(Error::Rejected)
    }
}

pub(super) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
    evidence: &ManualSentEvidence,
) -> Result<Snapshot, Error> {
    let data: Option<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('delivery',to_jsonb(d),'obligation',to_jsonb(o),
          'fulfillment',to_jsonb(f),'equipment',to_jsonb(e),'recovery',to_jsonb(r),
          'expectation',a.expectation,'sending_fencing_version',a.sending_fencing_version,
          'lot',to_jsonb(l),'settlement',to_jsonb(s),
          'funding_allocation',(SELECT to_jsonb(x) FROM public.settlement_funding_allocations x WHERE x.delivery_id=d.id),
          'funding_lot',(SELECT to_jsonb(x) FROM public.settlement_funding_lots x WHERE x.lot_id=o.lot_id),
          'admin_failure',(SELECT jsonb_build_object('command_id',c.command_id,'command_kind',c.command_kind,
             'status',c.result_payload->'Status','fencing_version',c.result_payload->'FencingVersion',
             'failure_code',c.result_payload->'FailureCode','created_at',c.created_at)
             FROM public.delivery_admin_commands c WHERE c.delivery_id=d.id
             ORDER BY c.created_at DESC,c.command_id DESC LIMIT 1),
          'cash_reservation',EXISTS(
             SELECT 1 FROM public.settlement_funding_allocations x
             JOIN public.settlement_funding_commands reservation ON reservation.command_id=d.id
               AND reservation.uid=x.uid AND reservation.action='reserve' AND reservation.target_id=d.id
             WHERE x.delivery_id=d.id
               AND EXISTS(SELECT 1 FROM public.settlement_funding_entries entry
                 WHERE entry.command_id=d.id AND entry.uid=x.uid AND entry.kind='reserve'
                   AND entry.debit_id=x.id AND entry.credit_id=x.id AND entry.amount=x.amount
                   AND entry.source_id IS NULL AND entry.proof_digest=reservation.request_digest)
               AND EXISTS(SELECT 1 FROM public.settlement_funding_entries entry
                 WHERE entry.command_id=d.id AND entry.uid=x.uid AND entry.kind='unknown'
                   AND entry.debit_id=x.id AND entry.credit_id=x.id AND entry.amount=x.amount
                   AND entry.source_id IS NULL AND entry.proof_digest=d.request_digest)),
          'player_name',c.player_name,
          'credential',jsonb_build_object('id',c.id,'kind',c.credential_kind,'status',c.status,'scope',c.scope,'version',c.state_version),
          'accounts',(SELECT COALESCE(jsonb_agg(jsonb_build_object('uid',uid,'status',status)),'[]'::jsonb)
             FROM public.settlement_funding_accounts),
          'prior_unbound',EXISTS(SELECT 1 FROM public.delivery_send_attempts h
             WHERE h.delivery_id=d.id AND h.fencing_version<>a.sending_fencing_version
               AND NOT EXISTS(SELECT 1 FROM public.delivery_sent_bindings b
                 WHERE b.delivery_id=h.delivery_id AND b.fencing_version=h.fencing_version)),
          'conflict',(
             EXISTS(SELECT 1 FROM public.settlement_equipment_losses x WHERE x.delivery_id=d.id OR x.lot_id=o.lot_id)
             OR EXISTS(SELECT 1 FROM public.delivery_sent_bindings b WHERE b.delivery_id=d.id
                 OR (b.world_id=$2 AND b.external_mail_id=$3))
             OR EXISTS(SELECT 1 FROM public.deliveries x WHERE x.id<>d.id AND x.external_reference=$4)
             OR EXISTS(SELECT 1 FROM public.delivery_sent_confirmations x WHERE x.delivery_id=d.id)
             OR EXISTS(SELECT 1 FROM public.settlement_recovery_actions x WHERE x.recovery_id=r.id
                 AND NOT (x.kind='payment_reminder' AND x.execution_state='satisfied'))
             OR EXISTS(SELECT 1 FROM public.compensation_pool_reversals x WHERE x.lot_id=o.lot_id)
             OR EXISTS(SELECT 1 FROM public.settlement_funding_receipts x WHERE x.delivery_id=d.id)
             OR (o.obligation_kind<>'seller_payout' AND (
                 EXISTS(SELECT 1 FROM public.settlement_funding_lots x WHERE x.lot_id=o.lot_id)
                 OR EXISTS(SELECT 1 FROM public.settlement_funding_allocations x WHERE x.lot_id=o.lot_id)))
             OR EXISTS(SELECT 1 FROM public.settlement_funding_returns x WHERE x.delivery_id=d.id)
             OR EXISTS(SELECT 1 FROM public.delivery_send_attempts h
                 WHERE h.delivery_id<>d.id AND h.world_id=a.expectation->>'world_id'
                   AND h.expectation-ARRAY['credential','recipient']::text[]
                     =a.expectation-ARRAY['credential','recipient']::text[]
                   AND translate(h.expectation->>'recipient','ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz') COLLATE \"C\"
                     =translate(a.expectation->>'recipient','ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz') COLLATE \"C\"
                   AND h.expectation->'credential'->'credential_id'=a.expectation->'credential'->'credential_id'
                   AND h.expectation->'credential'->'credential_version'=a.expectation->'credential'->'credential_version'
                   AND NOT EXISTS(SELECT 1 FROM public.delivery_sent_bindings b
                       WHERE b.delivery_id=h.delivery_id AND b.fencing_version=h.fencing_version))))
         FROM public.deliveries d
         LEFT JOIN public.settlement_obligations o ON d.origin_kind='settlement' AND d.origin_id=o.id::text
         LEFT JOIN public.lots l ON l.id=o.lot_id
         LEFT JOIN public.lot_settlements s ON s.lot_id=o.lot_id
         LEFT JOIN public.settlement_obligation_fulfillments f ON f.obligation_id=o.id
         LEFT JOIN public.equipment e ON e.id=o.equipment_id
         LEFT JOIN public.settlement_recoveries r ON r.payment_obligation_id=o.id
         LEFT JOIN LATERAL public.delivery_current_send_attempt(d.id) a ON true
         LEFT JOIN public.credential_keys c ON c.id::text=a.expectation->'credential'->>'credential_id'
         WHERE d.id=$1",
    )
    .bind(delivery_id)
    .bind(&evidence.world_id)
    .bind(&evidence.external_mail_id)
    .bind(format!("mooglemail:{}:send:{}", evidence.world_id, evidence.external_mail_id))
    .fetch_optional(&mut **tx)
    .await?;
    let mut preview = SentConfirmationPreview {
        eligible: false,
        blockers: Vec::new(),
        delivery_id,
        expected_fencing_version: 0,
        external_reference: format!(
            "mooglemail:{}:send:{}",
            evidence.world_id, evidence.external_mail_id
        ),
        requires_collected_asset_acceptance: evidence.collected,
        requires_attempt_history_acceptance: false,
        corrects_admin_failure: false,
    };
    let Some(data) = data else {
        preview.blockers.push("delivery_not_found".into());
        return Ok(Snapshot {
            data: Value::Null,
            admission: None,
            preview,
            expectation: None,
            sending_fencing_version: 0,
        });
    };
    let now: DateTime<Utc> =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(&mut **tx).await?;
    // 准入判定在应用层完成；这里只把锁定行投影成结构体并翻译阻断项。
    let admission = SentAdmissionSnapshot::deserialize(&data)?;
    let decision = admission.evaluate(evidence, now);
    preview.blockers =
        decision.blockers.iter().map(|blocker| blocker.as_str().to_owned()).collect();
    preview.eligible = decision.eligible;
    preview.expected_fencing_version = decision.expected_fencing_version;
    preview.requires_attempt_history_acceptance = decision.requires_attempt_history_acceptance;
    preview.corrects_admin_failure = decision.corrects_admin_failure;
    let expectation = admission.expectation.clone();
    let sending_fencing_version = admission.sending_fencing_version;
    Ok(Snapshot { data, admission: Some(admission), preview, expectation, sending_fencing_version })
}
