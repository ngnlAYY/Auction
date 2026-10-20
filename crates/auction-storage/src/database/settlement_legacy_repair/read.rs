//! 原版本快照及稳定命令重放；载荷只含结算公开字段和 secret UUID 引用。

use super::*;
use auction_application::{LegacySelfReconciliationAction as Action, RequestHash};
use auction_domain::{EquipmentId, SettlementWorld};
use serde_json::{Value, json};
use sqlx::Row;

pub(super) async fn replay(
    tx: &mut Transaction<'_, Postgres>,
    command: &LegacySelfReconciliationCommand,
) -> Result<Option<LegacySelfReconciliationResult>, FundingError> {
    let row = sqlx::query("SELECT request_digest,result FROM public.settlement_legacy_self_reconciliations WHERE command_id=$1")
        .bind(command.command_id).fetch_optional(&mut **tx).await.map_err(sql_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.try_get::<Vec<u8>, _>("request_digest").map_err(sql_error)?
        != command.request_digest()?
    {
        return Err(FundingError::Conflict);
    }
    let result: LegacySelfReconciliationResult =
        serde_json::from_value(row.try_get("result").map_err(sql_error)?)
            .map_err(|_| FundingError::Invalid)?;
    if result.command_id != command.command_id
        || result.obligation_id != command.target.obligation_id
        || result.delivery_id != command.target.delivery_id
        || result.action != command.target.action
    {
        return Err(FundingError::Conflict);
    }
    Ok(Some(result))
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, FundingError> {
    value[key].as_str().filter(|s| !s.is_empty() && *s == s.trim()).ok_or(FundingError::Invalid)
}
fn number(value: &Value, key: &str) -> Result<i64, FundingError> {
    value[key].as_i64().ok_or(FundingError::Invalid)
}
fn uuid(value: &Value, key: &str) -> Result<Uuid, FundingError> {
    Uuid::parse_str(text(value, key)?).map_err(|_| FundingError::Invalid)
}

pub(super) async fn snapshot(
    tx: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
) -> Result<(Value, LegacySelfReconciliationTarget), FundingError> {
    if obligation_id.is_nil() {
        return Err(FundingError::Invalid);
    }
    let original: Value = sqlx::query_scalar(
        "SELECT jsonb_build_object(
            'obligation',to_jsonb(o),'fulfillment',to_jsonb(f),
            'delivery',to_jsonb(d),'submission',jsonb_build_object('id',s.id,'source_kind',s.source_kind,'seller',s.seller,'source_reference',s.source_reference,'mailbox_key',s.mailbox_key,'external_mail_id',s.external_mail_id),
            'equipment',jsonb_build_object('id',e.id,'world_id',e.world_id,'external_equipment_id',e.external_equipment_id,'name',e.name),
            'lot',jsonb_build_object('status',l.status,'leader_id',l.leader_user_id),
            'recipient',jsonb_build_object('username',u.username,'status',u.status,'state_version',u.state_version))
         FROM public.settlement_obligations o
         JOIN public.settlement_obligation_fulfillments f ON f.obligation_id=o.id
         JOIN public.deliveries d ON d.id=f.delivery_id
         JOIN public.equipment e ON e.id=o.equipment_id
         JOIN public.lots l ON l.id=o.lot_id AND l.auction_id=o.auction_id AND l.equipment_id=e.id
         JOIN public.auctions a ON a.id=o.auction_id AND a.world_id=o.world_id
         LEFT JOIN public.auction_submissions s ON s.id=o.submission_id
         LEFT JOIN public.users u ON u.id=o.recipient_user_id
         WHERE o.id=$1"
    ).bind(obligation_id).fetch_one(&mut **tx).await.map_err(sql_error)?;
    let o = &original["obligation"];
    let f = &original["fulfillment"];
    let d = &original["delivery"];
    let e = &original["equipment"];
    let u = &original["recipient"];
    if o["business_state"] != "ready"
        || f["execution_state"] != "manual_reconcile"
        || d["status"] != "manual_reconcile"
        || d["origin_kind"] != "settlement"
        || d["origin_id"] != obligation_id.to_string()
        || f["delivery_operation"] != "send"
        || o["world_id"] != "main"
        || f["dispatch_world"] != "main"
        || e["world_id"] != "main"
        || !f["claim_token"].is_null()
        || !d["lease_token"].is_null()
        || !f["proof_kind"].is_null()
        || !d["external_reference"].is_null()
        || d["result_metadata"].get("legacy_self_reconciliation").is_some()
        || !o["prerequisite_obligation_id"].is_null()
        || number(o, "carrier_credit")? != 0
        || f["prepare_command_id"] != d["command_id"]
        || f["prepare_request_digest"] != d["request_digest"]
        || f["activate_command_id"].is_null()
        || f["activate_command_id"] != d["activation_command_id"]
        || f["activate_request_digest"] != d["activation_request_digest"]
    {
        return Err(FundingError::Unproven);
    }
    let (action, recipient, recipient_version) = match text(o, "obligation_kind")? {
        "unsold_return" if number(o, "amount")? == 0 && o["recipient_user_id"].is_null() => {
            let s = &original["submission"];
            let name = text(o, "recipient_game_identity")?;
            if s["seller"] != name
                || !matches!(s["source_kind"].as_str(), Some("admin" | "mail"))
                || original["lot"]["status"] != "unsold"
                || !original["lot"]["leader_id"].is_null()
            {
                return Err(FundingError::Unproven);
            }
            let source_valid = match s["source_kind"].as_str() {
                Some("admin") => {
                    s["source_reference"]
                        .as_str()
                        .is_some_and(|value| !value.is_empty() && value == value.trim())
                        && s["mailbox_key"].is_null()
                        && s["external_mail_id"].is_null()
                }
                Some("mail") => {
                    s["source_reference"].is_null()
                        && s["mailbox_key"] == "mooglemail:main"
                        && s["external_mail_id"]
                            .as_str()
                            .is_some_and(|value| !value.is_empty() && value == value.trim())
                }
                _ => false,
            };
            if !source_valid {
                return Err(FundingError::Unproven);
            }
            if !matches!(
                d["failure_code"].as_str(),
                Some("recipient_is_sender" | "write_unknown" | "remote_temporary")
            ) {
                return Err(FundingError::Unproven);
            }
            (Action::KeepUnsold, name.to_owned(), None)
        }
        "winner_equipment_cod" if number(o, "amount")? > 0 && !o["recipient_user_id"].is_null() => {
            let discard = &d["result_metadata"]["compose_discard"];
            if u["status"] != "active"
                || original["lot"]["status"] != "sold"
                || original["lot"]["leader_id"] != o["recipient_user_id"]
                || d["failure_code"] != "recipient_is_sender"
                || discard["outcome"] != "discarded"
                || discard["recipient_matches"] != true
                || discard["cod_matches"] != true
                || discard["matched_reference"] != obligation_id.to_string()
            {
                return Err(FundingError::Unproven);
            }
            (
                Action::ReopenPayment,
                text(u, "username")?.to_owned(),
                Some(number(u, "state_version")?),
            )
        }
        _ => return Err(FundingError::Invalid),
    };
    if d["recipient"] != recipient {
        return Err(FundingError::Unproven);
    }
    let mut variables = json!({"reference":obligation_id.to_string(),"item_name":text(e,"name")?});
    let template = if action == Action::ReopenPayment {
        variables["amount"] = number(o, "amount")?.to_string().into();
        "settlement_winner_equipment_cod"
    } else {
        "settlement_equipment_handoff"
    };
    let expected = json!({"schema_version":1,"operation":"send","world":"main","template":template,
        "variables":variables,"credits":0,"cod":number(o,"amount")?,
        "equipment_secret_refs":[{"kind":"auction_equipment_key","id":uuid(e,"id")?.to_string()}]});
    if d["payload"] != expected {
        return Err(FundingError::Unproven);
    }
    let target = LegacySelfReconciliationTarget {
        obligation_id,
        delivery_id: uuid(d, "id")?,
        action,
        world: SettlementWorld::Main,
        equipment_id: EquipmentId::from_uuid(uuid(e, "id")?),
        external_equipment_id: text(e, "external_equipment_id")?.to_owned(),
        recipient,
        expected_delivery_fencing: number(d, "fencing_version")?,
        expected_fulfillment_version: number(f, "state_version")?,
        expected_obligation_version: number(o, "state_version")?,
        recipient_version,
        snapshot_digest: RequestHash::from_request(
            "legacy_self_snapshot_v1",
            obligation_id,
            &original,
        )
        .map_err(|_| FundingError::Invalid)?
        .as_bytes(),
    };
    Ok((original, target))
}
