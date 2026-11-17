//! 结算退件的持久 Take：建立、激活与收口三个本地事务步骤。
//!
//! 激活：锁内复核案件与 world 级同邮件互斥后，经 delivery owner 以受限来源
//! `settlement_return_action` 写入 prepared Take 投递并以独立激活身份推进为 pending；外部 Take 只由
//! 投递 Worker 在租约与发送围栏之后执行（本地先提交，不变量 4）。收口：Take 投递到终态后，只有
//! 精确的领取证明（同世界/MID、动作移除或成功页、预期资产逐项一致、投递身份与载荷未变）才一次性
//! 写入领回事实并同事务返还原资金用途；失败或未知一律转人工核对，不自动重试，也不接受操作者
//! 声明（不变量 4、6）。结果未知之后只有邮箱巡检在原 Take 会话里观察到的精确证明（integrations
//! 核对过原身份、原正文前缀与逐条领取回执，预期资产逐项一致）才算第二种证明形状，记账与成功页
//! 证明完全一致；“附件为空”本身不是证明。

use auction_application::core::RequestHash;
use auction_application::mail::{
    MailDeliveryActivation, MailDeliveryCommand, MailDeliveryOperation, MailDeliveryOrigin,
};
use auction_application::settlement::{SYSTEM_RETURN_SENDER, SettlementReturnAssets};
use auction_domain::{CommandId, deterministic_uuid};
use auction_time::Timestamp;
use serde::Serialize;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::cases::CaseRow;
use super::errors::ReturnTransactionError;
use crate::database::TrackedTransaction as Transaction;
use crate::database::repository_error;

const TAKE_PREPARE_LABEL: &str = "settlement-return-take-prepare";
const TAKE_ACTIVATE_LABEL: &str = "settlement-return-take-activate";
const TAKE_FUNDING_LABEL: &str = "settlement-return-funding";

/// 投递 Worker 执行 Take 时验证回执得到的证明（动作移除或成功页）。
pub(super) const TAKE_RECEIPT_PROOF: &str = "action_removed_or_success_page";

/// 邮箱巡检在原 Take 会话里补采的证明；结算退件只认带领取回执的完成观察。
pub(super) const TAKE_SWEEP_PROOF: &str = "action_recovered_inbox_v1";

/// 巡检证明中唯一能证明资产已入账的观察：integrations 在原 MID 详情页上核对了原发件人与主题、
/// 原退件正文前缀，以及与领取前附件条目数相同的 `Removed attachment:` 回执（与正常 Take 成功证明
/// 同等强度）。仅“附件已为空”可能来自过期、他人处置或页面漂移，邮件整体缺失也不能证明资产进入
/// Bot 账号，都不作为结算退件的领回证明。
pub(crate) const TAKE_SWEEP_REASON: &str = "take_receipt_proven";

/// Take 投递的来源标识：案件 ID（即退件入站邮件 ID）。
pub(super) fn take_origin_id(return_mail_id: Uuid) -> String {
    format!("settlement_return:{return_mail_id}")
}

/// 案件唯一的 Take prepare 命令 ID；事务重试与重放都得到同一身份。
pub(super) fn take_command_id(return_mail_id: Uuid) -> CommandId {
    stable_command(return_mail_id, TAKE_PREPARE_LABEL)
}

/// 案件唯一的 Take activate 命令 ID。
pub(super) fn take_activate_command_id(return_mail_id: Uuid) -> CommandId {
    stable_command(return_mail_id, TAKE_ACTIVATE_LABEL)
}

/// 领回返还资金的稳定资金命令 ID；同一领回证明只能返还一次。
pub(super) fn take_funding_command_id(return_mail_id: Uuid) -> Uuid {
    stable_command(return_mail_id, TAKE_FUNDING_LABEL).as_uuid()
}

fn stable_command(return_mail_id: Uuid, label: &str) -> CommandId {
    CommandId::from_uuid(deterministic_uuid(CommandId::from_uuid(return_mail_id), label, 1))
}

/// 持久 Take 动作行。
#[derive(Debug, Clone)]
pub(super) struct ActionRow {
    pub(super) return_mail_id: Uuid,
    pub(super) world_id: String,
    pub(super) source_external_mail_id: String,
    pub(super) expected_credits: i64,
    pub(super) expected_cod: i64,
    pub(super) expected_attachment_ids: Vec<String>,
    pub(super) execution_state: String,
    pub(super) delivery_id: Option<Uuid>,
    pub(super) prepare_command_id: Option<Uuid>,
    pub(super) prepare_request_digest: Option<Vec<u8>>,
    pub(super) activate_command_id: Option<Uuid>,
    pub(super) activate_request_digest: Option<Vec<u8>>,
    pub(super) state_version: i64,
}

