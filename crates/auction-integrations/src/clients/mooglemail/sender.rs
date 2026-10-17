//! MoogleMail 分步写入与发送后 Sent 完成证明。

use std::{collections::HashSet, fmt, time::Duration};

mod proof;

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
    /// compose 初始页无法证明为干净状态，可能残留上一次结果未知的资产。
    PreflightUnproven,
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
    ) -> Result<bool, MoogleMailError> {
        let compose = self.client.compose(world)?;
        let document = self.client.get(&compose, cookie).await?;
        let state = self
            .client
            .diagnose_parse(&compose, &document, self.parser.parse_compose_state(&document, world))
            .await?;
        Ok(state.is_clean())
    }

    /// 执行 compose、附件/COD 分步写入并证明最终发送结果。
    /// 任何一次 POST 成功但后续页面无法证明的情况都会返回 write_unknown；调用方不得把
    /// 该结果自动重试为第二次远程写入。
    pub async fn send(
        &self,
        cookie: &MoogleMailCookie,
        request: MoogleMailSendRequest,
    ) -> Result<MoogleMailSendOutcome, MoogleMailError> {
        if !request.validate() {
            return Err(write_error(
                request.world,
                MoogleMailOperation::Send,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let baseline_deadline =
            tokio::time::Instant::now().checked_add(self.verification_budget).ok_or_else(|| {
                write_error(
                    request.world,
                    MoogleMailOperation::Sent,
                    MoogleMailErrorCategory::Configuration,
                )
            })?;
        let baseline = self.scan_sent_baseline(request.world, cookie, baseline_deadline).await?;
        let compose = self.client.compose(request.world)?;
        let initial = self.client.get(&compose, cookie).await?;
        let initial_parse = match self.parser.parse_compose_state(&initial, request.world) {
            Ok(state) if state.is_clean() => Ok(state),
            Ok(_) => {
                Err(MoogleMailError::protocol_drift(MoogleMailOperation::Compose, request.world))
            }
            Err(error) => Err(error),
        };
        let mut state = match self.client.diagnose_parse(&compose, &initial, initial_parse).await {
            Ok(state) => state,
            Err(_) => return Ok(MoogleMailSendOutcome::PreflightUnproven),
        };

        let mut equipment_ids = Vec::with_capacity(request.equipment_ids.len());
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
                        &equipment_ids,
                        0,
                    ),
                )
                .await
                .map_err(|_| write_unknown(request.world, MoogleMailOperation::AttachAdd))?;
        }

        for equipment_id in &request.equipment_ids {
            let form =
                MoogleMailForm::attach_equipment(state.token(), &request.message, equipment_id)
                    .map_err(|_| write_unknown(request.world, MoogleMailOperation::AttachAdd))?;
            let response = self.client.post(&compose, cookie, form).await?;
            equipment_ids.push(equipment_id.clone());
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
                        &equipment_ids,
                        0,
                    ),
                )
                .await
                .map_err(|_| write_unknown(request.world, MoogleMailOperation::AttachAdd))?;
        }

        if request.cod > 0 {
            let form = MoogleMailForm::attach_cod(state.token(), &request.message, request.cod)
                .map_err(|_| write_unknown(request.world, MoogleMailOperation::AttachCod))?;
            let response = self.client.post(&compose, cookie, form).await?;
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
                        &equipment_ids,
                        request.cod,
                    ),
                )
                .await
                .map_err(|_| write_unknown(request.world, MoogleMailOperation::AttachCod))?;
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
        if let Some(failure) = self
            .client
            .diagnose_parse(
                &compose,
                &response,
                self.parser.classify_send_response_failure(&response, request.world),
            )
            .await
            .ok()
            .flatten()
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
        let confirmation_hint = self
            .client
            .diagnose_parse(
                &compose,
                &response,
                self.parser.prove_send_completed(&response, request.world),
            )
            .await
            .ok()
            .flatten();
        match self.verify_new_sent(&request, cookie, &baseline, confirmation_hint.as_deref()).await
        {
            Ok(external_mail_id) => {
                Ok(MoogleMailSendOutcome::Sent(MoogleMailSendReceipt { external_mail_id }))
            }
            Err(_) => {
                // 即使 POST 有成功提示，缺少 Sent 证明也要留存这次响应，不能记成功或重寄。
                self.client
                    .diagnose_parse(
                        &compose,
                        &response,
                        Err(write_unknown(request.world, MoogleMailOperation::Send)),
                    )
                    .await
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
