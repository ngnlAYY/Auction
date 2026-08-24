//! MoogleMail delivery outbox 的远端执行适配器。
//!
//! 入站业务只把稳定 command 和无密文 payload 写入 outbox；本模块在 Worker 中使用 Bot Cookie
//! 执行受限的通知、结算发送、退回和领取动作。退回/领取必须同时通过动作前身份与资产栅栏、
//! 动作后收件箱或回执证明；结算
//! `send` 即使完成 compose 成功页，也要等待后续 Sent/余额证据才能收口履约义务。

use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

use auction_application::{
    ApplicationError, DeliverySendResult, DeliveryTransport, DeliveryTransportError,
    DeliveryWorkClaim, MailLoginKey, MailLoginKeyReader, MailProcessingError,
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::{
    MoogleMailActionState, MoogleMailClient, MoogleMailCredentialSet, MoogleMailError,
    MoogleMailErrorCategory, MoogleMailForm, MoogleMailMessageFields, MoogleMailOperation,
    MoogleMailParser, MoogleMailSendOutcome, MoogleMailSendReceipt, MoogleMailSendRequest,
    MoogleMailSender, MoogleMailWorld,
    protocol::{valid_decimal_id, valid_line, valid_opaque_id},
};
const SCHEMA_VERSION: u8 = 1;
const MAX_ATTACHMENT_IDS: usize = 1000;

/// PostgreSQL outbox 中版本化的 MoogleMail payload。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MailDeliveryPayload {
    schema_version: u8,
    operation: String,
    world: String,
    external_mail_id: String,
    template: Option<String>,
    result_code: Option<String>,
    body: Option<String>,
    user_key_user_id: Option<Uuid>,
    credits: i64,
    cod: i64,
    attachment_ids: Vec<String>,
}

/// take 动作必须在写入前核对的入站资产证据。
#[derive(Debug)]
struct ActionExpectation {
    credits: i64,
    cod: i64,
    attachment_ids: Vec<String>,
}

/// 结算履约 owner 生成的无密文 MoogleMail 意图。
///
/// 这里保留业务字段是为了让 transport 在外部写入前重新校验种类矩阵；装备 key 仍只存在
/// 数据库密文信封中，本次发送只使用同一快照中的远端装备 ID。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettlementDeliveryPayload {
    schema_version: u8,
    operation: String,
    world: String,
    obligation_id: Uuid,
    auction_id: Uuid,
    lot_id: Uuid,
    origin_command_id: Uuid,
    kind: String,
    recipient: String,
    template: String,
    item_name: String,
    external_equipment_id: String,
    amount: i64,
    carrier_credit: u32,
    credits: i64,
    cod: i64,
    equipment_id: Uuid,
    submission_id: Option<Uuid>,
    submission_source_kind: Option<String>,
    source_external_mail_id: Option<String>,
}

const MOOGLEMAIL_TEMPLATE_KEYS: &[&str] = &[
    "auction_assets_rejected",
    "auction_no_attachments",
    "auction_equipment_rejected",
    "user_assets_rejected",
    "register_key",
    "register_result",
    "rebind_result",
    "settlement_winner_equipment_cod",
    "settlement_payment_carrier",
    "settlement_payment_reminder",
    "settlement_equipment_handoff",
    "settlement_seller_payout",
    "settlement_buyer_refund",
    "operations_alert",
];

/// Worker 启动时冻结的 MoogleMail 通知模板快照。
///
/// 快照只保存经过闭集键名和占位符校验的正文；发送过程中不再读取可变配置，避免同一
/// 批次出现部分旧模板、部分新模板的结果。
#[derive(Debug, Clone)]
pub struct MoogleMailNotificationTemplates {
    values: BTreeMap<String, String>,
}

impl Default for MoogleMailNotificationTemplates {
    fn default() -> Self {
        Self::from_values(BTreeMap::from([
            ("auction_assets_rejected".to_owned(), "Auction assets rejected".to_owned()),
            ("auction_no_attachments".to_owned(), "Auction has no attachments".to_owned()),
            (
                "auction_equipment_rejected".to_owned(),
                "Auction equipment rejected: {reason_code}".to_owned(),
            ),
            ("user_assets_rejected".to_owned(), "User assets rejected".to_owned()),
            ("register_key".to_owned(), "Registration key {login_key} for {username}".to_owned()),
            (
                "register_result".to_owned(),
                "Registration result {result_code} {username}".to_owned(),
            ),
            ("rebind_result".to_owned(), "Rebind result {result_code} {username}".to_owned()),
            (
                "settlement_winner_equipment_cod".to_owned(),
                "Auction item {item_name}; COD {amount}; reference {reference}".to_owned(),
            ),
            (
                "settlement_payment_carrier".to_owned(),
                "Auction payment request {amount}; reference {reference}".to_owned(),
            ),
            (
                "settlement_payment_reminder".to_owned(),
                "Auction payment is overdue; reference {reference}".to_owned(),
            ),
            (
                "settlement_equipment_handoff".to_owned(),
                "Auction item {item_name}; reference {reference}".to_owned(),
            ),
            (
                "settlement_seller_payout".to_owned(),
                "Auction seller payout {amount}; reference {reference}".to_owned(),
            ),
            (
                "settlement_buyer_refund".to_owned(),
                "Auction buyer refund {amount}; reference {reference}".to_owned(),
            ),
            ("operations_alert".to_owned(), "Operations alert: {summary}".to_owned()),
        ]))
        .expect("内置 MoogleMail 模板必须通过启动校验")
    }
}