/// 动作行的 SELECT 片段。
macro_rules! action_select {
    () => {
        r#"SELECT return_mail_id, world_id, source_external_mail_id, expected_credits,
                  expected_cod, expected_attachment_ids, execution_state, delivery_id,
                  prepare_command_id, prepare_request_digest, activate_command_id,
                  activate_request_digest, state_version
           FROM public.settlement_return_actions"#
    };
}

/// 读取（`lock` 时锁定）案件的 Take 动作。
pub(super) async fn load_action(
    tx: &mut Transaction<'_, Postgres>,
    return_mail_id: Uuid,
    lock: bool,
) -> Result<Option<ActionRow>, ReturnTransactionError> {
    let query = if lock {
        concat!(action_select!(), " WHERE return_mail_id = $1 FOR UPDATE")
    } else {
        concat!(action_select!(), " WHERE return_mail_id = $1")
    };
    let row = sqlx::query(query).bind(return_mail_id).fetch_optional(&mut **tx).await?;
    row.map(|row| {
        Ok(ActionRow {
            return_mail_id: row.try_get("return_mail_id")?,
            world_id: row.try_get("world_id")?,
            source_external_mail_id: row.try_get("source_external_mail_id")?,
            expected_credits: row.try_get("expected_credits")?,
            expected_cod: row.try_get("expected_cod")?,
            expected_attachment_ids: row.try_get("expected_attachment_ids")?,
            execution_state: row.try_get("execution_state")?,
            delivery_id: row.try_get("delivery_id")?,
            prepare_command_id: row.try_get("prepare_command_id")?,
            prepare_request_digest: row.try_get("prepare_request_digest")?,
            activate_command_id: row.try_get("activate_command_id")?,
            activate_request_digest: row.try_get("activate_request_digest")?,
            state_version: row.try_get("state_version")?,
        })
    })
    .transpose()
}

/// Admin 领回命令同事务建立 pending Take：预期资产取自已认证证据，等于原派发冻结资产。
pub(super) async fn insert_action(
    tx: &mut Transaction<'_, Postgres>,
    case: &CaseRow,
    assets: &SettlementReturnAssets,
    now: Timestamp,
) -> Result<(), ReturnTransactionError> {
    sqlx::query(
        r#"INSERT INTO public.settlement_return_actions
               (return_mail_id, world_id, source_external_mail_id, expected_credits,
                expected_cod, expected_attachment_ids, execution_state, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7, $7)"#,
    )
    .bind(case.return_mail_id)
    .bind(&case.world_id)
    .bind(&case.external_mail_id)
    .bind(assets.credits)
    .bind(assets.cod)
    .bind(&assets.attachment_ids)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 冻结进 Take 请求摘要的字段；顺序决定摘要字节，不可调整。
#[derive(Serialize)]
struct TakeRequest<'a> {
    return_mail_id: Uuid,
    world_id: &'a str,
    source_external_mail_id: &'a str,
    expected_credits: i64,
    expected_cod: i64,
    expected_attachment_ids: &'a [String],
}

/// 冻结进 Take 激活摘要的字段；顺序决定摘要字节，不可调整。
#[derive(Serialize)]
struct TakeActivation<'a> {
    schema_version: u8,
    delivery_id: Uuid,
    return_mail_id: Uuid,
    prepare_request_digest: &'a [u8],
}

/// 由动作冻结事实重建 Take 投递命令：收件人字段在 Take 中匹配入站发件人，系统退件固定为
/// `MoogleMail`，不是原收件人。
pub(super) fn take_command(
    action: &ActionRow,
) -> Result<MailDeliveryCommand, ReturnTransactionError> {
    let request_hash = RequestHash::from_request(
        "settlement_return_take",
        action.return_mail_id,
        &TakeRequest {
            return_mail_id: action.return_mail_id,
            world_id: &action.world_id,
            source_external_mail_id: &action.source_external_mail_id,
            expected_credits: action.expected_credits,
            expected_cod: action.expected_cod,
            expected_attachment_ids: &action.expected_attachment_ids,
        },
    )
    .ok()
    .ok_or_else(|| repository_error("结算退件 Take 请求摘要编码失败"))?;
    Ok(MailDeliveryCommand {
        command_id: take_command_id(action.return_mail_id),
        request_hash,
        origin: MailDeliveryOrigin::SettlementReturnAction,
        origin_id: take_origin_id(action.return_mail_id),
        operation: MailDeliveryOperation::Take,
        world_id: action.world_id.clone(),
        external_mail_id: action.source_external_mail_id.clone(),
        recipient: SYSTEM_RETURN_SENDER.to_owned(),
        template: None,
        result_code: None,
        user_key_user_id: None,
        credits: action.expected_credits,
        cod: action.expected_cod,
        attachment_ids: action.expected_attachment_ids.clone(),
        notification_body: None,
    })
}

