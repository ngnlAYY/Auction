//! 发送编排：空草稿证明、装备与资产分步暂存、send POST 以及写后 Sent/compose 证明。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::diagnostics as safe_diagnostics;

use super::{
    MoogleMailComposeSnapshot, MoogleMailSendOutcome, MoogleMailSendReceipt, MoogleMailSendRequest,
    MoogleMailSender, MoogleMailSentBaseline, preserve_write_unknown, staged_proof_error,
    write_error, write_unknown,
};

use crate::clients::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailErrorCategory, MoogleMailForm,
    MoogleMailOperation, MoogleMailSendResponseFailure, MoogleMailWorld,
};

impl MoogleMailSender {
    /// 在持久 sending fence 前只读检查 compose；调用方须持有直到 send 完成的同一凭据 guard。
    /// 不清空草稿，也不把历史任务的未知结果改为未发送。
    pub(crate) async fn preflight(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailComposeSnapshot, MoogleMailError> {
        self.inspect_compose(cookie, world).await
    }

    /// 发送失败后按只读方式回读 compose，区分“邮件内容/草稿问题”和其他远端问题。
    /// 只记录页面是否干净与资产计数，不把主题、正文或收件人写入日志。
    async fn diagnose_after_failure(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        expected: &MoogleMailSendRequest,
    ) {
        match self.inspect_compose(cookie, world).await {
            Ok(snapshot) => tracing::debug!(
                clean = snapshot.clean,
                expected_message_staged = snapshot.recipient == expected.message.recipient
                    && snapshot.subject == expected.message.subject
                    && snapshot.body == expected.message.body,
                credits = snapshot.credits,
                cod = snapshot.cod,
                attachment_count = snapshot.attachment_count,
                "MoogleMail send failed; compose page rechecked"
            ),
            Err(error) => {
                if error.category == MoogleMailErrorCategory::SessionExpired {
                    // 诊断 GET 也可证明凭据失效；不覆盖原投递结果，由原 session 回写撤销。
                    cookie.revoke();
                }
                safe_diagnostics::warn(
                    "mooglemail_send",
                    "read_failed_compose",
                    safe_diagnostics::project(&error),
                    DiagnosticContext::default(),
                );
            }
        }
    }

    /// 执行一次发送并在失败后回读 compose；不自动重试远程写入。
    pub async fn send(
        &self,
        cookie: &MoogleMailCookie,
        request: MoogleMailSendRequest,
    ) -> Result<MoogleMailSendOutcome, MoogleMailError> {
        self.send_with_baseline(cookie, request, None).await
    }

    /// 消费同一独占 session 在 sending fence 前取得的完整基线，写后证明保持不变。
    pub(crate) async fn send_prepared(
        &self,
        cookie: &MoogleMailCookie,
        request: MoogleMailSendRequest,
        baseline: MoogleMailSentBaseline,
    ) -> Result<MoogleMailSendOutcome, MoogleMailError> {
        self.send_with_baseline(cookie, request, Some(baseline)).await
    }

    async fn send_with_baseline(
        &self,
        cookie: &MoogleMailCookie,
        request: MoogleMailSendRequest,
        baseline: Option<MoogleMailSentBaseline>,
    ) -> Result<MoogleMailSendOutcome, MoogleMailError> {
        let mut write_started = false;
        let outcome = self
            .send_verified(cookie, &request, baseline, &mut write_started)
            .await
            .map_err(|error| preserve_write_unknown(error, write_started));
        let failed = !matches!(outcome, Ok(MoogleMailSendOutcome::Sent(_)));
        if failed
            && !outcome
                .as_ref()
                .is_err_and(|error| error.category == MoogleMailErrorCategory::SessionExpired)
        {
            // 写入结果未知时 Sent 可能已变化，旧基线不能再作为下一次的排除集。
            self.forget_sent_baseline(request.world);
            self.diagnose_after_failure(cookie, request.world, &request).await;
        }
        outcome
    }

    /// 执行装备准备、compose 附件/COD 分步写入并证明最终发送结果。
    /// 任何一次 POST 成功但后续页面无法证明的情况都会返回 write_unknown；调用方不得把
    /// 该结果自动重试为第二次远程写入。
    async fn send_verified(
        &self,
        cookie: &MoogleMailCookie,
        request: &MoogleMailSendRequest,
        baseline: Option<MoogleMailSentBaseline>,
        write_started: &mut bool,
    ) -> Result<MoogleMailSendOutcome, MoogleMailError> {
        if !request.validate() {
            return Err(write_error(
                request.world,
                MoogleMailOperation::Send,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let baseline = match baseline {
            Some(baseline) => baseline,
            // 独立调用仍自行准备；持久 delivery 只能使用 fence 前已完成的基线。
            None => self.prepare_sent_baseline(cookie, request.world).await?,
        };
        if baseline.world != request.world {
            return Err(write_error(
                request.world,
                MoogleMailOperation::Sent,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        self.check_baseline_scope(cookie, &baseline)?;
        let compose = self.client.compose(request.world)?;
        let initial = self.client.get(&compose, cookie).await?;
        let parsed_initial = self.parser.parse_compose_state(&initial, request.world);
        let dirty = matches!(&parsed_initial, Ok(state) if !state.is_clean());
        let initial_parse = match parsed_initial {
            Ok(state) if state.is_clean() => Ok(state),
            Ok(_) => {
                Err(MoogleMailError::protocol_drift(MoogleMailOperation::Compose, request.world))
            }
            Err(error) => Err(error),
        };
        let mut state = match self.client.diagnose_parse(&compose, &initial, initial_parse).await {
            Ok(state) => state,
            Err(diagnostic_cause) => {
                safe_diagnostics::warn(
                    "mooglemail_send",
                    "parse_compose_state",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(request.world.as_str()),
                        ..Default::default()
                    },
                );
                return Ok(MoogleMailSendOutcome::PreflightUnproven { dirty });
            }
        };

        self.prepare_equipment(cookie, request, write_started).await?;
        if *write_started {
            // Armory 写入可能使先前 compose token 失效；重新证明空草稿后才暂存任何资产。
            let refreshed = self.client.get(&compose, cookie).await?;
            let parsed =
                self.parser.parse_compose_state(&refreshed, request.world).and_then(|state| {
                    if state.is_clean() {
                        Ok(state)
                    } else {
                        Err(MoogleMailError::protocol_drift(
                            MoogleMailOperation::Compose,
                            request.world,
                        ))
                    }
                });
            state = self.client.diagnose_parse(&compose, &refreshed, parsed).await?;
        }

        if request.credits > 0 {
            let form =
                MoogleMailForm::attach_credits(state.token(), &request.message, request.credits)
                    .map_err(|diagnostic_cause| {
                        safe_diagnostics::warn(
                            "mooglemail_send",
                            "build_credit_form",
                            safe_diagnostics::project(&diagnostic_cause),
                            DiagnosticContext::default(),
                        );
                        write_error(
                            request.world,
                            MoogleMailOperation::AttachAdd,
                            MoogleMailErrorCategory::Configuration,
                        )
                    })?;
            let response = self.client.post(&compose, cookie, form).await?;
            // 后续请求即使尚未发出，已暂存的资产也不能作为未写入处理。
            *write_started = true;
            state = self
                .client
                .diagnose_parse(
                    &compose,
                    &response,
                    self.parser.prove_staged_compose(
                        &response,
                        request.world,
                        &request.message,
                        request.credits,
                        &[],
                        0,
                    ),
                )
                .await
                .map_err(|error| staged_proof_error(error, MoogleMailOperation::AttachAdd))?;
        }

        for (index, equipment_id) in request.equipment_ids.iter().enumerate() {
            let form =
                MoogleMailForm::attach_equipment(state.token(), &request.message, equipment_id)
                    .map_err(|diagnostic_cause| {
                        safe_diagnostics::warn(
                            "mooglemail_send",
                            "build_equipment_form",
                            safe_diagnostics::project(&diagnostic_cause),
                            DiagnosticContext {
                                world: diagnostic_world(request.world.as_str()),
                                ..Default::default()
                            },
                        );
                        write_unknown(request.world, MoogleMailOperation::AttachAdd)
                    })?;
            let response = self.client.post(&compose, cookie, form).await?;
            *write_started = true;
            state = self
                .client
                .diagnose_parse(
                    &compose,
                    &response,
                    self.parser.prove_staged_compose(
                        &response,
                        request.world,
                        &request.message,
                        request.credits,
                        &request.equipment_ids[..=index],
                        0,
                    ),
                )
                .await
                .map_err(|error| staged_proof_error(error, MoogleMailOperation::AttachAdd))?;
        }

        if request.cod > 0 {
            let form = MoogleMailForm::attach_cod(state.token(), &request.message, request.cod)
                .map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "mooglemail_send",
                        "build_cod_form",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext {
                            world: diagnostic_world(request.world.as_str()),
                            ..Default::default()
                        },
                    );
                    write_unknown(request.world, MoogleMailOperation::AttachCod)
                })?;
            let response = self.client.post(&compose, cookie, form).await?;
            *write_started = true;
            state = self
                .client
                .diagnose_parse(
                    &compose,
                    &response,
                    self.parser.prove_staged_compose(
                        &response,
                        request.world,
                        &request.message,
                        request.credits,
                        &request.equipment_ids,
                        request.cod,
                    ),
                )
                .await
                .map_err(|error| staged_proof_error(error, MoogleMailOperation::AttachCod))?;
        }

        let assets_staged =
            request.credits > 0 || request.cod > 0 || !request.equipment_ids.is_empty();
        let form =
            MoogleMailForm::send(state.token(), &request.message).map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_send",
                    "build_send_form",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(request.world.as_str()),
                        ..Default::default()
                    },
                );

                if assets_staged {
                    write_unknown(request.world, MoogleMailOperation::Send)
                } else {
                    write_error(
                        request.world,
                        MoogleMailOperation::Send,
                        MoogleMailErrorCategory::Configuration,
                    )
                }
            })?;
        let response = self.client.post(&compose, cookie, form).await?;
        let (failure, confirmation) = self.parser.inspect_send_response(&response, request.world);
        if let Some(failure) =
            self.client.diagnose_parse(&compose, &response, failure).await.ok().flatten()
        {
            return Ok(match failure {
                MoogleMailSendResponseFailure::RecipientNotFound { returned_mail_id } => {
                    MoogleMailSendOutcome::RecipientNotFound { returned_mail_id }
                }
                MoogleMailSendResponseFailure::RecipientIsSender => {
                    MoogleMailSendOutcome::RecipientIsSender { assets_staged }
                }
                MoogleMailSendResponseFailure::Rejected => {
                    MoogleMailSendOutcome::Rejected { assets_staged }
                }
            });
        }
        let confirmation_hint =
            self.client.diagnose_parse(&compose, &response, confirmation).await.ok().flatten();
        let proof = async {
            let external_mail_id = self
                .verify_new_sent(request, cookie, &baseline, confirmation_hint.as_deref())
                .await?;
            // Sent 证明后必须再证明 compose 回到空表单；残留草稿意味着资产状态仍未收口。
            let state = self.inspect_compose(cookie, request.world).await?;
            if !state.is_clean() {
                return Err(write_unknown(request.world, MoogleMailOperation::Send));
            }
            Ok(MoogleMailSendOutcome::Sent(MoogleMailSendReceipt { external_mail_id }))
        }
        .await;
        match proof {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                // 即使 POST 有成功提示，缺少 Sent/compose 证明仍留存响应，不能记成功或重寄。
                let unknown = self
                    .client
                    .diagnose_parse(
                        &compose,
                        &response,
                        Err(write_unknown(request.world, MoogleMailOperation::Send)),
                    )
                    .await;
                // 保留只读证明阶段的明确登录失效及原证据，交由原 session 撤销 Cookie。
                if error.category == MoogleMailErrorCategory::SessionExpired {
                    Err(preserve_write_unknown(error, true))
                } else {
                    unknown
                }
            }
        }
    }
}