impl MoogleMailNotificationTemplates {
    /// 从配置快照构造模板，并在 Worker 启动阶段拒绝未知键、非法正文和未知占位符。
    pub fn from_values(values: BTreeMap<String, String>) -> Result<Self, String> {
        if values.len() != MOOGLEMAIL_TEMPLATE_KEYS.len()
            || MOOGLEMAIL_TEMPLATE_KEYS.iter().any(|key| !values.contains_key(*key))
        {
            return Err("MoogleMail mail_templates 必须包含且仅包含旧版闭集模板键".to_owned());
        }
        for key in MOOGLEMAIL_TEMPLATE_KEYS {
            let value = values.get(*key).expect("已通过模板键闭集检查");
            validate_template_body(key, value)?;
        }
        Ok(Self { values })
    }

    fn render(&self, key: &str, variables: &[(&str, &str)]) -> Option<String> {
        let template = self.values.get(key)?;
        let mut output = String::with_capacity(template.len());
        let mut cursor = 0;
        while let Some(relative_start) = template[cursor..].find('{') {
            let start = cursor + relative_start;
            let end = start + 1 + template[start + 1..].find('}')?;
            let placeholder = &template[start + 1..end];
            let value = variables.iter().find(|(name, _)| *name == placeholder)?.1;
            output.push_str(&template[cursor..start]);
            output.push_str(value);
            cursor = end + 1;
        }
        output.push_str(&template[cursor..]);
        Some(output)
    }
}

fn validate_template_body(key: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value != value.trim()
        || value.len() > 64 * 1024
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(format!("MoogleMail mail_templates.{key} 正文格式无效"));
    }
    let (required, allowed): (&[&str], &[&str]) = match key {
        "auction_assets_rejected" | "auction_no_attachments" | "user_assets_rejected" => (&[], &[]),
        "auction_equipment_rejected" => (&["reason_code"], &["reason_code"]),
        "register_key" => (&["login_key", "username"], &["login_key", "username"]),
        "register_result" | "rebind_result" => {
            (&["result_code", "username"], &["result_code", "username"])
        }
        "settlement_winner_equipment_cod" => {
            (&["item_name", "amount", "reference"], &["item_name", "amount", "reference"])
        }
        "settlement_payment_carrier" | "settlement_seller_payout" | "settlement_buyer_refund" => {
            (&["amount", "reference"], &["amount", "reference"])
        }
        "settlement_payment_reminder" => (&["reference"], &["reference"]),
        "settlement_equipment_handoff" => {
            (&["item_name", "reference"], &["item_name", "reference"])
        }
        "operations_alert" => (&["summary"], &["summary"]),
        _ => return Err("MoogleMail mail_templates 含有未知键".to_owned()),
    };
    let mut cursor = 0;
    let mut seen = Vec::new();
    while let Some(relative_start) = value[cursor..].find('{') {
        let start = cursor + relative_start;
        let relative_end = value[start + 1..]
            .find('}')
            .ok_or_else(|| format!("MoogleMail mail_templates.{key} 占位符未闭合"))?;
        let end = start + 1 + relative_end;
        let placeholder = &value[start + 1..end];
        if !allowed.contains(&placeholder) {
            return Err(format!("MoogleMail mail_templates.{key} 含有未知占位符"));
        }
        if !seen.contains(&placeholder) {
            seen.push(placeholder);
        }
        cursor = end + 1;
    }
    if value[cursor..].contains('}')
        || required.iter().any(|placeholder| !seen.contains(placeholder))
    {
        return Err(format!("MoogleMail mail_templates.{key} 缺少必需占位符"));
    }
    Ok(())
}

/// MoogleMail delivery 的受控远端 transport。
pub struct MoogleMailDeliveryTransport {
    client: MoogleMailClient,
    credentials: MoogleMailCredentialSet,
    sender: MoogleMailSender,
    parser: MoogleMailParser,
    login_keys: Arc<dyn MailLoginKeyReader>,
    templates: MoogleMailNotificationTemplates,
}

impl MoogleMailDeliveryTransport {
    /// 使用已经由配置层校验的 MoogleMail client 和凭据构造 transport。
    pub fn new(
        client: MoogleMailClient,
        credentials: MoogleMailCredentialSet,
        login_keys: Arc<dyn MailLoginKeyReader>,
    ) -> Self {
        Self::with_templates(
            client,
            credentials,
            login_keys,
            MoogleMailNotificationTemplates::default(),
        )
    }

    /// 使用指定的已校验模板快照构造 transport。
    pub fn with_templates(
        client: MoogleMailClient,
        credentials: MoogleMailCredentialSet,
        login_keys: Arc<dyn MailLoginKeyReader>,
        templates: MoogleMailNotificationTemplates,
    ) -> Self {
        let sender = MoogleMailSender::new(client.clone());
        Self { client, credentials, sender, parser: MoogleMailParser, login_keys, templates }
    }