/// Take 激活身份：激活命令 ID 固定，摘要绑定投递与 prepare 摘要。
pub(super) fn take_activation(
    action: &ActionRow,
    delivery_id: Uuid,
    prepare_request_hash: RequestHash,
    activated_at: Timestamp,
) -> Result<MailDeliveryActivation, ReturnTransactionError> {
    let prepare_digest = prepare_request_hash.as_bytes();
    let activate_request_hash = RequestHash::from_request(
        "settlement_return_take_activation",
        action.return_mail_id,
        &TakeActivation {
            schema_version: 1,
            delivery_id,
            return_mail_id: action.return_mail_id,
            prepare_request_digest: prepare_digest.as_slice(),
        },
    )
    .ok()
    .ok_or_else(|| repository_error("结算退件 Take 激活摘要编码失败"))?;
    Ok(MailDeliveryActivation {
        delivery_id,
        prepare_command_id: take_command_id(action.return_mail_id),
        prepare_request_hash,
        activate_command_id: take_activate_command_id(action.return_mail_id),
        activate_request_hash,
        activated_at,
    })
}

/// 终态 Take 投递的可验证结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TakeOutcome {
    /// 精确领取证明成立；摘要绑定证明形状、投递、世界、MID 与预期资产。`proof` 为
    /// [`TAKE_RECEIPT_PROOF`] 或 [`TAKE_SWEEP_PROOF`]。
    Proven { proof_digest: [u8; 32], external_reference: String, proof: &'static str },
    /// 投递终态 failed，且持久事实证明确定未外写：结果明确记录未开始写出、不是未知、从未因结果
    /// 未知被巡检重排、没有远端引用。邮件未被触碰，案件 owner 可在锁内复核后重新排队同一 Take。
    NotStarted { failure_code: &'static str },
    /// 投递失败但不能证明未外写：领取未发生或无法证明，转人工核对。
    Failed { failure_code: &'static str },
    /// 结果未知：转人工核对，只认系统可验证证明，不自动重试。
    Unknown { failure_code: &'static str },
    /// 投递仍在推进。
    Pending,
}

#[derive(Serialize)]
struct TakeProofPayload<'a> {
    schema_version: u8,
    kind: &'static str,
    delivery_id: String,
    return_mail_id: String,
    external_reference: &'a str,
    world: &'a str,
    source_external_mail_id: &'a str,
    expected_credits: i64,
    expected_cod: i64,
    expected_attachment_ids: &'a [String],
}

/// 读取 Take 投递并判定结论；投递身份、载荷或证明任何一项与动作冻结事实不符都不算证明成立。
pub(super) async fn observe_take(
    tx: &mut Transaction<'_, Postgres>,
    action: &ActionRow,
) -> Result<TakeOutcome, ReturnTransactionError> {
    let Some(delivery_id) = action.delivery_id else {
        return Ok(TakeOutcome::Unknown { failure_code: "settlement_return_take_conflict" });
    };
    let row = sqlx::query(
        r#"SELECT status, command_id, request_digest, origin_kind, origin_id, recipient,
                  payload, activation_command_id, activation_request_digest,
                  external_reference, result_metadata
           FROM public.deliveries WHERE id = $1 FOR UPDATE"#,
    )
    .bind(delivery_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| repository_error("结算退件 Take 投递不存在"))?;
    let status: String = row.try_get("status")?;
    let command = take_command(action)?;
    let identity_matches = row.try_get::<Uuid, _>("command_id")? == command.command_id.as_uuid()
        && action.prepare_command_id == Some(command.command_id.as_uuid())
        && row.try_get::<Vec<u8>, _>("request_digest")?.as_slice()
            == command.request_hash.as_bytes().as_slice()
        && action.prepare_request_digest.as_deref()
            == Some(command.request_hash.as_bytes().as_slice())
        && row.try_get::<String, _>("origin_kind")? == command.origin.as_str()
        && row.try_get::<String, _>("origin_id")? == command.origin_id
        && row.try_get::<String, _>("recipient")? == command.recipient
        && row.try_get::<serde_json::Value, _>("payload")?
            == crate::database::mail::delivery::payload(&command)
        && row.try_get::<Option<Uuid>, _>("activation_command_id")? == action.activate_command_id
        && row.try_get::<Option<Vec<u8>>, _>("activation_request_digest")?
            == action.activate_request_digest;
    if !identity_matches {
        return Ok(TakeOutcome::Unknown { failure_code: "settlement_return_take_conflict" });
    }
    let reference: Option<String> = row.try_get("external_reference")?;
    let metadata: serde_json::Value = row.try_get("result_metadata")?;
    match status.as_str() {
        "pending" | "claimed" | "sending" => return Ok(TakeOutcome::Pending),
        "failed" => {
            // 持久发送围栏与尝试历史：只有 `write_started` 与 `outcome_unknown` 都是明确的布尔
            // false、从未因未知结果被巡检重排、没有远端引用才算确定未外写；缺键、null 或错误类型
            // 都按可能已执行，与重启 SQL 和交接谓词同一口径。其余失败与未知同样只认领取证明。
            let not_started = reference.is_none()
                && metadata.get("write_started") == Some(&serde_json::Value::Bool(false))
                && metadata.get("outcome_unknown") == Some(&serde_json::Value::Bool(false))
                && metadata.get("sweep_retries").is_none();
            return Ok(if not_started {
                TakeOutcome::NotStarted { failure_code: "settlement_return_take_not_started" }
            } else {
                TakeOutcome::Failed { failure_code: "settlement_return_take_failed" }
            });
        }
        "succeeded" => {}
        _ => return Ok(TakeOutcome::Unknown { failure_code: "settlement_return_take_unknown" }),
    }
    let expected_reference =
        format!("mooglemail:{}:take:{}", action.world_id, action.source_external_mail_id);
    let mid = Some(action.source_external_mail_id.as_str());
    // 两种证明形状各自的身份字段：成功页回执带 `external_mail_id`；巡检恢复带 `recovered_mid`，
    // 且只认 integrations 核对过领取回执的完成观察。
    let proof = match metadata.get("proof").and_then(serde_json::Value::as_str) {
        Some(TAKE_RECEIPT_PROOF)
            if metadata.get("external_mail_id").and_then(serde_json::Value::as_str) == mid =>
        {
            Some((TAKE_RECEIPT_PROOF, "settlement_return_take_succeeded"))
        }
        Some(TAKE_SWEEP_PROOF)
            if metadata.get("recovered_mid").and_then(serde_json::Value::as_str) == mid
                && metadata.get("reason").and_then(serde_json::Value::as_str)
                    == Some(TAKE_SWEEP_REASON)
                && metadata
                    .get("external_reference_proven")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true) =>
        {
            Some((TAKE_SWEEP_PROOF, "settlement_return_take_recovered"))
        }
        _ => None,
    };
    let Some((proof, proof_kind)) = proof else {
        return Ok(TakeOutcome::Unknown { failure_code: "settlement_return_take_unknown" });
    };
    let proven = reference.as_deref() == Some(expected_reference.as_str())
        && metadata.get("operation").and_then(serde_json::Value::as_str) == Some("take")
        && metadata.get("expected_credits").and_then(serde_json::Value::as_i64)
            == Some(action.expected_credits)
        && metadata.get("expected_cod").and_then(serde_json::Value::as_i64)
            == Some(action.expected_cod)
        && metadata
            .get("expected_attachment_ids")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|values| {
                values.len() == action.expected_attachment_ids.len()
                    && values
                        .iter()
                        .zip(&action.expected_attachment_ids)
                        .all(|(value, expected)| value.as_str() == Some(expected.as_str()))
            });
    if !proven {
        return Ok(TakeOutcome::Unknown { failure_code: "settlement_return_take_unknown" });
    }
    let proof_digest = RequestHash::from_legacy_go_json(&TakeProofPayload {
        schema_version: 1,
        kind: proof_kind,
        delivery_id: delivery_id.to_string(),
        return_mail_id: action.return_mail_id.to_string(),
        external_reference: &expected_reference,
        world: &action.world_id,
        source_external_mail_id: &action.source_external_mail_id,
        expected_credits: action.expected_credits,
        expected_cod: action.expected_cod,
        expected_attachment_ids: &action.expected_attachment_ids,
    })
    .ok()
    .ok_or_else(|| repository_error("结算退件领回证明摘要编码失败"))?
    .as_bytes();
    Ok(TakeOutcome::Proven { proof_digest, external_reference: expected_reference, proof })
}
