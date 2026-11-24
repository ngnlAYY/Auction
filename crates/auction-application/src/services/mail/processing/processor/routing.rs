//! MoogleMail subject 路由、注册/换绑命令和未知邮件处理。

use auction_time::Timestamp;
use serde_json::json;
use uuid::Uuid;

use super::super::commands::{
    decision, extract_login_key, has_user_assets, stable_command_id, user_request_hash,
};
use super::MailProcessorService;
use super::delivery::{REGISTER_REPLY_TEMPLATES, REKEY_REPLY_TEMPLATES};
use crate::errors::ApplicationError;
use crate::ports::core::Clock;
use crate::ports::mail::{
    MailAuctionIntakePort, MailDeliveryEnqueuer, MailEquipmentReader, MailMessage,
    MailNotificationTemplate, MailProcessingAttempt, MailProcessingDecision, MailProcessingError,
    MailProcessingRoute, MailUserCommandKind, MailUserPort, MailUserRebindCommand,
    MailUserRegisterCommand, MailUserRekeyCommand, MailUserResultCode,
};
use crate::ports::settlement::{
    parse_returned_settlement_body, system_returned_settlement_obligation_id,
};

/// 旧路由的中间结果；未知邮件由调用方决定继续 `process_unknown` 还是交给寄售候选。
enum LegacyMessageRoute {
    Decision(MailProcessingDecision),
    Unknown(MailMessage),
}

