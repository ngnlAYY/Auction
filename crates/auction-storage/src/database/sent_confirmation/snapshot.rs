//! 人工证明准入与原 owner 快照；预览不返回邮件正文或历史私有快照。

use super::*;

pub(super) struct Snapshot {
    pub data: Value,
    pub preview: SentConfirmationPreview,
    pub expectation: Option<DeliverySendExpectation>,
    pub sending_fencing_version: i64,
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
             OR EXISTS(SELECT 1 FROM public.settlement_recovery_actions x WHERE x.recovery_id=r.id)
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
    };
    let Some(data) = data else {
        preview.blockers.push("delivery_not_found".into());
        return Ok(Snapshot {
            data: Value::Null,
            preview,
            expectation: None,
            sending_fencing_version: 0,
        });
    };
    let d = &data["delivery"];
    let o = &data["obligation"];
    let f = &data["fulfillment"];
    let r = &data["recovery"];
    preview.expected_fencing_version = d["fencing_version"].as_i64().unwrap_or(0);
    preview.requires_attempt_history_acceptance = data["prior_unbound"] == true;
    let sending_fencing_version = data["sending_fencing_version"].as_i64().unwrap_or(0);
    let expectation =
        serde_json::from_value::<DeliverySendExpectation>(data["expectation"].clone()).ok();
    let now: DateTime<Utc> =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(&mut **tx).await?;
    let time = |value: &Value| {
        value
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc))
    };
    let mut require = |condition: bool, blocker: &str| {
        if !condition {
            preview.blockers.push(blocker.to_owned());
        }
    };
    require(
        d["origin_kind"] == "settlement"
            && d["payload"]["world"] == "main"
            && d["payload"]["operation"] == "send"
            && o["world_id"] == "main"
            && matches!(
                o["obligation_kind"].as_str(),
                Some(
                    "winner_equipment_cod" | "unsold_return" | "withdrawn_return" | "seller_payout"
                )
            ),
        "unsupported_delivery_scope",
    );
    require(
        d["status"] == "manual_reconcile"
            && d["failure_code"] == "write_unknown"
            && d["result_metadata"]["write_started"] == true
            && d["external_reference"].is_null()
            && d["lease_token"].is_null()
            && d["lease_expires_at"].is_null()
            && d["result_metadata"]["retired"] != true,
        "requires_unclaimed_current_write_unknown",
    );
    require(
        f["execution_state"] == "manual_reconcile"
            && f["delivery_id"] == d["id"]
            && f["delivery_operation"] == "send"
            && f["dispatch_world"] == "main"
            && f["claim_token"].is_null()
            && f["claim_expires_at"].is_null()
            && f["proof_kind"].is_null()
            && f["proof_reference"].is_null()
            && f["proof_digest"].is_null()
            && f["proof_observed_at"].is_null()
            && d["command_id"] == f["prepare_command_id"]
            && !f["prepare_command_id"].is_null()
            && d["request_digest"] == f["prepare_request_digest"]
            && d["activation_command_id"] == f["activate_command_id"]
            && !f["activate_command_id"].is_null()
            && d["activation_request_digest"] == f["activate_request_digest"]
            && o["business_state"] == "ready",
        "fulfillment_identity_or_claim_conflict",
    );
    require(
        r.is_null()
            || (r["stage"] == "awaiting_payment"
                && r["resolution"].is_null()
                && r["blocking_code"].is_null()
                && r["manual_reconcile_delivery_id"].is_null()
                && r["claim_token"].is_null()),
        "conflicting_recovery_disposition",
    );
    require(data["conflict"] == false, "existing_binding_retirement_or_conflicting_attempt");
    let accounts = data["accounts"].as_array().ok_or(Error::Rejected)?;
    require(
        accounts.len() == 1
            && accounts[0]["uid"] == evidence.sender_uid
            && accounts[0]["status"] == "ready",
        "requires_unique_ready_sender_funding_account",
    );
    require(
        data["player_name"].is_null() || data["player_name"] == evidence.sender_name,
        "sender_player_name_mismatch",
    );
    require(
        expectation.as_ref().and_then(|e| e.credential.as_ref()).is_some_and(|binding| {
            data["credential"]["id"] == binding.credential_id.to_string()
                && data["credential"]["kind"] == "mail_cookie"
                && data["credential"]["status"] == "active"
                && data["credential"]["version"] == binding.credential_version
                && matches!(
                    data["credential"]["scope"].as_str(),
                    Some("mooglemail:main" | "mooglemail:shared")
                )
        }),
        "frozen_credential_retired_or_changed",
    );
    require(
        matches!((time(&d["sending_at"]), time(&d["completed_at"])), (Some(start), Some(end))
        if start <= end && evidence.sent_minute <= end
            && evidence.sent_minute.checked_add_signed(chrono::Duration::minutes(1)).is_some_and(|upper| upper > start)
            && evidence.observed_at >= end && evidence.observed_at <= now),
        "sent_minute_outside_frozen_window",
    );
    if o["obligation_kind"] == "seller_payout" {
        let allocation = &data["funding_allocation"];
        let funded_lot = &data["funding_lot"];
        // 只消除原始卖方支出的未知占用；正常税款等分配保留，不能因支持现金而豁免资金身份冲突。
        require(
            !allocation["id"].is_null()
                && allocation["uid"] == evidence.sender_uid
                && allocation["lot_id"] == o["lot_id"]
                && allocation["obligation_id"] == o["id"]
                && allocation["delivery_id"] == d["id"]
                && allocation["purpose"] == "seller"
                && allocation["state"] == "unknown"
                && allocation["amount"].as_i64().is_some_and(|amount| amount > 0)
                && allocation["amount"] == o["amount"]
                && data["cash_reservation"] == true
                && funded_lot["lot_id"] == o["lot_id"]
                && funded_lot["uid"] == evidence.sender_uid
                && funded_lot["source_kind"] == "collected"
                && funded_lot["disposition"] == "seller"
                && !o["prerequisite_obligation_id"].is_null()
                && funded_lot["payment_obligation_id"] == o["prerequisite_obligation_id"],
            "requires_exact_unknown_seller_reservation",
        );
    }
    let valid_frozen = expectation.as_ref().is_some_and(|expected| {
        let item_name = d["payload"]["variables"]["item_name"].as_str().unwrap_or("");
        evidence.matches_frozen(expected, item_name)
            && expected.credential.is_some()
            && expected.world_id == d["payload"]["world"]
            && expected.recipient == d["recipient"]
            && expected.credits == d["payload"]["credits"].as_i64().unwrap_or(-1)
            && expected.cod == d["payload"]["cod"].as_i64().unwrap_or(-1)
            && match o["obligation_kind"].as_str() {
                Some("seller_payout") => {
                    expected.credits > 0
                        && expected.cod == 0
                        && expected.equipment_ids.is_empty()
                        && o["amount"].as_i64() == Some(expected.credits)
                        && o["carrier_credit"] == 0
                        && o["recipient_user_id"].is_null()
                        && o["recipient_game_identity"] == expected.recipient
                        && data["lot"]["auction_id"] == o["auction_id"]
                        && data["lot"]["equipment_id"] == o["equipment_id"]
                        && data["equipment"]["world_id"] == "main"
                        && data["settlement"]["seller_amount"] == o["amount"]
                        && d["payload"] == serde_json::json!({
                            "schema_version":1,"operation":"send","world":"main",
                            "template":"settlement_seller_payout",
                            "variables":{"reference":o["id"],"amount":expected.credits.to_string()},
                            "credits":expected.credits,"cod":0,"equipment_secret_refs":[]
                        })
                }
                Some("winner_equipment_cod" | "unsold_return" | "withdrawn_return") => {
                    expected
                        .equipment_ids
                        .first()
                        .is_some_and(|id| data["equipment"]["external_equipment_id"] == *id)
                        && d["payload"]["equipment_secret_refs"]
                            == serde_json::json!([{"kind":"auction_equipment_key","id":o["equipment_id"]}])
                        && match o["obligation_kind"].as_str() {
                            Some("winner_equipment_cod") => {
                                d["payload"]["template"] == "settlement_winner_equipment_cod"
                                    && o["amount"].as_i64() == Some(expected.cod)
                            }
                            Some("unsold_return" | "withdrawn_return") => {
                                d["payload"]["template"] == "settlement_equipment_handoff"
                                    && expected.cod == 0
                            }
                            _ => false,
                        }
                }
                _ => false,
            }
    });
    require(
        valid_frozen && sending_fencing_version > 0,
        "frozen_identity_body_money_or_assets_mismatch",
    );
    preview.eligible = preview.blockers.is_empty();
    Ok(Snapshot { data, preview, expectation, sending_fencing_version })
}