    async fn send_claim(
        &self,
        claim: DeliveryWorkClaim,
    ) -> Result<DeliverySendResult, DeliveryTransportError> {
        match claim.origin_kind.as_str() {
            "mail_intake" => {
                let (payload, world) = decode_payload(&claim)?;
                match payload.operation.as_str() {
                    "send" => self.send_notification(&claim, &payload, world).await,
                    "return" => {
                        self.send_action(
                            &claim,
                            &payload.external_mail_id,
                            world,
                            MoogleMailOperation::ReturnMessage,
                            None,
                        )
                        .await
                    }
                    "take" => {
                        self.send_action(
                            &claim,
                            &payload.external_mail_id,
                            world,
                            MoogleMailOperation::Take,
                            Some(ActionExpectation {
                                credits: payload.credits,
                                cod: payload.cod,
                                attachment_ids: payload.attachment_ids.clone(),
                            }),
                        )
                        .await
                    }
                    _ => Err(invalid_payload()),
                }
            }
            "settlement" => {
                let (payload, world) = decode_settlement_payload(&claim)?;
                match payload.operation.as_str() {
                    "send" => self.send_settlement(&claim, &payload, world).await,
                    "return" => {
                        let external_mail_id = payload
                            .source_external_mail_id
                            .as_deref()
                            .ok_or_else(invalid_payload)?;
                        self.send_action(
                            &claim,
                            external_mail_id,
                            world,
                            MoogleMailOperation::ReturnMessage,
                            None,
                        )
                        .await
                    }
                    _ => Err(invalid_payload()),
                }
            }
            "settlement_recovery_action" => {
                let (payload, world) = decode_payload(&claim)?;
                match payload.operation.as_str() {
                    "send" => self.send_notification(&claim, &payload, world).await,
                    "take" => {
                        self.send_action(
                            &claim,
                            &payload.external_mail_id,
                            world,
                            MoogleMailOperation::Take,
                            Some(ActionExpectation {
                                credits: payload.credits,
                                cod: payload.cod,
                                attachment_ids: payload.attachment_ids.clone(),
                            }),
                        )
                        .await
                    }
                    _ => Err(invalid_payload()),
                }
            }
            "operations_alert" => {
                let (payload, world) = decode_payload(&claim)?;
                if payload.operation != "send" {
                    return Err(invalid_payload());
                }
                self.send_notification(&claim, &payload, world).await
            }
            _ => Err(invalid_payload()),
        }
    }

    async fn send_settlement(
        &self,
        claim: &DeliveryWorkClaim,
        payload: &SettlementDeliveryPayload,
        world: MoogleMailWorld,
    ) -> Result<DeliverySendResult, DeliveryTransportError> {
        let (subject, body) =
            render_settlement(payload, &self.templates).ok_or_else(invalid_payload)?;
        let outcome = self
            .send_with_credentials(MoogleMailSendRequest {
                world,
                message: MoogleMailMessageFields {
                    recipient: claim.recipient.clone(),
                    subject,
                    body,
                },
                credits: payload.credits,
                cod: payload.cod,
                equipment_ids: match payload.template.as_str() {
                    "settlement_winner_equipment_cod" | "settlement_equipment_handoff" => {
                        vec![payload.external_equipment_id.clone()]
                    }
                    _ => Vec::new(),
                },
            })
            .await
            .map_err(|error| transport_error(error, false))?;
        let receipt = send_receipt(outcome)?;
        let external_reference_proven = receipt.external_mail_id.is_some();
        let external_reference =
            receipt.external_mail_id.unwrap_or_else(|| format!("delivery:{}", claim.delivery_id));
        Ok(DeliverySendResult {
            external_reference,
            result_metadata: json!({
                "operation": "send",
                "template": payload.template,
                "external_reference_proven": external_reference_proven,
                "settlement_obligation_id": payload.obligation_id,
            }),
        })
    }

    async fn send_notification(
        &self,
        claim: &DeliveryWorkClaim,
        payload: &MailDeliveryPayload,
        world: MoogleMailWorld,
    ) -> Result<DeliverySendResult, DeliveryTransportError> {
        let template = payload.template.as_deref().ok_or_else(invalid_payload)?;
        let result_code = payload.result_code.as_deref().ok_or_else(invalid_payload)?;
        let login_key = match payload.user_key_user_id {
            Some(user_id) => Some(self.login_keys.read(user_id).await.map_err(login_key_error)?),
            None => None,
        };
        let (subject, body) = if template == "operations_alert" && result_code == "operations_alert"
        {
            let summary = payload.body.as_deref().ok_or_else(invalid_payload)?;
            (
                "Auction operations alert".to_owned(),
                self.templates
                    .render("operations_alert", &[("summary", summary)])
                    .ok_or_else(invalid_payload)?,
            )
        } else {
            render_notification(
                &self.templates,
                template,
                result_code,
                login_key.as_ref(),
                &claim.recipient,
                &payload.external_mail_id,
            )
            .ok_or_else(invalid_payload)?
        };
        if !valid_line(&subject, 512, false) || !valid_line(&body, 64 * 1024, true) {
            return Err(invalid_payload());
        }
        let message = MoogleMailMessageFields { recipient: claim.recipient.clone(), subject, body };
        let outcome = self
            .send_with_credentials(MoogleMailSendRequest {
                world,
                message,
                credits: 0,
                cod: 0,
                equipment_ids: Vec::new(),
            })
            .await
            .map_err(|error| transport_error(error, false))?;
        let receipt = send_receipt(outcome)?;
        let external_reference_proven = receipt.external_mail_id.is_some();
        let external_reference =
            receipt.external_mail_id.unwrap_or_else(|| format!("delivery:{}", claim.delivery_id));
        Ok(DeliverySendResult {
            external_reference,
            result_metadata: json!({
                "operation": "send",
                "template": template,
                "external_reference_proven": external_reference_proven,
            }),
        })
    }

