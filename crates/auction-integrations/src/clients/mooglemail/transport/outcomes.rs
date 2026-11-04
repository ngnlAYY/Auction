use super::*;

mod rejection;

pub(crate) use rejection::equipment_rejection_copy;

pub(crate) fn exact_action_expectation(
    state: &MoogleMailActionState,
    expected: &ActionExpectation,
) -> bool {
    let expected_count =
        expected.attachment_ids.len().saturating_add(usize::from(expected.credits > 0));
    if state.credits() != expected.credits
        || state.cod() != expected.cod
        || state.attachment_count() != expected_count
        || state.equipment_ids().len() != expected.attachment_ids.len()
    {
        return false;
    }
    let mut observed_ids = state.equipment_ids().iter().collect::<Vec<_>>();
    let mut expected_ids = expected.attachment_ids.iter().collect::<Vec<_>>();
    observed_ids.sort_unstable();
    expected_ids.sort_unstable();
    observed_ids == expected_ids
}

pub(crate) fn send_receipt(
    outcome: MoogleMailSendOutcome,
) -> Result<MoogleMailSendReceipt, DeliveryTransportError> {
    match outcome {
        MoogleMailSendOutcome::Sent(receipt) => Ok(receipt),
        MoogleMailSendOutcome::PreflightUnproven { dirty } => Err(DeliveryTransportError {
            failure_code: "preflight_unproven".to_owned(),
            safe_detail: if dirty {
                "MoogleMail compose 初始页仍有草稿资产，未执行发送"
            } else {
                "MoogleMail compose 初始状态无法证明为干净状态"
            }
            .to_owned(),
            outcome_unknown: true,
            external_reference: None,
            result_metadata: json!({
                "stage": "preflight",
                "operation": MoogleMailOperation::Compose.as_str(),
                "category": if dirty { "compose_dirty" } else { "protocol_drift" },
            }),
        }),
        MoogleMailSendOutcome::RecipientNotFound { returned_mail_id } => {
            let mut result_metadata = json!({
                "stage": "send",
                "operation": MoogleMailOperation::Send.as_str(),
                "category": "recipient_not_found",
                "reason_code": "recipient_not_found",
                "recipient_skipped_for_cycle": true,
                "retryable": false,
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
        MoogleMailSendOutcome::RecipientIsSender { assets_staged } => Err(DeliveryTransportError {
            failure_code: "recipient_is_sender".to_owned(),
            safe_detail: "MoogleMail 不允许向发件账号自身寄信；暂存资产须人工核对".to_owned(),
            // 已证明拒绝不等于资产已退回库存，更不等于原履约义务已经完成。
            outcome_unknown: assets_staged,
            external_reference: None,
            result_metadata: json!({
                "stage": "send",
                "operation": MoogleMailOperation::Send.as_str(),
                "category": "write_rejected",
                "reason_code": "recipient_is_sender",
                "assets_staged": assets_staged,
                "retryable": false,
            }),
        }),
        MoogleMailSendOutcome::Rejected { assets_staged } => Err(DeliveryTransportError {
            failure_code: if assets_staged { "send_rejected_after_stage" } else { "send_rejected" }
                .to_owned(),
            safe_detail: "MoogleMail 远端明确拒绝发送".to_owned(),
            outcome_unknown: assets_staged,
            external_reference: None,
            result_metadata: json!({
                "stage": "send",
                "operation": MoogleMailOperation::Send.as_str(),
                "category": "write_rejected",
            }),
        }),
    }
}

pub(crate) fn preflight_unproven(operation: MoogleMailOperation) -> DeliveryTransportError {
    DeliveryTransportError {
        failure_code: "preflight_unproven".to_owned(),
        safe_detail: "MoogleMail 动作前置身份或资产无法证明".to_owned(),
        outcome_unknown: true,
        external_reference: None,
        result_metadata: json!({
            "stage": "action_preflight",
            "operation": operation.as_str(),
            "category": "protocol_drift",
        }),
    }
}

pub(crate) fn action_proof_unproven(operation: MoogleMailOperation) -> DeliveryTransportError {
    DeliveryTransportError {
        failure_code: "write_unknown".to_owned(),
        safe_detail: "MoogleMail 外部动作结果无法证明".to_owned(),
        outcome_unknown: true,
        external_reference: None,
        result_metadata: json!({
            "stage": "action_proof",
            "operation": operation.as_str(),
            "category": "protocol_drift",
        }),
    }
}

pub(crate) fn action_name(operation: MoogleMailOperation) -> &'static str {
    match operation {
        MoogleMailOperation::ReturnMessage => "return",
        MoogleMailOperation::Take => "take",
        _ => "action",
    }
}

pub(crate) fn render_notification(
    templates: &MoogleMailNotificationTemplates,
    template: &str,
    result_code: &str,
    login_key: Option<&MailLoginKey>,
    recipient: &str,
    world: &str,
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
        ("auction_equipment_rejected", reason_code) => {
            let (reason_text, next_steps) = equipment_rejection_copy(reason_code)?;
            // 使用入站世界和邮件 ID 关联原提交；拒绝通知不复制装备 URL 或 key。
            let mail_reference = format!("{world}:{reference}");
            Some((
                "Auction mail rejected".to_owned(),
                templates.render(
                    "auction_equipment_rejected",
                    &[
                        ("reason_code", reason_code),
                        ("reason_text", reason_text),
                        ("next_steps", next_steps),
                        ("mail_reference", &mail_reference),
                    ],
                )?,
            ))
        }
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
        ("rekey_key", "rekeyed") => {
            let key = login_key?;
            let body = key.with_encoded(|encoded| {
                templates.render("rekey_key", &[("login_key", encoded), ("username", recipient)])
            })?;
            Some(("Auction login key rotated".to_owned(), body))
        }
        ("rekey_result", "blacklisted" | "identity_conflict" | "not_registered") => Some((
            "Key rotation result".to_owned(),
            templates
                .render("rekey_result", &[("result_code", result_code), ("username", recipient)])?,
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
            format!(
                "Hello {recipient},\n\n\
                 This is the auction notification you requested\n\
                 from the forum.\n\n\
                 Please check the auction page for the latest\n\
                 item details and bidding status."
            ),
        )),
        ("recovery_payment_reminder", "payment_overdue") => Some((
            "Auction payment overdue".to_owned(),
            templates.render("settlement_payment_reminder", &[("reference", reference)])?,
        )),
        _ => None,
    }
}

pub(crate) fn invalid_payload() -> DeliveryTransportError {
    DeliveryTransportError {
        failure_code: "invalid_payload".to_owned(),
        safe_detail: "MoogleMail delivery payload 无效".to_owned(),
        outcome_unknown: false,
        external_reference: None,
        result_metadata: json!({}),
    }
}

pub(crate) fn equipment_secret_not_found() -> DeliveryTransportError {
    DeliveryTransportError {
        failure_code: "secret_not_found".to_owned(),
        safe_detail: "MoogleMail 装备 secret 引用不存在".to_owned(),
        outcome_unknown: false,
        external_reference: None,
        result_metadata: json!({
            "stage": "secret_resolution",
            "category": "missing_reference",
        }),
    }
}

pub(crate) fn equipment_secret_unavailable() -> DeliveryTransportError {
    DeliveryTransportError {
        // 旧版 resolver 返回普通 error 时由 delivery Worker 统一收口为未知结果。
        failure_code: "gateway_outcome_unknown".to_owned(),
        safe_detail: "MoogleMail 装备 secret 解析结果未知".to_owned(),
        outcome_unknown: true,
        external_reference: None,
        result_metadata: json!({
            "category": "gateway_error",
        }),
    }
}

pub(crate) fn login_key_error(error: ApplicationError) -> DeliveryTransportError {
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

pub(crate) fn transport_error(error: MoogleMailError, after_write: bool) -> DeliveryTransportError {
    let write_started = after_write || error.write_started();
    let outcome_unknown = write_started || error.category != MoogleMailErrorCategory::Configuration;
    DeliveryTransportError {
        failure_code: error.category.as_str().to_owned(),
        safe_detail: if write_started {
            "MoogleMail 外部写入结果未知".to_owned()
        } else {
            "MoogleMail 外部请求失败".to_owned()
        },
        // Delivery 已经进入 sending；除纯本地配置错误外，依赖/网络/协议错误都不能安全
        // 收口为普通失败。availability gate 会在此之前释放已知无凭据的 claim。
        outcome_unknown,
        external_reference: None,
        result_metadata: json!({
            "stage": transport_stage(error.operation),
            "operation": error.operation.as_str(),
            "category": error.category.as_str(),
            "write_started": write_started,
        }),
    }
}

fn transport_stage(operation: MoogleMailOperation) -> &'static str {
    match operation {
        MoogleMailOperation::Compose => "compose_preflight",
        MoogleMailOperation::AttachAdd => "compose_attach",
        MoogleMailOperation::AttachCod => "compose_cod",
        MoogleMailOperation::InventoryOrganize
        | MoogleMailOperation::EquipmentUnstore
        | MoogleMailOperation::EquipmentUnlock => "equipment_preparation",
        MoogleMailOperation::Send => "send",
        MoogleMailOperation::ReturnMessage | MoogleMailOperation::Take => "action",
        _ => "read",
    }
}
