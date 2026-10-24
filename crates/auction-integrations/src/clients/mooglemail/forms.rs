use std::fmt;

use url::form_urlencoded;

use super::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailToken,
    MoogleMailWorld,
    protocol::{valid_decimal_id, valid_line},
};

const MAX_RECIPIENT_BYTES: usize = 256;
const MAX_SUBJECT_BYTES: usize = 512;
pub(super) const MAX_BODY_BYTES: usize = 64 * 1024;

/// 每次 compose POST 都需要重放的消息字段。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailMessageFields {
    pub recipient: String,
    pub subject: String,
    pub body: String,
}

impl MoogleMailMessageFields {
    /// 校验字段可以安全编码为 MoogleMail 表单。
    pub fn validate(&self) -> bool {
        valid_line(&self.recipient, MAX_RECIPIENT_BYTES, false)
            && valid_line(&self.subject, MAX_SUBJECT_BYTES, false)
            && valid_body(&self.body)
    }
}

impl fmt::Debug for MoogleMailMessageFields {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailMessageFields")
            .field("recipient", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("body", &"[REDACTED]")
            .finish()
    }
}

impl Drop for MoogleMailMessageFields {
    fn drop(&mut self) {
        clear_string(&mut self.recipient);
        clear_string(&mut self.subject);
        clear_string(&mut self.body);
    }
}

/// 只可由 typed constructor 生成的 MoogleMail 表单。
/// token、消息正文和附件参数不会通过 Debug/Display 输出；表单被丢弃时会尽力清除其字符串
/// 缓冲区。真正发起请求前还必须由 MoogleMailClient::post 校验目标页面类型。
pub struct MoogleMailForm {
    pub(crate) operation: MoogleMailOperation,
    pub(crate) values: Vec<(String, String)>,
}

impl MoogleMailForm {
    pub fn attach_credits(
        token: &MoogleMailToken,
        message: &MoogleMailMessageFields,
        credits: i64,
    ) -> Result<Self, MoogleMailError> {
        if credits <= 0 {
            return Err(form_configuration(MoogleMailOperation::AttachAdd));
        }
        let mut form = Self::base(MoogleMailOperation::AttachAdd, token, message)?;
        form.set("action", MoogleMailOperation::AttachAdd.as_str());
        form.set("select_pane", "credits");
        form.set("select_item", "0");
        form.set("select_count", &credits.to_string());
        Ok(form)
    }

    pub fn attach_equipment(
        token: &MoogleMailToken,
        message: &MoogleMailMessageFields,
        equipment_id: &str,
    ) -> Result<Self, MoogleMailError> {
        if !valid_decimal_id(equipment_id) {
            return Err(form_configuration(MoogleMailOperation::AttachAdd));
        }
        let mut form = Self::base(MoogleMailOperation::AttachAdd, token, message)?;
        form.set("action", MoogleMailOperation::AttachAdd.as_str());
        form.set("select_pane", "equip");
        form.set("select_item", equipment_id);
        form.set("select_count", "1");
        Ok(form)
    }

    /// 构造设置正数 CoD 的 compose 动作。
    pub fn attach_cod(
        token: &MoogleMailToken,
        message: &MoogleMailMessageFields,
        cod: i64,
    ) -> Result<Self, MoogleMailError> {
        if cod <= 0 {
            return Err(form_configuration(MoogleMailOperation::AttachCod));
        }
        let mut form = Self::base(MoogleMailOperation::AttachCod, token, message)?;
        form.set("action", MoogleMailOperation::AttachCod.as_str());
        form.set("action_value", &cod.to_string());
        Ok(form)
    }

    pub fn send(
        token: &MoogleMailToken,
        message: &MoogleMailMessageFields,
    ) -> Result<Self, MoogleMailError> {
        let mut form = Self::base(MoogleMailOperation::Send, token, message)?;
        form.set("action", MoogleMailOperation::Send.as_str());
        Ok(form)
    }

    pub fn return_message(token: &MoogleMailToken) -> Result<Self, MoogleMailError> {
        Self::mail_action(MoogleMailOperation::ReturnMessage, token)
    }

    pub fn take(token: &MoogleMailToken) -> Result<Self, MoogleMailError> {
        Self::mail_action(MoogleMailOperation::Take, token)
    }

    /// 构造丢弃当前草稿的表单；调用方必须在 POST 前证明草稿精确属于已知投递。
    pub fn discard(
        token: &MoogleMailToken,
        message: &MoogleMailMessageFields,
    ) -> Result<Self, MoogleMailError> {
        let mut form = Self::base(MoogleMailOperation::Discard, token, message)?;
        form.set("action", MoogleMailOperation::Discard.as_str());
        Ok(form)
    }

    /// 返回表单对应的外部动作，不暴露任何字段值。
    pub const fn operation(&self) -> MoogleMailOperation {
        self.operation
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut serializer = form_urlencoded::Serializer::new(String::new());
        for (name, value) in &self.values {
            serializer.append_pair(name, value);
        }
        serializer.finish().into_bytes()
    }

    fn base(
        operation: MoogleMailOperation,
        token: &MoogleMailToken,
        message: &MoogleMailMessageFields,
    ) -> Result<Self, MoogleMailError> {
        if token.expose().is_empty() || !message.validate() {
            return Err(form_configuration(operation));
        }
        Ok(Self {
            operation,
            values: vec![
                ("mmtoken".to_owned(), token.expose().to_owned()),
                ("message_to_name".to_owned(), message.recipient.clone()),
                ("message_subject".to_owned(), message.subject.clone()),
                ("message_body".to_owned(), message.body.clone()),
            ],
        })
    }

    fn mail_action(
        operation: MoogleMailOperation,
        token: &MoogleMailToken,
    ) -> Result<Self, MoogleMailError> {
        if token.expose().is_empty()
            || !matches!(operation, MoogleMailOperation::ReturnMessage | MoogleMailOperation::Take)
        {
            return Err(form_configuration(operation));
        }
        Ok(Self {
            operation,
            values: vec![
                ("mmtoken".to_owned(), token.expose().to_owned()),
                ("action".to_owned(), operation.as_str().to_owned()),
                ("action_value".to_owned(), "0".to_owned()),
            ],
        })
    }

    fn set(&mut self, name: &str, value: &str) {
        self.values.push((name.to_owned(), value.to_owned()));
    }
}

impl fmt::Debug for MoogleMailForm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("MoogleMailForm").field("operation", &self.operation).finish()
    }
}

impl fmt::Display for MoogleMailForm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "MoogleMail form: operation={}", self.operation)
    }
}

impl Drop for MoogleMailForm {
    fn drop(&mut self) {
        for (_, value) in &mut self.values {
            clear_string(value);
        }
    }
}

fn form_configuration(operation: MoogleMailOperation) -> MoogleMailError {
    MoogleMailError::new(
        operation,
        MoogleMailWorld::Main,
        MoogleMailErrorCategory::Configuration,
        false,
    )
}

fn clear_string(value: &mut String) {
    let mut bytes = std::mem::take(value).into_bytes();
    bytes.fill(0);
}

pub(super) fn valid_body(value: &str) -> bool {
    value.len() <= MAX_BODY_BYTES
        && !value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
}