    /// 读侧 session 失效时轮换并回写当前凭据；写侧失效只回写，不发起第二次 compose。
    async fn send_with_credentials(
        &self,
        request: MoogleMailSendRequest,
    ) -> Result<MoogleMailSendOutcome, MoogleMailError> {
        let mut session = self.credentials.begin(request.world)?;
        for _ in 0..64 {
            match self.sender.send(session.cookie(), request.clone()).await {
                Ok(receipt) => return Ok(receipt),
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    if error.write_started() {
                        let _ = session.invalidate_current().await;
                        return Err(error);
                    }
                    session.invalidate_current().await?;
                    session.advance()?;
                }
                Err(error) => return Err(error),
            }
        }
        Err(MoogleMailError::new(
            MoogleMailOperation::Send,
            request.world,
            MoogleMailErrorCategory::CredentialUnavailable,
            false,
        ))
    }

    async fn send_action(
        &self,
        claim: &DeliveryWorkClaim,
        external_mail_id: &str,
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
        expected: Option<ActionExpectation>,
    ) -> Result<DeliverySendResult, DeliveryTransportError> {
        let target = self
            .client
            .detail(world, external_mail_id)
            .map_err(|error| transport_error(error, false))?;
        let mut session =
            self.credentials.begin(world).map_err(|error| transport_error(error, false))?;
        for _ in 0..64 {
            let before = match self.client.get(&target, session.cookie()).await {
                Ok(document) => document,
                Err(error) if error.is_session_expired() => {
                    session
                        .invalidate_current()
                        .await
                        .map_err(|error| transport_error(error, false))?;
                    session.advance().map_err(|error| transport_error(error, false))?;
                    continue;
                }
                Err(error) => return Err(transport_error(error, false)),
            };
            let state =
                self.parser.parse_action_state(&before, world).map_err(|_| preflight_unproven())?;
            let preflight_proven = state.sender().eq_ignore_ascii_case(&claim.recipient)
                && match operation {
                    MoogleMailOperation::ReturnMessage => state.returnable && expected.is_none(),
                    MoogleMailOperation::Take => {
                        state.claimable
                            && expected
                                .as_ref()
                                .is_some_and(|expected| exact_action_expectation(&state, expected))
                    }
                    _ => false,
                };
            if !preflight_proven {
                return Err(preflight_unproven());
            }
            let form = match operation {
                MoogleMailOperation::ReturnMessage => MoogleMailForm::return_message(state.token()),
                MoogleMailOperation::Take => MoogleMailForm::take(state.token()),
                _ => unreachable!("validated MoogleMail action operation"),
            };
            let form = form.map_err(|error| transport_error(error, false))?;
            let response = match self.client.post(&target, session.cookie(), form).await {
                Ok(response) => response,
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    let _ = session.invalidate_current().await;
                    return Err(transport_error(error, true));
                }
                Err(error) => return Err(transport_error(error, true)),
            };
            match operation {
                MoogleMailOperation::ReturnMessage => {
                    self.parser.prove_return_completed(&response, world, external_mail_id)
                }
                MoogleMailOperation::Take => {
                    self.parser.prove_take_completed(&state, &response, world)
                }
                _ => unreachable!("validated MoogleMail action operation"),
            }
            .map_err(|_| action_proof_unproven())?;
            let mut result_metadata = json!({
                "operation": action_name(operation),
                "external_mail_id": external_mail_id,
                "proof": "action_removed_or_success_page",
            });
            if let Some(expected) = expected.as_ref() {
                result_metadata["expected_credits"] = expected.credits.into();
                result_metadata["expected_cod"] = expected.cod.into();
                result_metadata["expected_attachment_ids"] = serde_json::Value::Array(
                    expected.attachment_ids.iter().cloned().map(Into::into).collect(),
                );
            }
            return Ok(DeliverySendResult {
                external_reference: format!(
                    "mooglemail:{}:{}:{}",
                    world,
                    action_name(operation),
                    external_mail_id
                ),
                result_metadata,
            });
        }
        Err(DeliveryTransportError {
            failure_code: "credential_unavailable".to_owned(),
            safe_detail: "MoogleMail 没有可用凭据".to_owned(),
            outcome_unknown: false,
            external_reference: None,
            result_metadata: json!({}),
        })
    }
}

