use std::{collections::HashSet, fmt};

use super::{
    MoogleMailClient, MoogleMailCookie, MoogleMailError, MoogleMailErrorCategory, MoogleMailForm,
    MoogleMailMessageFields, MoogleMailOperation, MoogleMailParser, MoogleMailWorld,
    protocol::valid_decimal_id,
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
    ) -> Result<MoogleMailSendReceipt, MoogleMailError> {
        if !request.validate() {
            return Err(write_error(
                request.world,
                MoogleMailOperation::Send,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let compose = self.client.compose(request.world)?;
        let initial = self.client.get(&compose, cookie).await?;
        let mut state = self.parser.parse_compose_state(&initial, request.world)?;
        if !state.is_clean() {
            return Err(write_error(
                request.world,
                MoogleMailOperation::Compose,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }

        let mut equipment_ids = Vec::with_capacity(request.equipment_ids.len());
        if request.credits > 0 {
            let form =
                MoogleMailForm::attach_credits(state.token(), &request.message, request.credits)
                    .map_err(|_| write_unknown(request.world, MoogleMailOperation::AttachAdd))?;
            let response = self.client.post(&compose, cookie, form).await?;
            state = self
                .parser
                .prove_staged_compose(
                    &response,
                    request.world,
                    &request.message,
                    request.credits,
                    &equipment_ids,
                    0,
                )
                .map_err(|_| write_unknown(request.world, MoogleMailOperation::AttachAdd))?;
        }

        for equipment_id in &request.equipment_ids {
            let form =
                MoogleMailForm::attach_equipment(state.token(), &request.message, equipment_id)
                    .map_err(|_| write_unknown(request.world, MoogleMailOperation::AttachAdd))?;
            let response = self.client.post(&compose, cookie, form).await?;
            equipment_ids.push(equipment_id.clone());
            state = self
                .parser
                .prove_staged_compose(
                    &response,
                    request.world,
                    &request.message,
                    request.credits,
                    &equipment_ids,
                    0,
                )
                .map_err(|_| write_unknown(request.world, MoogleMailOperation::AttachAdd))?;
        }

        if request.cod > 0 {
            let form = MoogleMailForm::attach_cod(state.token(), &request.message, request.cod)
                .map_err(|_| write_unknown(request.world, MoogleMailOperation::AttachCod))?;
            let response = self.client.post(&compose, cookie, form).await?;
            state = self
                .parser
                .prove_staged_compose(
                    &response,
                    request.world,
                    &request.message,
                    request.credits,
                    &equipment_ids,
                    request.cod,
                )
                .map_err(|_| write_unknown(request.world, MoogleMailOperation::AttachCod))?;
        }

        let form = MoogleMailForm::send(state.token(), &request.message)
            .map_err(|_| write_unknown(request.world, MoogleMailOperation::Send))?;
        let response = self.client.post(&compose, cookie, form).await?;
        let external_mail_id = self
            .parser
            .prove_send_completed(&response, request.world)
            .map_err(|_| write_unknown(request.world, MoogleMailOperation::Send))?;
        Ok(MoogleMailSendReceipt { external_mail_id })
    }
}

fn write_unknown(world: MoogleMailWorld, operation: MoogleMailOperation) -> MoogleMailError {
    write_error(world, operation, MoogleMailErrorCategory::WriteUnknown)
}

fn write_error(
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
    category: MoogleMailErrorCategory,
) -> MoogleMailError {
    MoogleMailError::new(operation, world, category, false)
}
