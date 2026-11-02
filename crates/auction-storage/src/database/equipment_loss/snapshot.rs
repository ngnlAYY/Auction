//! 损失处置快照；预览与提交复用同一份资格投影，原始快照仅留数据库事实表。

use super::*;
use auction_domain::{FundingSplit, SettlementAmount};
use serde_json::Value;

pub(super) struct Snapshot {
    pub data: Value,
    pub preview: EquipmentLossPreview,
}

impl Snapshot {
    pub fn uuid(&self, row: &str, key: &str) -> Result<Uuid, Error> {
        self.data[row][key]
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
            .filter(|id| !id.is_nil())
            .ok_or(Error::Rejected)
    }

    pub fn split(&self) -> Result<FundingSplit, Error> {
        let amount = |key: &str| {
            SettlementAmount::new(self.data["settlement"][key].as_i64().ok_or(Error::Rejected)?)
                .map_err(|_| Error::Rejected)
        };
        Ok(FundingSplit {
            buyer: amount("buyer_amount")?,
            compensation: amount("compensation_amount")?,
            seller: amount("seller_amount")?,
            tax: amount("tax_amount")?,
        })
    }
}

pub(super) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    lot_id: Uuid,
) -> Result<Snapshot, Error> {
    let data: Option<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('auction',to_jsonb(a),'lot',to_jsonb(l),
          'equipment',to_jsonb(e),'settlement',to_jsonb(ls),'recovery',to_jsonb(r),
          'payment',to_jsonb(p),'payment_fulfillment',to_jsonb(pf),
          'seller',to_jsonb(s),'seller_fulfillment',to_jsonb(sf),
          'refund',to_jsonb(b),'refund_fulfillment',to_jsonb(bf),'delivery',to_jsonb(d),
          'funding_accounts',(SELECT COALESCE(jsonb_agg(jsonb_build_object('uid',uid,
             'status',status,'accounting_mode',accounting_mode)),'[]'::jsonb)
             FROM public.settlement_funding_accounts),
          'conflict',(
             EXISTS(SELECT 1 FROM public.settlement_funding_lots WHERE lot_id=l.id)
             OR EXISTS(SELECT 1 FROM public.settlement_funding_allocations WHERE lot_id=l.id)
             OR EXISTS(SELECT 1 FROM public.settlement_equipment_losses WHERE lot_id=l.id OR delivery_id=d.id)
             OR EXISTS(SELECT 1 FROM public.settlement_recovery_actions WHERE recovery_id=r.id)
             OR EXISTS(SELECT 1 FROM public.compensation_pool_reversals WHERE lot_id=l.id)
             OR EXISTS(SELECT 1 FROM public.delivery_sent_bindings WHERE delivery_id=d.id)
             OR EXISTS(SELECT 1 FROM public.settlement_funding_receipts WHERE payment_obligation_id=p.id OR delivery_id=d.id)
             OR EXISTS(SELECT 1 FROM public.settlement_funding_sources WHERE source_key=p.id)
             OR EXISTS(SELECT 1 FROM public.settlement_internal_commands WHERE obligation_id IN (p.id,s.id,b.id))
             OR EXISTS(SELECT 1 FROM public.settlement_legacy_self_reconciliations WHERE obligation_id IN (p.id,s.id,b.id))
             OR EXISTS(SELECT 1 FROM public.settlement_legacy_bot_seller_claims WHERE lot_id=l.id)
             OR EXISTS(SELECT 1 FROM public.deliveries WHERE id<>d.id AND origin_kind='settlement' AND origin_id IN (p.id::text,s.id::text,b.id::text))
             OR (SELECT count(*) FROM public.settlement_obligations WHERE lot_id=l.id)<>3))
         FROM public.lots l JOIN public.auctions a ON a.id=l.auction_id
         JOIN public.equipment e ON e.id=l.equipment_id
         JOIN public.lot_settlements ls ON ls.lot_id=l.id
         JOIN public.auction_settlements ast ON ast.auction_id=a.id
         JOIN public.settlement_recoveries r ON r.lot_id=l.id AND r.auction_id=a.id
         JOIN public.settlement_obligations p ON p.id=r.payment_obligation_id
         JOIN public.settlement_obligation_fulfillments pf ON pf.obligation_id=p.id
         JOIN public.settlement_obligations s ON s.id=r.seller_payout_obligation_id
         JOIN public.settlement_obligation_fulfillments sf ON sf.obligation_id=s.id
         JOIN public.settlement_obligations b ON b.id=r.buyer_refund_obligation_id
         JOIN public.settlement_obligation_fulfillments bf ON bf.obligation_id=b.id
         JOIN public.deliveries d ON d.id=pf.delivery_id
         WHERE l.id=$1",
    )
    .bind(lot_id).fetch_optional(&mut **tx).await?;
    let mut preview = EquipmentLossPreview {
        eligible: false,
        blockers: Vec::new(),
        lot_id,
        funding_uid: None,
        expected_recovery_version: 0,
        expected_payment_version: 0,
        expected_delivery_fencing_version: 0,
        seller_amount: 0,
        buyer_amount: 0,
    };
    let Some(data) = data else {
        preview.blockers.push("missing_settled_equipment_cod_graph".into());
        return Ok(Snapshot { data: Value::Null, preview });
    };
    preview.expected_recovery_version = data["recovery"]["state_version"].as_u64().unwrap_or(0);
    preview.expected_payment_version =
        data["payment_fulfillment"]["state_version"].as_u64().unwrap_or(0);
    preview.expected_delivery_fencing_version =
        data["delivery"]["fencing_version"].as_i64().unwrap_or(0);
    preview.seller_amount = data["settlement"]["seller_amount"].as_i64().unwrap_or(0);
    preview.buyer_amount = data["settlement"]["buyer_amount"].as_i64().unwrap_or(0);
    let accounts = data["funding_accounts"].as_array().ok_or(Error::Rejected)?;
    if accounts.len() == 1 {
        preview.funding_uid = accounts[0]["uid"].as_str().map(str::to_owned);
    }
    let mut require = |condition: bool, blocker: &str| {
        if !condition {
            preview.blockers.push(blocker.to_owned());
        }
    };
    require(
        accounts.len() == 1
            && accounts[0]["status"] == "ready"
            && accounts[0]["accounting_mode"] == "record_only",
        "requires_unique_ready_record_only_account",
    );
    require(
        data["auction"]["world_id"] == "main"
            && data["auction"]["status"] == "settled"
            && !data["auction"]["settled_at"].is_null()
            && data["lot"]["status"] == "sold",
        "requires_main_settled_sold_lot",
    );
    let p = &data["payment"];
    let s = &data["seller"];
    let b = &data["refund"];
    let pf = &data["payment_fulfillment"];
    let sf = &data["seller_fulfillment"];
    let bf = &data["refund_fulfillment"];
    let r = &data["recovery"];
    let d = &data["delivery"];
    require(
        p["obligation_kind"] == "winner_equipment_cod"
            && p["business_state"] == "ready"
            && p["amount"] == data["settlement"]["buyer_amount"]
            && s["obligation_kind"] == "seller_payout"
            && s["business_state"] == "blocked"
            && s["amount"] == data["settlement"]["seller_amount"]
            && s["prerequisite_obligation_id"] == p["id"]
            && b["obligation_kind"] == "buyer_refund"
            && b["business_state"] == "contingent"
            && b["amount"] == p["amount"]
            && b["prerequisite_obligation_id"] == p["id"],
        "frozen_obligation_mismatch",
    );
    require(
        [p, s, b].iter().all(|o| {
            o["lot_id"] == data["lot"]["id"]
                && o["auction_id"] == data["auction"]["id"]
                && o["equipment_id"] == data["equipment"]["id"]
                && o["world_id"] == "main"
        }) && r["equipment_id"] == data["equipment"]["id"]
            && r["winner_user_id"] == p["recipient_user_id"]
            && p["recipient_user_id"] == data["lot"]["leader_user_id"]
            && b["recipient_user_id"] == p["recipient_user_id"]
            && data["equipment"]["world_id"] == "main",
        "owner_identity_mismatch",
    );
    require(
        r["world_id"] == "main"
            && r["stage"] == "awaiting_payment"
            && r["cod_return_count"] == 0
            && r["reminder_count"] == 0
            && [
                "resolution",
                "equipment_obligation_id",
                "return_mail_id",
                "return_external_mail_id",
                "custody_proof_kind",
                "custody_proof_reference",
                "custody_proof_digest",
                "custody_proof_observed_at",
                "relisted_auction_id",
                "relisted_lot_id",
                "blocking_code",
                "manual_reconcile_delivery_id",
                "completed_at",
            ]
            .iter()
            .all(|key| r[*key].is_null()),
        "recovery_already_acted",
    );
    require(
        pf["execution_state"] == "manual_reconcile"
            && sf["execution_state"] == "pending"
            && bf["execution_state"] == "pending"
            && sf["state_version"] == 1
            && bf["state_version"] == 1
            && [sf, bf].iter().all(|f| {
                ["delivery_id", "prepare_command_id", "activate_command_id"]
                    .iter()
                    .all(|key| f[*key].is_null())
            })
            && [pf, sf, bf].iter().all(|f| {
                [
                    "claim_token",
                    "claimed_at",
                    "claim_expires_at",
                    "proof_kind",
                    "proof_reference",
                    "proof_digest",
                    "proof_observed_at",
                ]
                .iter()
                .all(|key| f[*key].is_null())
            }),
        "fulfillment_already_acted_or_claimed",
    );
    require(
        d["status"] == "manual_reconcile"
            && d["attempt_count"] == 1
            && d["fencing_version"] == 1
            && d["failure_code"] == "not_observed"
            && d["origin_kind"] == "settlement"
            && d["origin_id"] == p["id"]
            && d["command_id"] == pf["prepare_command_id"]
            && d["request_digest"] == pf["prepare_request_digest"]
            && d["activation_command_id"] == pf["activate_command_id"]
            && d["activation_request_digest"] == pf["activate_request_digest"]
            && d["external_reference"].is_null()
            && d["lease_token"].is_null()
            && d["lease_expires_at"].is_null()
            && d["result_metadata"]["operation"] == "inventory"
            && d["result_metadata"]["stage"] == "read"
            && d["result_metadata"]["write_started"] == false
            && d["result_metadata"]["proof"].is_null()
            && d["result_metadata"]["sent_verified"].is_null(),
        "original_attempt_not_proven_inventory_only",
    );
    require(
        d["payload"]["schema_version"] == 1
            && d["payload"]["operation"] == "send"
            && d["payload"]["world"] == "main"
            && d["payload"]["template"] == "settlement_winner_equipment_cod"
            && d["payload"]["cod"] == p["amount"]
            && d["payload"]["credits"] == 0
            && d["payload"]["equipment_secret_refs"]
                == serde_json::json!([{"kind":"auction_equipment_key","id":p["equipment_id"]}]),
        "original_delivery_payload_mismatch",
    );
    require(data["conflict"] == false, "existing_disposition_or_funding");
    let mut snapshot = Snapshot { data, preview };
    if snapshot
        .split()
        .and_then(|split| {
            auction_domain::equipment_loss_funding(split, true, true).map_err(|_| Error::Rejected)
        })
        .is_err()
    {
        snapshot.preview.blockers.push("invalid_frozen_amounts".into());
    }
    if snapshot.preview.blockers.is_empty() {
        match super::super::fulfillment_scheduler::exclude_equipment_loss_conflicts(
            tx,
            lot_id,
            snapshot.uuid("payment", "id")?,
            snapshot.data["equipment"]["external_equipment_id"].as_str().ok_or(Error::Rejected)?,
            snapshot.uuid("delivery", "id")?,
        )
        .await
        {
            Ok(()) => {}
            Err(FundingError::Unproven | FundingError::Conflict | FundingError::Invalid) => {
                snapshot.preview.blockers.push("conflicting_asset_operation".into())
            }
            Err(error) => return Err(Error::Funding(error)),
        }
    }
    snapshot.preview.eligible = snapshot.preview.blockers.is_empty();
    Ok(snapshot)
}