impl DeliveryTransport for MoogleMailDeliveryTransport {
    fn available(&self, claim: &DeliveryWorkClaim) -> bool {
        let world = match claim.origin_kind.as_str() {
            "settlement" => decode_settlement_payload(claim).map(|(_, world)| world),
            "mail_intake" | "settlement_recovery_action" | "operations_alert" => {
                decode_payload(claim).map(|(_, world)| world)
            }
            // 非法 owner/payload 必须进入正式发送路径并确定失败，不能被 availability gate
            // 永久留在 pending。
            _ => return true,
        };
        world.map_or(true, |world| self.credentials.available(world))
    }

    fn send(
        &self,
        claim: DeliveryWorkClaim,
    ) -> Pin<Box<dyn Future<Output = Result<DeliverySendResult, DeliveryTransportError>> + Send + '_>>
    {
        Box::pin(async move { self.send_claim(claim).await })
    }
}

fn decode_payload(
    claim: &DeliveryWorkClaim,
) -> Result<(MailDeliveryPayload, MoogleMailWorld), DeliveryTransportError> {
    if !matches!(
        claim.origin_kind.as_str(),
        "mail_intake" | "settlement_recovery_action" | "operations_alert"
    ) || claim.delivery_id.is_nil()
        || !valid_line(&claim.recipient, 256, false)
        || !valid_line(&claim.origin_id, 512, false)
    {
        return Err(invalid_payload());
    }
    let payload: MailDeliveryPayload =
        serde_json::from_value(claim.payload.clone()).map_err(|_| invalid_payload())?;
    if payload.schema_version != SCHEMA_VERSION
        || !valid_line(&payload.world, 16, false)
        || !valid_line(&payload.external_mail_id, 256, false)
        || !valid_opaque_id(&payload.external_mail_id)
        || payload.credits < 0
        || payload.cod < 0
        || payload.attachment_ids.len() > MAX_ATTACHMENT_IDS
        || payload.attachment_ids.iter().any(|id| !valid_decimal_id(id))
        || payload
            .attachment_ids
            .iter()
            .enumerate()
            .any(|(index, id)| payload.attachment_ids[..index].contains(id))
    {
        return Err(invalid_payload());
    }
    let world = MoogleMailWorld::parse(&payload.world).ok_or_else(invalid_payload)?;
    let expected_origin_id =
        format!("{}:{}:{}", payload.world, payload.external_mail_id, payload.operation);
    if claim.origin_kind == "mail_intake" && claim.origin_id != expected_origin_id {
        return Err(invalid_payload());
    }
    if claim.origin_kind == "operations_alert"
        && (claim.origin_id
            != payload
                .external_mail_id
                .strip_prefix("operations-alert:")
                .map(|value| format!("operations_alert:{value}"))
                .unwrap_or_default()
            || !valid_notification_body(payload.body.as_deref()))
    {
        return Err(invalid_payload());
    }
    if claim.origin_kind != "operations_alert" && payload.body.is_some() {
        return Err(invalid_payload());
    }
    if claim.origin_kind == "settlement_recovery_action" {
        let recovery_id =
            claim.origin_id.strip_prefix("recovery:").and_then(|value| Uuid::parse_str(value).ok());
        if recovery_id.is_none()
            || (payload.operation == "send" && payload.world != "main")
            || !matches!(payload.operation.as_str(), "send" | "take")
        {
            return Err(invalid_payload());
        }
    }
    match payload.operation.as_str() {
        "send" => {
            if payload.template.is_none()
                || payload.result_code.is_none()
                || payload.credits != 0
                || payload.cod != 0
                || !payload.attachment_ids.is_empty()
            {
                return Err(invalid_payload());
            }
            match payload.template.as_deref() {
                Some("register_key") => {
                    if claim.origin_kind != "mail_intake"
                        || payload.user_key_user_id.is_none()
                        || !matches!(
                            payload.result_code.as_deref(),
                            Some("registered" | "recovered")
                        )
                    {
                        return Err(invalid_payload());
                    }
                }
                Some("register_result") => {
                    if claim.origin_kind != "mail_intake"
                        || payload.user_key_user_id.is_some()
                        || !matches!(
                            payload.result_code.as_deref(),
                            Some("blacklisted" | "identity_conflict")
                        )
                    {
                        return Err(invalid_payload());
                    }
                }
                Some("rebind_result") => {
                    if claim.origin_kind != "mail_intake"
                        || payload.user_key_user_id.is_some()
                        || !matches!(
                            payload.result_code.as_deref(),
                            Some(
                                "rebound"
                                    | "unchanged"
                                    | "blacklisted"
                                    | "identity_conflict"
                                    | "invalid_key"
                                    | "missing_key"
                            )
                        )
                    {
                        return Err(invalid_payload());
                    }
                }
                Some(
                    "auction_assets_rejected"
                    | "auction_no_attachments"
                    | "auction_equipment_rejected"
                    | "user_assets_rejected",
                ) => {
                    if claim.origin_kind != "mail_intake" || payload.user_key_user_id.is_some() {
                        return Err(invalid_payload());
                    }
                }
                Some("recovery_payment_reminder") => {
                    if claim.origin_kind != "settlement_recovery_action"
                        || payload.result_code.as_deref() != Some("payment_overdue")
                        || payload.user_key_user_id.is_some()
                    {
                        return Err(invalid_payload());
                    }
                }
                Some("operations_alert") => {
                    if claim.origin_kind != "operations_alert"
                        || payload.result_code.as_deref() != Some("operations_alert")
                        || payload.user_key_user_id.is_some()
                        || !valid_notification_body(payload.body.as_deref())
                    {
                        return Err(invalid_payload());
                    }
                }
                _ => return Err(invalid_payload()),
            }
        }
        "return" => {
            if payload.template.is_some()
                || payload.result_code.is_some()
                || payload.credits != 0
                || payload.cod != 0
                || payload.user_key_user_id.is_some()
                || claim.origin_kind != "mail_intake"
            {
                return Err(invalid_payload());
            }
        }
        "take" => {
            if payload.template.is_some()
                || payload.result_code.is_some()
                || payload.user_key_user_id.is_some()
                || (claim.origin_kind == "mail_intake"
                    && (payload.credits != 0 || payload.cod != 0))
            {
                return Err(invalid_payload());
            }
        }
        _ => return Err(invalid_payload()),
    }
    Ok((payload, world))
}

