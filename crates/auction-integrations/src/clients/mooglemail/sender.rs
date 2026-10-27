//! MoogleMail 分步写入与发送后 Sent 完成证明。

use std::{collections::HashSet, fmt, time::Duration};

mod equipment;
mod maintenance;
mod proof;

pub(crate) use proof::MoogleMailSentBaseline;

pub use maintenance::{
    MoogleMailComposeSnapshot, MoogleMailDiscardOutcome, MoogleMailSentListEntry,
    MoogleMailSentObservation,
};

use super::{
    MoogleMailClient, MoogleMailCookie, MoogleMailError, MoogleMailErrorCategory, MoogleMailForm,
    MoogleMailMessageFields, MoogleMailOperation, MoogleMailParser, MoogleMailSendResponseFailure,
    MoogleMailWorld, protocol::valid_decimal_id,
};

const MAX_SEND_ATTACHMENTS: usize = 1000;

/// 一封待发送 MoogleMail 的业务无关输入。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailSendRequest {
    pub world: MoogleMailWorld,
    pub message: MoogleMailMessageFields,
    pub credits: i64,
    /// 要设置的 CoD 数量；零表示不设置 CoD。
    pub cod: i64,
    pub equipment_ids: Vec<String>,
}

impl MoogleMailSendRequest {
    pub fn validate(&self) -> bool {
        self.message.validate()
            && self.credits >= 0
            && self.cod >= 0
            && self.equipment_ids.len() <= MAX_SEND_ATTACHMENTS
            && self.equipment_ids.iter().all(|id| valid_decimal_id(id))
            && self.equipment_ids.iter().collect::<HashSet<_>>().len() == self.equipment_ids.len()
            && (self.cod == 0 || self.credits > 0 || !self.equipment_ids.is_empty())
    }
}

impl fmt::Debug for MoogleMailSendRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailSendRequest")
            .field("world", &self.world)
            .field("message", &"[REDACTED]")
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("equipment_count", &self.equipment_ids.len())
            .finish()
    }
}

/// 已经由新增 Sent 邮件的身份、正文和精确资产证明的寄送结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailSendReceipt {
    /// 通过发送前后 Sent 差异核对得到的真实远端邮件 ID。
    pub external_mail_id: String,
}

/// 最终 send POST 的结构化结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoogleMailSendOutcome {
    /// 新增 Sent 邮件已通过完整内容核验。
    Sent(MoogleMailSendReceipt),
    /// compose 初始页无法证明为干净状态；`dirty` 表示页面可解析但仍有草稿资产。
    PreflightUnproven { dirty: bool },
    /// 远端可见文字明确报告收件人不存在。
    RecipientNotFound { returned_mail_id: Option<String> },
    /// 远端明确拒绝自寄；已经暂存的资产仍须人工核对，不能当作交付成功。
    RecipientIsSender { assets_staged: bool },
    /// 远端明确拒绝发送；附件已经分步放入 compose 时不能自动重试。
    Rejected { assets_staged: bool },
}

/// 负责把一封 typed message 编排为受证明的多步 compose 写入。
#[derive(Clone)]
pub struct MoogleMailSender {
    client: MoogleMailClient,
    parser: MoogleMailParser,
    verification_pages: usize,
    verification_budget: Duration,
}

impl MoogleMailSender {
    /// 使用共享 MoogleMail client 构造发送编排器。
    pub fn new(client: MoogleMailClient) -> Self {
        Self {
            client,
            parser: MoogleMailParser,
            verification_pages: 100,
            verification_budget: Duration::from_secs(30),
        }
    }

    /// 设置发送前后 Sent 核对的页数和总预算。
    pub fn with_send_verification(
        mut self,
        maximum_pages: usize,
        budget: Duration,
    ) -> Result<Self, MoogleMailError> {
        if maximum_pages == 0 || maximum_pages > 100 || budget.is_zero() {
            return Err(MoogleMailError::new(
                MoogleMailOperation::Send,
                MoogleMailWorld::Main,
                MoogleMailErrorCategory::Configuration,
                false,
            ));
        }
        self.verification_pages = maximum_pages;
        self.verification_budget = budget;
        Ok(self)
    }

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
            Ok(snapshot) => tracing::warn!(
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
                tracing::warn!(
                    error = %error,
                    "MoogleMail send failed; compose page unreadable"
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
            Err(_) => return Ok(MoogleMailSendOutcome::PreflightUnproven { dirty }),
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
                    .map_err(|_| {
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
                    .map_err(|_| write_unknown(request.world, MoogleMailOperation::AttachAdd))?;
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
                .map_err(|_| write_unknown(request.world, MoogleMailOperation::AttachCod))?;
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
        let form = MoogleMailForm::send(state.token(), &request.message).map_err(|_| {
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
                .verify_new_sent(request, cookie, &baseline.seen, confirmation_hint.as_deref())
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

fn write_unknown(world: MoogleMailWorld, operation: MoogleMailOperation) -> MoogleMailError {
    MoogleMailError::after_write(operation, world, MoogleMailErrorCategory::WriteUnknown)
}

fn write_error(
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
    category: MoogleMailErrorCategory,
) -> MoogleMailError {
    MoogleMailError::new(operation, world, category, false)
}

fn staged_proof_error(
    mut error: MoogleMailError,
    operation: MoogleMailOperation,
) -> MoogleMailError {
    error.operation = operation;
    preserve_write_unknown(error, true)
}

/// 已发生外写后，读失败或后续写前失败都不能重新变成可安全重发；保留失效类别和原证据。
fn preserve_write_unknown(error: MoogleMailError, write_started: bool) -> MoogleMailError {
    if !write_started {
        return error;
    }
    let category = if error.category == MoogleMailErrorCategory::SessionExpired {
        MoogleMailErrorCategory::SessionExpired
    } else {
        MoogleMailErrorCategory::WriteUnknown
    };
    let mut unknown = MoogleMailError::after_write(error.operation, error.world, category);
    unknown.evidence_id = error.evidence_id;
    unknown.evidence_status = error.evidence_status;
    unknown.evidence_failure = error.evidence_failure;
    unknown
}
