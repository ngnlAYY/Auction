use std::{collections::HashSet, fmt};

use super::{
    MoogleMailClient, MoogleMailCookie, MoogleMailError, MoogleMailErrorCategory, MoogleMailForm,
    MoogleMailMessageFields, MoogleMailOperation, MoogleMailParser, MoogleMailSendResponseFailure,
    MoogleMailWorld, protocol::valid_decimal_id,
};

const MAX_SEND_ATTACHMENTS: usize = 1000;

/// 一封待发送 MoogleMail 的业务无关输入。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailSendRequest {
    /// 发送所在世界。
    pub world: MoogleMailWorld,
    /// 收件人、标题和正文。
    pub message: MoogleMailMessageFields,
    /// 要附加的 Credits 数量。
    pub credits: i64,
    /// 要设置的 CoD 数量；零表示不设置 CoD。
    pub cod: i64,
    /// 要附加的远端装备 ID。
    pub equipment_ids: Vec<String>,
}

impl MoogleMailSendRequest {
    /// 校验请求可以进入 compose 写入流程。
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

/// 已经由发送成功页证明的 MoogleMail 结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailSendReceipt {
    /// 成功页暴露的远端邮件 ID；旧页面未暴露时为 None。
    pub external_mail_id: Option<String>,
}

/// 最终 send POST 的结构化结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoogleMailSendOutcome {
    /// 远端成功页已通过证明。
    Sent(MoogleMailSendReceipt),
    /// compose 初始页无法证明为干净状态，可能残留上一次结果未知的资产。
    PreflightUnproven,
    /// 远端可见文字明确报告收件人不存在。
    RecipientNotFound { returned_mail_id: Option<String> },
    /// 远端明确拒绝发送；附件已经分步放入 compose 时不能自动重试。
    Rejected { assets_staged: bool },
}

/// 负责把一封 typed message 编排为受证明的多步 compose 写入。
#[derive(Clone)]
pub struct MoogleMailSender {
    client: MoogleMailClient,
    parser: MoogleMailParser,
}

impl MoogleMailSender {
    /// 使用共享 MoogleMail client 构造发送编排器。
    pub fn new(client: MoogleMailClient) -> Self {
        Self { client, parser: MoogleMailParser }
    }

    /// 执行 compose、附件/COD 分步写入并证明最终发送结果。
    ///
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
            .map_err(|_| write_unknown(request.world, MoogleMailOperation::Send))?
        {
            return Ok(match failure {
                MoogleMailSendResponseFailure::RecipientNotFound { returned_mail_id } => {
                    MoogleMailSendOutcome::RecipientNotFound { returned_mail_id }
                }
                MoogleMailSendResponseFailure::Rejected => {
                    MoogleMailSendOutcome::Rejected { assets_staged }
                }
            });
        }
        let external_mail_id = self
            .client
            .diagnose_parse(
                &compose,
                &response,
                self.parser.prove_send_completed(&response, request.world),
            )
            .await
            .map_err(|_| write_unknown(request.world, MoogleMailOperation::Send))?;
        Ok(MoogleMailSendOutcome::Sent(MoogleMailSendReceipt { external_mail_id }))
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
