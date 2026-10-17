//! 完整责任快照：旧无UID责任只隔离扣占，绝不改历史归属或生成收款事实。

use super::*;
use auction_application::{
    FundingFuture, FundingPreview, FundingReconciliationCommand, FundingReconciliationReader,
    FundingReconciliationResult,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

impl FundingReconciliationReader for crate::DatabaseStore {
    fn preview_funding(&self, expected_uid: String) -> FundingFuture<'_, FundingPreview> {
        Box::pin(async move {
            auction_domain::FundingAccountKey::new(&expected_uid).map_err(invalid)?;
            let mut tx = self.postgres_pool().begin().await.map_err(storage)?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE READ ONLY")
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            let result = snapshot(&mut tx, &expected_uid).await?;
            tx.commit().await.map_err(storage)?;
            Ok(result)
        })
    }
    fn replay_funding(
        &self,
        command: FundingReconciliationCommand,
    ) -> FundingFuture<'_, Option<FundingReconciliationResult>> {
        Box::pin(async move {
            command.validate()?;
            let mut tx = self.postgres_pool().begin().await.map_err(storage)?;
            sqlx::query("SET TRANSACTION READ ONLY").execute(&mut *tx).await.map_err(storage)?;
            let result = super::reconciliation::replay_command(&mut tx, &command).await?;
            tx.commit().await.map_err(storage)?;
            Ok(result)
        })
    }
}

async fn array(
    tx: &mut Transaction<'_, Postgres>,
    query: &'static str,
) -> Result<Value, FundingError> {
    sqlx::query_scalar(query).fetch_one(&mut **tx).await.map_err(storage)
}