impl<E, A, D, U, C> MailProcessorService<E, A, D, U, C>
where
    E: MailEquipmentReader,
    A: MailAuctionIntakePort,
    D: MailDeliveryEnqueuer,
    U: MailUserPort,
    C: Clock,
{
    pub(super) async fn process_message(
        &self,
        message: MailMessage,
        pending_equipment_diagnostic: &mut Option<
            crate::ports::mail::PendingMailEquipmentDiagnostic,
        >,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        match self.route_legacy_message(message, pending_equipment_diagnostic).await? {
            LegacyMessageRoute::Decision(decision) => Ok(decision),
            LegacyMessageRoute::Unknown(message) => self.process_unknown(message).await,
        }
    }

    /// 旧路由优先的分流入口；只有调用方确认带 4D1 候选且全部旧路由无法归属时才返回候选。
    pub(super) async fn route_message(
        &self,
        message: MailMessage,
        consignment_candidate: bool,
        pending_equipment_diagnostic: &mut Option<
            crate::ports::mail::PendingMailEquipmentDiagnostic,
        >,
    ) -> MailProcessingRoute {
        let routed = self.route_legacy_message(message, pending_equipment_diagnostic).await;
        match routed {
            Ok(LegacyMessageRoute::Decision(decision)) => {
                MailProcessingRoute::Legacy(Box::new(MailProcessingAttempt {
                    result: Ok(decision),
                    pending_equipment_diagnostic: pending_equipment_diagnostic.take(),
                }))
            }
            Ok(LegacyMessageRoute::Unknown(_)) if consignment_candidate => {
                MailProcessingRoute::ConsignmentCandidate
            }
            Ok(LegacyMessageRoute::Unknown(message)) => {
                let result = self.process_unknown(message).await;
                MailProcessingRoute::Legacy(Box::new(MailProcessingAttempt {
                    result,
                    pending_equipment_diagnostic: pending_equipment_diagnostic.take(),
                }))
            }
            Err(error) => MailProcessingRoute::Legacy(Box::new(MailProcessingAttempt {
                result: Err(error),
                pending_equipment_diagnostic: pending_equipment_diagnostic.take(),
            })),
        }
    }

    /// 原有世界校验、续办、精确主题与系统结算退件分支；函数体逐项保持原顺序。
    async fn route_legacy_message(
        &self,
        message: MailMessage,
        pending_equipment_diagnostic: &mut Option<
            crate::ports::mail::PendingMailEquipmentDiagnostic,
        >,
    ) -> Result<LegacyMessageRoute, ApplicationError> {
        message.validate()?;
        if !self.world_ids.iter().any(|world_id| world_id == &message.world_id) {
            return Err(MailProcessingError::InvalidConfiguration.into());
        }
        // 标的提交与领取入队之间可能发生取消；接受事实优先于当前远端内容，禁止重试时
        // 因页面暂时不可读或附件已被领取而改判为退回。
        if let Some(decision) = self.resume_accepted_auction(&message).await? {
            return Ok(LegacyMessageRoute::Decision(decision));
        }
        // Users 的创建/更新时刻只来自本地处理时钟；远端邮件时间可能偏快，不能污染账号元数据。
        // 装备投稿仍保留既有的接收时间门禁。
        let now = self.clock.now();
        // 旧版使用 Unicode CaseFold 路由，但刻意不裁剪标题首尾空白；保持这一边界可以
        // 兼容合法的大小写变体，同时不把带隐藏空白的未知标题误认成业务命令。
        let routing_subject = message.subject.to_ascii_lowercase();
        Ok(match routing_subject.as_str() {
            "auction" => {
                let intake_now = now.max(message.received_at);
                LegacyMessageRoute::Decision(
                    self.process_auction(message, intake_now, pending_equipment_diagnostic).await?,
                )
            }
            "register" => LegacyMessageRoute::Decision(self.process_register(message, now).await?),
            "rebind" => LegacyMessageRoute::Decision(self.process_rebind(message, now).await?),
            "rekey" if self.rekey_enabled => {
                LegacyMessageRoute::Decision(self.process_rekey(message, now).await?)
            }
            // 被拒收或过期的结算邮件只由系统发件人 `MoogleMail` 以 `[RTS] ` + 原 canonical
            // 主题退回。义务 ID 可由公开 ID 推出，不能证明来源：无前缀的结算主题与玩家自拟的
            // 同名主题都按未知邮件处理（不变量 6）。
            subject => match system_returned_settlement_obligation_id(&message.sender, subject) {
                Some(obligation_id) => LegacyMessageRoute::Decision(
                    self.process_settlement_return(message, obligation_id)?,
                ),
                None => LegacyMessageRoute::Unknown(message),
            },
        })
    }

    /// 将系统退回的结算邮件转换为严格的退件证据。
    /// 退回邮件本身不创建任何 delivery 动作（不领取、不退回、不回信）；它只把 obligation、资产
    /// 和金额事实写入 `inbound_mail`，由 recovery Worker 在 Serializable 事务中复核来源并与义务
    /// 比对：恢复流程决定收回动作，其余结算退件只留证并转人工对账。来源已认证的退件即使资产
    /// 异常（例如不可领取）也照样记为证据，交给消费方隔离，不以错误让收件游标停在这封邮件上。
    /// 正文只记录系统后缀写明的原收件人、原正文摘要与补寄派发引用原文，用于把退件绑定到具体
    /// 派发；正文本身不进入证据。系统后缀不完整时不记录这三项，补寄退件随后按无法证明隔离。
    fn process_settlement_return(
        &self,
        message: MailMessage,
        obligation_id: Uuid,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        let mut evidence = json!({
            "obligation_id": obligation_id.to_string(),
            "credits": message.credits,
            "cod": message.cod,
            "attachment_ids": message.sorted_attachment_ids(),
        });
        if let Some(body) = parse_returned_settlement_body(&message.body) {
            evidence["returned_from"] = json!(body.returned_from);
            evidence["body_digest"] = json!(
                body.body_digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
            );
            if let Some(reference) = body.dispatch_reference {
                evidence["dispatch_reference"] = json!(reference);
            }
        }
        decision("delivery", "accepted", "settlement_recovery_returned", evidence)
    }

    async fn process_register(
        &self,
        message: MailMessage,
        now: Timestamp,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        if has_user_assets(&message) {
            return self.reject_user_assets(&message, "registration").await;
        }
        let command_id = stable_command_id(&message, "user-register");
        let request_hash = user_request_hash(&message, command_id, MailUserCommandKind::Register)?;
        let result = self
            .users
            .register_or_recover(MailUserRegisterCommand {
                command_id,
                request_hash,
                username: message.sender.clone(),
                now,
            })
            .await?;
        let result = result.validate(MailUserCommandKind::Register)?;
        let (status, template, user_key_user_id) = match result.code {
            MailUserResultCode::Registered | MailUserResultCode::Recovered => {
                ("accepted", MailNotificationTemplate::RegisterKey, result.user_id)
            }
            MailUserResultCode::Blacklisted | MailUserResultCode::IdentityConflict => {
                ("rejected", MailNotificationTemplate::RegisterResult, None)
            }
            _ => return Err(MailProcessingError::InvalidUserResult.into()),
        };
        // 同一账号 24 小时内只回一封信。
        let delivery_id = self
            .enqueue_account_notice(
                &message,
                template,
                result.code.as_str(),
                user_key_user_id,
                REGISTER_REPLY_TEMPLATES,
            )
            .await?;
        let mut delivery_ids = Vec::new();
        delivery_ids.extend(delivery_id);
        decision(
            "registration",
            status,
            if status == "accepted" { "registration_accepted" } else { "registration_rejected" },
            json!({
                "user_id": result.user_id,
                "owner_result_code": result.code.as_str(),
                "delivery_ids": delivery_ids,
                "notice_suppressed": delivery_id.is_none(),
            }),
        )
    }

    /// `rekey` 不带资产：轮换登录 key、注销全部有效会话，并在 24 小时预算内回信新 key。
    async fn process_rekey(
        &self,
        message: MailMessage,
        now: Timestamp,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        if has_user_assets(&message) {
            return self.reject_user_assets(&message, "rekey").await;
        }
        let command_id = stable_command_id(&message, "user-rekey");
        let request_hash = user_request_hash(&message, command_id, MailUserCommandKind::Rekey)?;
        let result = self
            .users
            .rekey_login_key(MailUserRekeyCommand {
                command_id,
                request_hash,
                username: message.sender.clone(),
                now,
            })
            .await?;
        let result = result.validate(MailUserCommandKind::Rekey)?;
        let (status, template, user_key_user_id) = match result.code {
            MailUserResultCode::Rekeyed => {
                ("accepted", MailNotificationTemplate::RekeyKey, result.user_id)
            }
            MailUserResultCode::Blacklisted
            | MailUserResultCode::IdentityConflict
            | MailUserResultCode::NotRegistered => {
                ("rejected", MailNotificationTemplate::RekeyResult, None)
            }
            _ => return Err(MailProcessingError::InvalidUserResult.into()),
        };
        let delivery_id = self
            .enqueue_account_notice(
                &message,
                template,
                result.code.as_str(),
                user_key_user_id,
                REKEY_REPLY_TEMPLATES,
            )
            .await?;
        let mut delivery_ids = Vec::new();
        delivery_ids.extend(delivery_id);
        decision(
            "rekey",
            status,
            if status == "accepted" { "rekey_accepted" } else { "rekey_rejected" },
            json!({
                "user_id": result.user_id,
                "owner_result_code": result.code.as_str(),
                "delivery_ids": delivery_ids,
                "notice_suppressed": delivery_id.is_none(),
            }),
        )
    }

    async fn process_rebind(
        &self,
        message: MailMessage,
        now: Timestamp,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        if has_user_assets(&message) {
            return self.reject_user_assets(&message, "rebind").await;
        }
        let old_key = match extract_login_key(&message.body) {
            Ok(key) => key,
            Err(result_code) => {
                let notice_id = self
                    .enqueue_explanation(
                        &message,
                        MailNotificationTemplate::RebindResult,
                        result_code,
                    )
                    .await?;
                let mut delivery_ids = Vec::new();
                delivery_ids.extend(notice_id);
                return decision(
                    "rebind",
                    "rejected",
                    "rebind_rejected",
                    json!({
                        "owner_result_code": result_code,
                        "delivery_ids": delivery_ids,
                        "notice_suppressed": notice_id.is_none(),
                    }),
                );
            }
        };
        let command_id = stable_command_id(&message, "user-rebind");
        let request_hash = user_request_hash(&message, command_id, MailUserCommandKind::Rebind)?;
        let old_key_hash = old_key.public_key_hash();
        let result = self
            .users
            .rebind_username(MailUserRebindCommand {
                command_id,
                request_hash,
                old_key_hash,
                next_username: message.sender.clone(),
                history_id: Uuid::new_v4(),
                now,
            })
            .await?;
        let result = result.validate(MailUserCommandKind::Rebind)?;
        let status = match result.code {
            MailUserResultCode::Rebound | MailUserResultCode::Unchanged => "accepted",
            MailUserResultCode::Blacklisted
            | MailUserResultCode::IdentityConflict
            | MailUserResultCode::InvalidKey => "rejected",
            _ => return Err(MailProcessingError::InvalidUserResult.into()),
        };
        let notice_id = self
            .enqueue_explanation(
                &message,
                MailNotificationTemplate::RebindResult,
                result.code.as_str(),
            )
            .await?;
        let mut delivery_ids = Vec::new();
        delivery_ids.extend(notice_id);
        decision(
            "rebind",
            status,
            if status == "accepted" { "rebind_accepted" } else { "rebind_rejected" },
            json!({
                "user_id": result.user_id,
                "owner_result_code": result.code.as_str(),
                "delivery_ids": delivery_ids,
                "notice_suppressed": notice_id.is_none(),
            }),
        )
    }

    async fn process_unknown(
        &self,
        message: MailMessage,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        if !has_user_assets(&message) {
            return decision("unknown", "accepted", "archived", json!({}));
        }
        if !message.returnable {
            return Err(MailProcessingError::UnsafeDisposition.into());
        }
        let attachment_ids = message.sorted_attachment_ids();
        let attachment_count = attachment_ids.len();
        let delivery_id = self.enqueue_return(&message, attachment_ids).await?;
        decision(
            "unknown",
            "rejected",
            "unknown_assets_rejected",
            json!({ "attachment_count": attachment_count, "delivery_ids": [delivery_id] }),
        )
    }

    async fn reject_user_assets(
        &self,
        message: &MailMessage,
        mail_kind: &str,
    ) -> Result<MailProcessingDecision, ApplicationError> {
        if !message.returnable {
            return Err(MailProcessingError::UnsafeDisposition.into());
        }
        let attachment_ids = message.sorted_attachment_ids();
        let attachment_count = attachment_ids.len();
        let return_id = self.enqueue_return(message, attachment_ids).await?;
        // 说明信受发件人预算限制；资产仍照常退回给发件人。
        let notice_id = self
            .enqueue_explanation(
                message,
                MailNotificationTemplate::UserAssetsRejected,
                "user_assets_rejected",
            )
            .await?;
        let mut delivery_ids = vec![return_id];
        delivery_ids.extend(notice_id);
        decision(
            mail_kind,
            "rejected",
            "user_assets_rejected",
            json!({
                "attachment_count": attachment_count,
                "delivery_ids": delivery_ids,
                "notice_suppressed": notice_id.is_none(),
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Waker};

    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::super::MailProcessorConfig;
    use super::super::MailProcessorService;
    use crate::ports::admin::AdminIntakeSourceSnapshot;
    use crate::ports::core::{ApplicationFuture, Clock};
    use crate::ports::mail::{
        MailAuctionAcceptance, MailAuctionIntakeCommand, MailAuctionIntakeLookup,
        MailAuctionIntakePort, MailAuctionIntakeResult, MailDeliveryActivation,
        MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryResult, MailEquipmentAttachment,
        MailEquipmentFilter, MailEquipmentReader, MailMessage, MailNoticeEnqueue, MailNoticeLimit,
        MailProcessingAttempt, MailProcessingRoute, MailProcessor, MailReplyPolicy,
        MailReplyPolicySource, MailUserPort, MailUserRebindCommand, MailUserRegisterCommand,
        MailUserRekeyCommand, MailUserResult, SubmissionReceiptFacts,
    };

    struct UnusedEquipment;

    impl MailEquipmentReader for UnusedEquipment {
        fn read(
            &self,
            _world_id: String,
            _attachment: MailEquipmentAttachment,
        ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot> {
            Box::pin(async { unreachable!("旧路由测试不读取装备") })
        }
    }

    struct NothingAccepted;

    impl MailAuctionIntakePort for NothingAccepted {
        fn find_accepted(
            &self,
            _lookup: MailAuctionIntakeLookup,
        ) -> ApplicationFuture<'_, Option<MailAuctionAcceptance>> {
            Box::pin(async { Ok(None) })
        }

        fn ingest_mail(
            &self,
            _command: MailAuctionIntakeCommand,
        ) -> ApplicationFuture<'_, MailAuctionIntakeResult> {
            Box::pin(async { unreachable!("旧路由测试不写入投稿") })
        }

        fn load_submission_receipt(
            &self,
            _submission_id: Uuid,
        ) -> ApplicationFuture<'_, Option<SubmissionReceiptFacts>> {
            Box::pin(async { unreachable!("旧路由测试不读取投稿回执") })
        }
    }

    struct SuppressedDeliveries;

    impl MailDeliveryEnqueuer for SuppressedDeliveries {
        fn enqueue(
            &self,
            _command: MailDeliveryCommand,
        ) -> ApplicationFuture<'_, MailDeliveryResult> {
            Box::pin(async { unreachable!("旧路由测试不直接入队") })
        }

        fn enqueue_within_limit(
            &self,
            _command: MailDeliveryCommand,
            _limit: MailNoticeLimit,
        ) -> ApplicationFuture<'_, MailNoticeEnqueue> {
            Box::pin(async { Ok(MailNoticeEnqueue::Suppressed) })
        }

        fn prepare_recovery(
            &self,
            _command: MailDeliveryCommand,
        ) -> ApplicationFuture<'_, MailDeliveryResult> {
            Box::pin(async { unreachable!("旧路由测试不准备 recovery") })
        }

        fn activate_recovery(
            &self,
            _activation: MailDeliveryActivation,
        ) -> ApplicationFuture<'_, MailDeliveryResult> {
            Box::pin(async { unreachable!("旧路由测试不激活 recovery") })
        }
    }

    struct UnusedUsers;

    impl MailUserPort for UnusedUsers {
        fn register_or_recover(
            &self,
            _command: MailUserRegisterCommand,
        ) -> ApplicationFuture<'_, MailUserResult> {
            Box::pin(async { unreachable!("旧路由测试不注册账号") })
        }

        fn rebind_username(
            &self,
            _command: MailUserRebindCommand,
        ) -> ApplicationFuture<'_, MailUserResult> {
            Box::pin(async { unreachable!("旧路由测试不换绑账号") })
        }

        fn rekey_login_key(
            &self,
            _command: MailUserRekeyCommand,
        ) -> ApplicationFuture<'_, MailUserResult> {
            Box::pin(async { unreachable!("旧路由测试不轮换 key") })
        }
    }

    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> Timestamp {
            Timestamp::from_timestamp(1_790_000_000, 0).expect("固定测试时钟")
        }
    }

    #[derive(Debug)]
    struct FixedReplyPolicy;

    impl MailReplyPolicySource for FixedReplyPolicy {
        fn current(&self) -> MailReplyPolicy {
            MailReplyPolicy::default()
        }
    }

    type Service = MailProcessorService<
        UnusedEquipment,
        NothingAccepted,
        SuppressedDeliveries,
        UnusedUsers,
        FixedClock,
    >;

    fn run<T>(future: impl Future<Output = T>) -> T {
        let mut future = pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        for _ in 0..64 {
            if let Poll::Ready(result) = future.as_mut().poll(&mut context) {
                return result;
            }
        }
        panic!("内存端口未在预期轮数内完成");
    }

    fn service() -> Service {
        MailProcessorService::new(
            vec!["main".to_owned()],
            MailProcessorConfig {
                start_price: 100,
                equipment_filter: MailEquipmentFilter::default(),
                rekey_enabled: true,
                reply_policy: Arc::new(FixedReplyPolicy),
            },
            Arc::new(UnusedEquipment),
            Arc::new(NothingAccepted),
            Arc::new(SuppressedDeliveries),
            Arc::new(UnusedUsers),
            Arc::new(FixedClock),
        )
        .expect("测试处理器配置")
    }

    fn message(subject: &str, sender: &str) -> MailMessage {
        MailMessage {
            mailbox_key: "mooglemail:main".to_owned(),
            external_mail_id: "3125357".to_owned(),
            world_id: "main".to_owned(),
            received_at: FixedClock.now(),
            sender: sender.to_owned(),
            subject: subject.to_owned(),
            body: String::new(),
            credits: 0,
            cod: 0,
            attachments: Vec::new(),
            returnable: true,
            claimable: true,
            content_digest: [7; 32],
        }
    }

    fn legacy_attempt(route: MailProcessingRoute) -> MailProcessingAttempt {
        match route {
            MailProcessingRoute::Legacy(attempt) => *attempt,
            MailProcessingRoute::ConsignmentCandidate => {
                panic!("旧路由必须优先归属")
            }
        }
    }

    #[test]
    fn identified_system_return_stays_on_legacy_route() {
        let service = service();
        let obligation = Uuid::from_u128(77);
        let route = run(service.route(
            message(&format!("[RTS] Auction buyer refund [{obligation}]"), "MoogleMail"),
            true,
        ));
        let decision = legacy_attempt(route).result.expect("系统退件决定");
        assert_eq!(decision.mail_kind, "delivery");
        assert_eq!(decision.decision_code, "settlement_recovery_returned");
    }

    #[test]
    fn exact_auction_subject_stays_on_legacy_route() {
        let service = service();
        let route = run(service.route(message("auction", "FakeSeller"), true));
        let decision = legacy_attempt(route).result.expect("拍卖主题决定");
        assert_eq!(decision.mail_kind, "equipment");
        assert_eq!(decision.decision_code, "auction_no_attachments");
    }

    #[test]
    fn unknown_with_candidate_returns_consignment_candidate() {
        let service = service();
        match run(service.route(message("consign", "FakeSeller"), true)) {
            MailProcessingRoute::ConsignmentCandidate => {}
            MailProcessingRoute::Legacy(_) => panic!("旧路由无法归属时必须返回候选"),
        }
    }

    #[test]
    fn unknown_without_candidate_keeps_process_unknown_result() {
        let service = service();
        let route = run(service.route(message("unrelated", "FakeSeller"), false));
        let decision = legacy_attempt(route).result.expect("未知邮件决定");
        assert_eq!(decision.mail_kind, "unknown");
        assert_eq!(decision.decision_code, "archived");
    }
}