fn decode_settlement_payload(
    claim: &DeliveryWorkClaim,
) -> Result<(SettlementDeliveryPayload, MoogleMailWorld), DeliveryTransportError> {
    if claim.delivery_id.is_nil() || !valid_line(&claim.recipient, 256, false) {
        return Err(invalid_payload());
    }
    let payload: SettlementDeliveryPayload =
        serde_json::from_value(claim.payload.clone()).map_err(|_| invalid_payload())?;
    if payload.schema_version != 2
        || payload.obligation_id.is_nil()
        || payload.auction_id.is_nil()
        || payload.lot_id.is_nil()
        || payload.origin_command_id.is_nil()
        || payload.equipment_id.is_nil()
        || payload.submission_id.is_some_and(|submission_id| submission_id.is_nil())
        || payload.recipient != claim.recipient
        || claim.origin_id != payload.obligation_id.to_string()
        || !valid_line(&payload.world, 16, false)
        || !valid_line(&payload.template, 128, false)
        || !valid_line(&payload.item_name, 512, false)
        || !valid_decimal_id(&payload.external_equipment_id)
        || payload.amount < 0
        || payload.credits < 0
        || payload.cod < 0
    {
        return Err(invalid_payload());
    }
    let world = MoogleMailWorld::parse(&payload.world).ok_or_else(invalid_payload)?;
    match payload.operation.as_str() {
        "send" => {
            if payload.submission_source_kind.is_some()
                && !matches!(payload.submission_source_kind.as_deref(), Some("mail" | "admin"))
                || payload.source_external_mail_id.is_some()
            {
                return Err(invalid_payload());
            }
            match payload.kind.as_str() {
                "winner_equipment_cod"
                    if payload.template == "settlement_winner_equipment_cod"
                        && world == MoogleMailWorld::Main
                        && payload.credits == 0
                        && payload.cod == payload.amount
                        && payload.amount > 0
                        && payload.carrier_credit == 0
                        && !payload.item_name.is_empty() => {}
                "winner_payment_carrier_cod"
                    if payload.template == "settlement_payment_carrier"
                        && world == MoogleMailWorld::Isekai
                        && payload.carrier_credit > 0
                        && payload.credits == i64::from(payload.carrier_credit)
                        && payload.amount > 0
                        && payload.item_name.is_empty() => {}
                "winner_equipment_handoff"
                    if payload.template == "settlement_equipment_handoff"
                        && payload.credits == 0
                        && payload.cod == 0
                        && payload.amount == 0
                        && payload.carrier_credit == 0
                        && !payload.item_name.is_empty() => {}
                "seller_payout"
                    if payload.template == "settlement_seller_payout"
                        && payload.credits == payload.amount
                        && payload.credits > 0
                        && payload.cod == 0
                        && payload.carrier_credit == 0
                        && payload.item_name.is_empty() => {}
                "buyer_refund"
                    if payload.template == "settlement_buyer_refund"
                        && payload.credits == payload.amount
                        && payload.credits > 0
                        && payload.cod == 0
                        && payload.carrier_credit == 0
                        && payload.item_name.is_empty() => {}
                "unsold_return" | "withdrawn_return"
                    if payload.template == "settlement_equipment_handoff"
                        && payload.credits == 0
                        && payload.cod == 0
                        && payload.amount == 0
                        && payload.carrier_credit == 0
                        && !payload.item_name.is_empty() => {}
                _ => return Err(invalid_payload()),
            }
            if payload.kind == "winner_payment_carrier_cod"
                && payload.cod
                    != payload.amount.checked_add(payload.credits).ok_or_else(invalid_payload)?
            {
                return Err(invalid_payload());
            }
        }
        "return" => {
            if !matches!(payload.kind.as_str(), "unsold_return" | "withdrawn_return")
                || !payload.template.is_empty()
                || !payload.item_name.is_empty()
                || payload.credits != 0
                || payload.cod != 0
                || payload.amount != 0
                || payload.carrier_credit != 0
                || payload.submission_source_kind.as_deref() != Some("mail")
                || payload
                    .source_external_mail_id
                    .as_deref()
                    .is_none_or(|value| !valid_opaque_id(value))
            {
                return Err(invalid_payload());
            }
        }
        _ => return Err(invalid_payload()),
    }
    Ok((payload, world))
}

