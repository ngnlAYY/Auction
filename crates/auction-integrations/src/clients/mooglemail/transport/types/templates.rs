//! MoogleMail 通知模板快照与渲染；闭集、默认正文和占位符规则由应用层模板 owner 维护。

use auction_application::admin::AdminRuntimeMailTemplatesConfig;

/// Worker 启动时冻结的 MoogleMail 通知模板快照。
/// 快照只保存经过闭集和占位符校验的正文；发送过程中不再读取可变配置，避免同一
/// 批次出现部分旧模板、部分新模板的结果。
#[derive(Debug, Clone, Default)]
pub struct MoogleMailNotificationTemplates {
    templates: AdminRuntimeMailTemplatesConfig,
}

impl MoogleMailNotificationTemplates {
    /// 从配置快照构造模板，并在 Worker 启动阶段再次拒绝非法正文和未知占位符。
    pub fn from_config(templates: &AdminRuntimeMailTemplatesConfig) -> Result<Self, String> {
        templates.validate().map_err(|(key, error)| {
            format!("MoogleMail mail_templates.{key}: {}", error.message())
        })?;
        Ok(Self { templates: templates.clone() })
    }

    /// 渲染统一委托给应用层模板 owner，避免发送端与创建端出现两套占位符展开规则。
    pub(crate) fn render(&self, key: &str, variables: &[(&str, &str)]) -> Option<String> {
        self.templates.render(key, variables)
    }
}
