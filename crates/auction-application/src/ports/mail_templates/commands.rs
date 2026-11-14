//! 模板修订命令：稳定摘要、原样重放与版本 CAS。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{MailTemplateSet, MailTemplateStoreError};

/// 修订动作。恢复默认也是一条事实：之后内置默认正文更新时，该模板随代码一起更新。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailTemplateOperation {
    /// 保存运营编写的正文。
    Set,
    /// 放弃运营正文，回到内置默认正文。
    Reset,
}

impl MailTemplateOperation {
    /// SQL 与审计共用的固定标签。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::Reset => "reset",
        }
    }
}

/// 可重放的接收结果，同时是只追加的修订事实。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MailTemplateEvent {
    /// 非零命令身份。
    pub command_id: Uuid,
    /// 模板键；历史事实里可能出现本版不认识的键。
    pub template_key: String,
    /// 单个模板键内递增的版本。
    pub revision: i64,
    pub operation: MailTemplateOperation,
    /// `set` 的正文；`reset` 为空。
    pub body: Option<String>,
    /// 操作者，Admin 使用会话身份。
    pub actor_id: String,
    /// 数据库接收时间。
    pub recorded_at: DateTime<Utc>,
}

impl MailTemplateEvent {
    /// 校验可读事实的形状。这里不按当前占位符规则拒绝历史正文：规则收紧后旧修订仍是
    /// 合法历史，能否用于渲染由生效快照判断。
    pub fn validate(&self) -> Result<(), MailTemplateStoreError> {
        let shape = match self.operation {
            MailTemplateOperation::Set => self.body.as_deref().is_some_and(|body| !body.is_empty()),
            MailTemplateOperation::Reset => self.body.is_none(),
        };
        if !shape
            || self.command_id.is_nil()
            || self.revision < 1
            || !valid_key_shape(&self.template_key)
            || !text(&self.actor_id, 256)
        {
            return Err(MailTemplateStoreError::Corrupt);
        }
        Ok(())
    }
}

/// 稳定命令负载；摘要不包含请求关联 ID 等易变信息。
#[derive(Debug, Clone, Serialize)]
pub struct MailTemplateCommand {
    /// 原样重试所需的调用方命令 ID。
    pub command_id: Uuid,
    pub template_key: String,
    /// 预期最新版本；从未修订过时为零。
    pub expected_revision: i64,
    pub operation: MailTemplateOperation,
    /// `set` 的正文。
    pub body: Option<String>,
    pub actor_id: String,
    /// 请求关联 ID 只进审计，不参与摘要。
    #[serde(skip)]
    pub request_id: Option<String>,
}

impl MailTemplateCommand {
    /// 接收新命令前的完整校验：模板键在闭集内，`set` 的正文满足该模板当前的规则。
    pub fn validate(&self) -> Result<(), MailTemplateStoreError> {
        if self.command_id.is_nil()
            || self.expected_revision < 0
            || self.expected_revision == i64::MAX
            || !text(&self.actor_id, 256)
            || self.request_id.as_deref().is_some_and(|value| !text(value, 128))
        {
            return Err(MailTemplateStoreError::Invalid);
        }
        if !MailTemplateSet::KEYS.contains(&self.template_key.as_str()) {
            return Err(MailTemplateStoreError::UnknownTemplate);
        }
        match (self.operation, self.body.as_deref()) {
            (MailTemplateOperation::Set, Some(body)) => {
                MailTemplateSet::validate_body(&self.template_key, body)
                    .map_err(|error| MailTemplateStoreError::InvalidBody(error.message()))
            }
            (MailTemplateOperation::Reset, None) => Ok(()),
            _ => Err(MailTemplateStoreError::Invalid),
        }
    }

    /// 应用层统一的版本化摘要。
    pub fn digest(&self) -> Result<[u8; 32], MailTemplateStoreError> {
        crate::ports::core::RequestHash::from_request("mail_template_v1", self.command_id, self)
            .map(crate::ports::core::RequestHash::as_bytes)
            .map_err(invalid)
    }

    /// 验证账本身份并返回原结果；重放不重新检查当前版本，也不按现行规则重审已接受的正文。
    pub fn replay(
        &self,
        digest_version: i16,
        request_digest: &[u8],
        event: MailTemplateEvent,
    ) -> Result<MailTemplateEvent, MailTemplateStoreError> {
        if digest_version != 1 || request_digest != self.digest()? {
            return Err(MailTemplateStoreError::Conflict);
        }
        event.validate()?;
        if event.command_id != self.command_id
            || event.template_key != self.template_key
            || event.revision
                != self.expected_revision.checked_add(1).ok_or(MailTemplateStoreError::Corrupt)?
            || event.operation != self.operation
            || event.body != self.body
            || event.actor_id != self.actor_id
        {
            return Err(MailTemplateStoreError::Corrupt);
        }
        Ok(event)
    }

    /// 在模板键锁内构造下一条事实；不修改旧事实。
    pub fn accept(
        &self,
        previous: Option<&MailTemplateEvent>,
        now: DateTime<Utc>,
    ) -> Result<MailTemplateEvent, MailTemplateStoreError> {
        self.validate()?;
        if let Some(previous) = previous {
            previous.validate()?;
            if previous.template_key != self.template_key {
                return Err(MailTemplateStoreError::Corrupt);
            }
        }
        if previous.map_or(0, |event| event.revision) != self.expected_revision {
            return Err(MailTemplateStoreError::Conflict);
        }
        let revision = self
            .expected_revision
            .checked_add(1)
            .filter(|revision| *revision > 0)
            .ok_or(MailTemplateStoreError::Invalid)?;
        let event = MailTemplateEvent {
            command_id: self.command_id,
            template_key: self.template_key.clone(),
            revision,
            operation: self.operation,
            body: self.body.clone(),
            actor_id: self.actor_id.clone(),
            recorded_at: now,
        };
        event.validate().map_err(invalid)?;
        Ok(event)
    }
}