fn render_settlement(
    payload: &SettlementDeliveryPayload,
    templates: &MoogleMailNotificationTemplates,
) -> Option<(String, String)> {
    let reference = payload.obligation_id.to_string();
    let amount = match payload.template.as_str() {
        "settlement_winner_equipment_cod" | "settlement_payment_carrier" => payload.cod,
        "settlement_seller_payout" | "settlement_buyer_refund" => payload.credits,
        "settlement_equipment_handoff" => 0,
        _ => return None,
    }
    .to_string();
    let (subject, body) = match payload.template.as_str() {
        "settlement_winner_equipment_cod" => (
            format!("Auction equipment delivery [{reference}]"),
            templates.render(
                "settlement_winner_equipment_cod",
                &[
                    ("item_name", &payload.item_name),
                    ("amount", &amount),
                    ("reference", &reference),
                ],
            )?,
        ),
        "settlement_payment_carrier" => (
            format!("Auction payment request [{reference}]"),
            templates.render(
                "settlement_payment_carrier",
                &[("amount", &amount), ("reference", &reference)],
            )?,
        ),
        "settlement_equipment_handoff" => (
            format!("Auction equipment delivery [{reference}]"),
            templates.render(
                "settlement_equipment_handoff",
                &[("item_name", &payload.item_name), ("reference", &reference)],
            )?,
        ),
        "settlement_seller_payout" => (
            "Auction seller payout".to_owned(),
            templates.render(
                "settlement_seller_payout",
                &[("amount", &amount), ("reference", &reference)],
            )?,
        ),
        "settlement_buyer_refund" => (
            "Auction buyer refund".to_owned(),
            templates.render(
                "settlement_buyer_refund",
                &[("amount", &amount), ("reference", &reference)],
            )?,
        ),
        "" => return None,
        _ => return None,
    };
    (valid_line(&subject, 512, false) && valid_line(&body, 64 * 1024, true))
        .then_some((subject, body))
}

fn exact_action_expectation(state: &MoogleMailActionState, expected: &ActionExpectation) -> bool {
    let expected_count =
        expected.attachment_ids.len().saturating_add(usize::from(expected.credits > 0));
    if state.credits() != expected.credits
        || state.cod() != expected.cod
        || state.attachment_count() != expected_count
        || state.equipment_ids().len() != expected.attachment_ids.len()
    {
        return false;
    }
    let mut observed_ids = state.equipment_ids().to_vec();
    let mut expected_ids = expected.attachment_ids.clone();
    observed_ids.sort_unstable();
    expected_ids.sort_unstable();
    observed_ids == expected_ids
}

fn send_receipt(
    outcome: MoogleMailSendOutcome,
) -> Result<MoogleMailSendReceipt, DeliveryTransportError> {
    match outcome {
        MoogleMailSendOutcome::Sent(receipt) => Ok(receipt),
        MoogleMailSendOutcome::PreflightUnproven => Err(DeliveryTransportError {
            failure_code: "preflight_unproven".to_owned(),
            safe_detail: "MoogleMail compose 初始状态无法证明为干净状态".to_owned(),
            outcome_unknown: true,
            external_reference: None,
            result_metadata: json!({
                "stage": "preflight",
                "category": "protocol_drift",
            }),
        }),
        MoogleMailSendOutcome::RecipientNotFound { returned_mail_id } => {
            let mut result_metadata = json!({
                "stage": "send",
                "category": "recipient_not_found",
                "reason_code": "recipient_not_found",
                "take_required": true,
            });
            if let Some(returned_mail_id) = returned_mail_id.as_ref() {
                result_metadata["returned_mail_id"] = returned_mail_id.clone().into();
            }
            Err(DeliveryTransportError {
                failure_code: "recipient_not_found".to_owned(),
                safe_detail: "MoogleMail 收件人不存在，需要核对远端退信".to_owned(),
                // 收件人不存在本身已有协议证明，但退信中的 Credits/装备尚未取回，
                // 不能把 delivery 终结为普通 failed。
                outcome_unknown: true,
                external_reference: returned_mail_id,
                result_metadata,
            })
        }
        MoogleMailSendOutcome::Rejected { assets_staged } => Err(DeliveryTransportError {
            failure_code: if assets_staged { "send_rejected_after_stage" } else { "send_rejected" }
                .to_owned(),
            safe_detail: "MoogleMail 远端明确拒绝发送".to_owned(),
            outcome_unknown: assets_staged,
            external_reference: None,
            result_metadata: json!({
                "stage": "send",
                "category": "write_rejected",
            }),
        }),
    }
}

fn preflight_unproven() -> DeliveryTransportError {
    DeliveryTransportError {
        failure_code: "preflight_unproven".to_owned(),
        safe_detail: "MoogleMail 动作前置身份或资产无法证明".to_owned(),
        outcome_unknown: true,
        external_reference: None,
        result_metadata: json!({}),
    }
}

fn action_proof_unproven() -> DeliveryTransportError {
    DeliveryTransportError {
        failure_code: "write_unknown".to_owned(),
        safe_detail: "MoogleMail 外部动作结果无法证明".to_owned(),
        outcome_unknown: true,
        external_reference: None,
        result_metadata: json!({}),
    }
}

