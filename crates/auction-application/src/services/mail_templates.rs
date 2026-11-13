//! 生效模板快照的单次刷新；定时器由外层负责。

use std::sync::Arc;

use crate::ports::mail_templates::{
    LiveMailTemplates, MailTemplateStore, MailTemplateStoreError, effective_mail_templates,
};

/// 一次刷新的结果，供外层记录日志。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailTemplateRefresh {
    /// 快照内容是否发生变化。
    pub changed: bool,
    /// 正文不满足现行规则、已退回默认正文的模板键。
    pub skipped: Vec<String>,
}

/// 三个角色共用的刷新用例：读取每个模板键的最新修订，叠加到内置默认正文后整体替换快照。
pub struct MailTemplateRefreshService {
    store: Arc<dyn MailTemplateStore>,
    templates: LiveMailTemplates,
}

impl MailTemplateRefreshService {
    /// 注入窄数据库端口与进程快照。
    pub fn new(store: Arc<dyn MailTemplateStore>, templates: LiveMailTemplates) -> Self {
        Self { store, templates }
    }

    /// 一个读取周期。读取失败时不动现有快照，由调用方决定如何告警。
    pub async fn refresh(&self) -> Result<MailTemplateRefresh, MailTemplateStoreError> {
        let events = self.store.latest_mail_templates().await?;
        for event in &events {
            event.validate()?;
        }
        let effective = effective_mail_templates(&events);
        let changed = self.templates.replace(effective.templates);
        Ok(MailTemplateRefresh { changed, skipped: effective.skipped })
    }
}