pub(super) async fn snapshot(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
) -> Result<FundingPreview, FundingError> {
    // 明确投影安全财务字段，不导出payload正文、Cookie、secret_refs或整封邮件。
    let lots = array(tx, r#"SELECT COALESCE(jsonb_agg(to_jsonb(x) ORDER BY x.lot_id),'[]') FROM (
      SELECT l.*,p.id payment_id,p.business_state payment_business,pf.execution_state payment_state,
        pf.proof_kind payment_proof,pf.delivery_id payment_delivery,
        s.id seller_id,s.business_state seller_business,sf.execution_state seller_state,sf.delivery_id seller_delivery,
        b.id refund_id,b.business_state refund_business,bf.execution_state refund_state,bf.delivery_id refund_delivery,
        h.id handoff_id,hf.execution_state handoff_state,
        r.id recovery_id,r.stage recovery_stage,r.resolution,r.blocking_code,
        r.custody_proof_kind,r.custody_proof_digest,
        fl.uid funded_uid,fl.disposition funded_disposition
      FROM public.lot_settlements l
      LEFT JOIN public.settlement_obligations p ON p.lot_id=l.lot_id AND p.obligation_kind IN ('winner_equipment_cod','winner_payment_carrier_cod')
      LEFT JOIN public.settlement_obligation_fulfillments pf ON pf.obligation_id=p.id
      LEFT JOIN public.settlement_obligations s ON s.lot_id=l.lot_id AND s.obligation_kind='seller_payout'
      LEFT JOIN public.settlement_obligation_fulfillments sf ON sf.obligation_id=s.id
      LEFT JOIN public.settlement_obligations b ON b.lot_id=l.lot_id AND b.obligation_kind='buyer_refund'
      LEFT JOIN public.settlement_obligation_fulfillments bf ON bf.obligation_id=b.id
      LEFT JOIN public.settlement_obligations h ON h.lot_id=l.lot_id AND h.obligation_kind='winner_equipment_handoff'
      LEFT JOIN public.settlement_obligation_fulfillments hf ON hf.obligation_id=h.id
      LEFT JOIN public.settlement_recoveries r ON r.lot_id=l.lot_id
      LEFT JOIN public.settlement_funding_lots fl ON fl.lot_id=l.lot_id
    ) x"#).await?;
    let deliveries = array(tx, r#"SELECT COALESCE(jsonb_agg(to_jsonb(x) ORDER BY x.id),'[]') FROM (
      SELECT d.id,d.origin_kind,d.origin_id,d.status,d.fencing_version,d.sending_at,d.completed_at,
        d.failure_code,d.external_reference,d.request_digest,
        d.payload->'operation' operation,d.payload->'world' world,d.payload->'credits' credits,d.payload->'cod' cod,
        a.uid funded_uid,a.state funded_state
      FROM public.deliveries d LEFT JOIN public.settlement_funding_allocations a ON a.delivery_id=d.id
    ) x"#).await?;
    let recovery_actions = array(tx, r#"SELECT COALESCE(jsonb_agg(to_jsonb(x) ORDER BY x.id),'[]') FROM (
      SELECT id,recovery_id,kind,execution_state,state_version,expected_credits,expected_cod,delivery_id,proof_kind,proof_digest,last_error_code
      FROM public.settlement_recovery_actions
    ) x"#).await?;
    let economics = array(tx, r#"SELECT COALESCE(jsonb_agg(to_jsonb(x) ORDER BY x.settled_at,x.auction_id),'[]') FROM (
      SELECT s.auction_id,a.world_id,s.opening_pool_balance,s.closing_pool_balance,s.tax_total,s.compensation_total,s.settled_at
      FROM public.auction_settlements s JOIN public.auctions a ON a.id=s.auction_id
    ) x"#).await?;
    let pools = array(tx,"SELECT COALESCE(jsonb_agg(to_jsonb(p) ORDER BY world_id),'[]') FROM public.settlement_pools p").await?;
    let all_accounts = array(tx,"SELECT COALESCE(jsonb_agg(to_jsonb(a) ORDER BY uid),'[]') FROM public.settlement_funding_accounts a").await?;
    let allocations = array(tx,"SELECT COALESCE(jsonb_agg(to_jsonb(a) ORDER BY uid,id),'[]') FROM public.settlement_funding_allocations a").await?;
    let mut blockers = Vec::new();
    let mut liabilities = 0i64;
    let mut compensation = 0i64;
    for lot in lots.as_array().ok_or(FundingError::Storage)? {
        if lot["buyer_amount"].as_i64() == Some(0) {
            continue;
        }
        if matches!(lot["handoff_state"].as_str(), Some("failed" | "manual_reconcile"))
            || (!lot["handoff_id"].is_null() && lot["handoff_state"].is_null())
        {
            blockers.push(format!("lot:{}:handoff_unproven", lot["lot_id"]));
        }
        if lot["seller_state"] == "satisfied" && lot["refund_state"] == "satisfied" {
            blockers.push(format!("lot:{}:seller_refund_conflict", lot["lot_id"]));
        }
        let b = number(lot, "buyer_amount")?;
        let c = number(lot, "compensation_amount")?;
        let s = number(lot, "seller_amount")?;
        let t = number(lot, "tax_amount")?;
        if b.checked_add(c) != s.checked_add(t) {
            return Err(FundingError::Invalid);
        }
        for field in ["payment_state", "seller_state", "refund_state"] {
            if matches!(lot[field].as_str(), Some("failed" | "manual_reconcile"))
                || lot[field].is_null()
            {
                blockers.push(format!("lot:{}:{field}:unproven", lot["lot_id"]));
            }
        }
        if !lot["blocking_code"].is_null() {
            blockers.push(format!("lot:{}:recovery_blocked", lot["lot_id"]));
        }
        if !lot["funded_uid"].is_null() {
            continue;
        }
        if lot["payment_state"] == "satisfied" {
            if lot["payment_proof"] != "mooglemail_sent_collected" {
                blockers.push(format!("lot:{}:payment_source_unproven", lot["lot_id"]));
            }
            let amount = if lot["refund_state"] == "satisfied" {
                c
            } else if lot["seller_state"] == "satisfied" {
                t
            } else {
                s.checked_add(t).ok_or(FundingError::Invalid)?
            };
            liabilities = liabilities.checked_add(amount).ok_or(FundingError::Invalid)?;
            if lot["refund_business"] == "ready"
                && (lot["resolution"] != "equipment_recovered"
                    || lot["seller_business"] != "blocked")
            {
                blockers.push(format!("lot:{}:refund_alternative_unproven", lot["lot_id"]));
            }
        } else {
            if !lot["payment_delivery"].is_null() {
                let payment_delivery = deliveries
                    .as_array()
                    .ok_or(FundingError::Storage)?
                    .iter()
                    .find(|delivery| delivery["id"] == lot["payment_delivery"]);
                // Main装备COD及跨世界现金carrier都可能在本地收款核对前已被领取。
                // 已外发或关联缺失时阻断opening，不能把未satisfied推断成现金尚未到账。
                if payment_delivery.is_none_or(|delivery| {
                    delivery["status"] == "succeeded" || !delivery["sending_at"].is_null()
                }) {
                    blockers.push(format!("lot:{}:payment_collection_unclosed", lot["lot_id"]));
                }
            }
            // 确定尚未外发的contingent不是已支付责任；只列冻结补偿需求供明确授权。
            compensation = compensation.checked_add(c).ok_or(FundingError::Invalid)?;
        }
    }
    for delivery in deliveries.as_array().ok_or(FundingError::Storage)? {
        let state = delivery["status"].as_str().ok_or(FundingError::Storage)?;
        if matches!(state, "sending" | "manual_reconcile")
            || (state == "failed" && !delivery["sending_at"].is_null())
        {
            blockers.push(format!("delivery:{}:outcome_unknown", delivery["id"]));
        }
        if !delivery["funded_uid"].is_null() {
            continue;
        }
        let credits = delivery["credits"].as_i64();
        let cod = delivery["cod"].as_i64();
        if credits.is_none()
            || cod.is_none()
            || credits.is_some_and(|n| n < 0)
            || cod.is_some_and(|n| n < 0)
        {
            blockers.push(format!("delivery:{}:assets_unbounded", delivery["id"]));
            continue;
        }
        if state == "failed" && (credits != Some(0) || cod != Some(0)) {
            blockers.push(format!("delivery:{}:failed_assets_unproven", delivery["id"]));
        }
        if matches!(state, "prepared" | "pending" | "claimed") && credits.is_some_and(|n| n > 0) {
            // 即使已在lot上界中计入，也保守重复隔离旧附件；不让旧prepared逃过门。
            liabilities = liabilities
                .checked_add(credits.ok_or(FundingError::Invalid)?)
                .ok_or(FundingError::Invalid)?;
        }
        if state == "succeeded"
            && delivery["origin_kind"] != "settlement"
            && credits.is_some_and(|n| n > 0)
        {
            blockers.push(format!("delivery:{}:legacy_generic_cash_unattributed", delivery["id"]));
        }
    }
    for action in recovery_actions.as_array().ok_or(FundingError::Storage)? {
        if matches!(
            action["execution_state"].as_str(),
            Some("failed" | "manual_reconcile" | "prepared" | "enqueued")
        ) {
            blockers.push(format!("recovery_action:{}:assets_unresolved", action["id"]));
        }
    }
    let mut worlds = std::collections::HashSet::new();
    for row in economics.as_array().ok_or(FundingError::Storage)? {
        let world = row["world_id"].as_str().ok_or(FundingError::Storage)?;
        if worlds.insert(world) && row["opening_pool_balance"].as_i64() != Some(0) {
            blockers.push(format!("legacy_opening_pool_unexplained:{world}"));
        }
    }
    let mut version = 0;
    let mut status = "unreconciled".to_owned();
    for account in all_accounts.as_array().ok_or(FundingError::Storage)? {
        if account["uid"] == uid {
            version = number(account, "state_version")?;
            status = account["status"].as_str().ok_or(FundingError::Storage)?.to_owned();
        } else if matches!(account["status"].as_str(), Some("ready" | "frozen")) {
            blockers.push("other_uid_protected_account_requires_reconciliation".to_owned());
        }
    }
    let mut allocated = 0i64;
    let mut isolated = 0i64;
    for allocation in allocations.as_array().ok_or(FundingError::Storage)? {
        if allocation["uid"] != uid {
            continue;
        }
        if matches!(allocation["state"].as_str(), Some("unknown" | "in_flight")) {
            blockers.push("funding_outcome_unknown".to_owned());
        }
        if allocation["state"] != "spent" {
            allocated = allocated
                .checked_add(number(allocation, "amount")?)
                .ok_or(FundingError::Invalid)?;
            if allocation["purpose"] == "quarantine" {
                isolated = isolated
                    .checked_add(number(allocation, "amount")?)
                    .ok_or(FundingError::Invalid)?;
            }
            if allocation["purpose"] == "compensation" {
                // 新增需求下限为零；既有补偿仍完整计入allocated，不释放多余占用。
                compensation = compensation.saturating_sub(number(allocation, "amount")?).max(0);
            }
        }
    }
    // 既有隔离永不自动释放；新增隔离只覆盖责任上界增加值，且明确不低于零。
    liabilities = liabilities.saturating_sub(isolated).max(0);
    blockers.sort();
    blockers.dedup();
    let facts = json!({"lots":lots,"deliveries":deliveries,"recovery_actions":recovery_actions,"economics":economics,"pools":pools,"accounts":all_accounts,"allocations":allocations});
    let digest = Sha256::digest(
        serde_json::to_vec(&json!({"version":1,"uid":uid,"facts":facts})).map_err(invalid)?,
    )
    .into();
    Ok(FundingPreview {
        expected_uid: uid.to_owned(),
        account_version: version,
        account_status: status,
        allocated_local: allocated,
        isolated_liabilities: liabilities,
        unfunded_compensation: compensation,
        blockers,
        facts,
        digest,
    })
}

fn number(value: &Value, key: &str) -> Result<i64, FundingError> {
    value[key].as_i64().filter(|n| *n >= 0).ok_or(FundingError::Invalid)
}