fn action_name(operation: MoogleMailOperation) -> &'static str {
    match operation {
        MoogleMailOperation::ReturnMessage => "return",
        MoogleMailOperation::Take => "take",
        _ => "action",
    }
}

fn render_notification(
    templates: &MoogleMailNotificationTemplates,
    template: &str,
    result_code: &str,
    login_key: Option<&MailLoginKey>,
    recipient: &str,
    reference: &str,
) -> Option<(String, String)> {
    match (template, result_code) {
        ("auction_assets_rejected", "auction_assets_rejected") => Some((
            "Auction mail rejected".to_owned(),
            templates.render("auction_assets_rejected", &[])?,
        )),
        ("auction_no_attachments", "auction_no_attachments") => Some((
            "Auction mail rejected".to_owned(),
            templates.render("auction_no_attachments", &[])?,
        )),
        (
            "auction_equipment_rejected",
            "auction_equipment_rejected"
            | "start_price_invalid"
            | "start_price_missing"
            | "start_price_duplicate"
            | "start_price_identity_mismatch"
            | "start_price_attachment_not_found",
        ) => Some((
            "Auction mail rejected".to_owned(),
            templates.render("auction_equipment_rejected", &[("reason_code", result_code)])?,
        )),
        ("user_assets_rejected", "user_assets_rejected") => Some((
            "Account mail rejected".to_owned(),
            templates.render("user_assets_rejected", &[])?,
        )),
        ("register_key", "registered" | "recovered") => {
            let key = login_key?;
            let body = key.with_encoded(|encoded| {
                templates.render("register_key", &[("login_key", encoded), ("username", recipient)])
            })?;
            Some(("Auction login key".to_owned(), body))
        }
        ("register_result", "blacklisted" | "identity_conflict") => Some((
            "Registration result".to_owned(),
            templates.render(
                "register_result",
                &[("result_code", result_code), ("username", recipient)],
            )?,
        )),
        (
            "rebind_result",
            "rebound" | "unchanged" | "blacklisted" | "identity_conflict" | "invalid_key"
            | "missing_key",
        ) => Some((
            "Rebind result".to_owned(),
            templates.render(
                "rebind_result",
                &[("result_code", result_code), ("username", recipient)],
            )?,
        )),
        ("forum_auction_notice", "forum_auction_notice") => Some((
            "Forum auction notification".to_owned(),
            format!("Forum auction notification for {recipient}"),
        )),
        ("recovery_payment_reminder", "payment_overdue") => Some((
            "Auction payment overdue".to_owned(),
            templates.render("settlement_payment_reminder", &[("reference", reference)])?,
        )),
        _ => None,
    }
}

fn invalid_payload() -> DeliveryTransportError {
    DeliveryTransportError {
        failure_code: "invalid_payload".to_owned(),
        safe_detail: "MoogleMail delivery payload 无效".to_owned(),
        outcome_unknown: false,
        external_reference: None,
        result_metadata: json!({}),
    }
}

fn login_key_error(error: ApplicationError) -> DeliveryTransportError {
    match error {
        ApplicationError::MailProcessing(MailProcessingError::LoginKeyNotFound) => {
            DeliveryTransportError {
                failure_code: "secret_not_found".to_owned(),
                safe_detail: "MoogleMail 注册 key 不存在".to_owned(),
                outcome_unknown: false,
                external_reference: None,
                result_metadata: json!({
                    "stage": "secret_resolution",
                    "category": "missing_reference",
                }),
            }
        }
        ApplicationError::MailProcessing(MailProcessingError::InvalidLoginKey) => {
            DeliveryTransportError {
                failure_code: "invalid_secret".to_owned(),
                safe_detail: "MoogleMail 注册 key 无效".to_owned(),
                outcome_unknown: false,
                external_reference: None,
                result_metadata: json!({
                    "stage": "secret_resolution",
                    "category": "invalid_secret",
                }),
            }
        }
        _ => DeliveryTransportError {
            failure_code: "login_key_unavailable".to_owned(),
            safe_detail: "MoogleMail 注册 key 暂时不可用".to_owned(),
            outcome_unknown: true,
            external_reference: None,
            result_metadata: json!({
                "stage": "secret_resolution",
                "category": "dependency_unavailable",
            }),
        },
    }
}

fn valid_notification_body(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        !value.is_empty()
            && value.len() <= (16 << 10)
            && value == value.trim()
            && !value
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    })
}

fn transport_error(error: MoogleMailError, after_write: bool) -> DeliveryTransportError {
    let outcome_unknown = after_write
        || error.write_started()
        || error.category != MoogleMailErrorCategory::Configuration;
    DeliveryTransportError {
        failure_code: error.category.as_str().to_owned(),
        safe_detail: if after_write {
            "MoogleMail 外部写入结果未知".to_owned()
        } else {
            "MoogleMail 外部请求失败".to_owned()
        },
        // Delivery 已经进入 sending；除纯本地配置错误外，依赖/网络/协议错误都不能安全
        // 收口为普通失败。availability gate 会在此之前释放已知无凭据的 claim。
        outcome_unknown,
        external_reference: None,
        result_metadata: json!({}),
    }
}