/// 模板键的持久形状，与数据库 CHECK 一致；是否属于本版闭集另行判断。
fn valid_key_shape(value: &str) -> bool {
    let mut characters = value.chars();
    value.len() <= 64
        && characters.next().is_some_and(|first| first.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

/// 编码失败或由命令构造出的事实不自洽，都归为命令无效；不携带底层错误内容。
fn invalid<T>(_: T) -> MailTemplateStoreError {
    MailTemplateStoreError::Invalid
}

fn text(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    use super::{MailTemplateCommand, MailTemplateEvent, MailTemplateOperation};
    use crate::ports::mail_templates::MailTemplateStoreError;

    const PAYOUT: &str = "Sold {item_name}.\nPayout: {amount} Credits\nReference: {reference}";

    fn set(expected_revision: i64, body: &str) -> MailTemplateCommand {
        MailTemplateCommand {
            command_id: Uuid::from_u128(0x10 + expected_revision as u128),
            template_key: "settlement_seller_payout".to_owned(),
            expected_revision,
            operation: MailTemplateOperation::Set,
            body: Some(body.to_owned()),
            actor_id: "admin".to_owned(),
            request_id: None,
        }
    }

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 13, 12, 0, 0).unwrap()
    }

    #[test]
    fn first_revision_starts_from_zero_and_later_ones_need_the_latest_revision() {
        let first = set(0, PAYOUT).accept(None, now()).expect("首次保存");
        assert_eq!((first.revision, first.body.as_deref()), (1, Some(PAYOUT)));
        // 版本已变化：必须重新读取后用新命令提交，不能覆盖别人刚保存的正文。
        assert_eq!(
            set(0, PAYOUT).accept(Some(&first), now()),
            Err(MailTemplateStoreError::Conflict)
        );
        let second = set(1, PAYOUT).accept(Some(&first), now()).expect("基于最新版本保存");
        assert_eq!(second.revision, 2);
    }

    #[test]
    fn reset_is_a_fact_without_body() {
        let first = set(0, PAYOUT).accept(None, now()).expect("首次保存");
        let reset = MailTemplateCommand {
            command_id: Uuid::from_u128(0x99),
            operation: MailTemplateOperation::Reset,
            body: None,
            ..set(1, PAYOUT)
        };
        let event = reset.accept(Some(&first), now()).expect("恢复默认");
        assert_eq!(
            (event.revision, event.operation, event.body),
            (2, MailTemplateOperation::Reset, None)
        );
        // 动作与正文必须成对：恢复默认不带正文，保存必须带正文。
        let with_body = MailTemplateCommand { body: Some(PAYOUT.to_owned()), ..reset };
        assert_eq!(with_body.validate(), Err(MailTemplateStoreError::Invalid));
        let without_body = MailTemplateCommand { body: None, ..set(0, PAYOUT) };
        assert_eq!(without_body.validate(), Err(MailTemplateStoreError::Invalid));
    }

    #[test]
    fn body_is_checked_against_the_template_rules_before_it_is_accepted() {
        // 缺必需占位符、带未登记占位符、未知模板键都在写入前被拒绝。
        assert!(matches!(
            set(0, "Payout: {amount} Credits").accept(None, now()),
            Err(MailTemplateStoreError::InvalidBody(_))
        ));
        assert!(matches!(
            set(0, "{item_name} {amount} {reference} {login_key}").accept(None, now()),
            Err(MailTemplateStoreError::InvalidBody(_))
        ));
        let unknown =
            MailTemplateCommand { template_key: "no_such_template".to_owned(), ..set(0, PAYOUT) };
        assert_eq!(unknown.accept(None, now()), Err(MailTemplateStoreError::UnknownTemplate));
    }

    #[test]
    fn replay_returns_the_stored_fact_only_for_the_identical_request() {
        let command = set(0, PAYOUT);
        let event = command.accept(None, now()).expect("首次保存");
        let digest = command.digest().expect("摘要");
        assert_eq!(command.replay(1, &digest, event.clone()), Ok(event.clone()));
        // 同一命令 ID 换了正文：摘要不同，按冲突处理而不是返回旧结果。
        let changed =
            MailTemplateCommand { body: Some(format!("{PAYOUT}\nThanks.")), ..command.clone() };
        assert_eq!(
            changed.replay(1, &digest, event.clone()),
            Err(MailTemplateStoreError::Conflict)
        );
        // 账本里的事实与命令对不上时失败关闭。
        let foreign = MailTemplateEvent { revision: 7, ..event };
        assert_eq!(command.replay(1, &digest, foreign), Err(MailTemplateStoreError::Corrupt));
    }

    #[test]
    fn request_id_does_not_change_the_digest() {
        let command = set(0, PAYOUT);
        let traced =
            MailTemplateCommand { request_id: Some("req-1".to_owned()), ..command.clone() };
        assert_eq!(command.digest(), traced.digest());
    }
}
