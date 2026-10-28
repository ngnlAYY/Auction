//! MoogleMail 通知模板快照与渲染；闭集、默认正文和占位符规则由应用层模板 owner 维护。

use auction_application::AdminRuntimeMailTemplatesConfig;

use crate::clients::mooglemail::forms::MAX_BODY_BYTES;

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

    pub(crate) fn render(&self, key: &str, variables: &[(&str, &str)]) -> Option<String> {
        let template = self.templates.body(key)?;
        let mut output = String::with_capacity(template.len());
        let mut cursor = 0;
        while let Some(relative_start) = template[cursor..].find('{') {
            let start = cursor + relative_start;
            let end = start + 1 + template[start + 1..].find('}')?;
            let placeholder = &template[start + 1..end];
            let value = variables.iter().find(|(name, _)| *name == placeholder)?.1;
            // 按展开后的字节数限流；重复占位符不能先膨胀成巨型正文再交给发送校验。
            if output.len().checked_add(start - cursor)?.checked_add(value.len())? > MAX_BODY_BYTES
            {
                return None;
            }
            output.push_str(&template[cursor..start]);
            output.push_str(value);
            cursor = end + 1;
        }
        if output.len().checked_add(template.len() - cursor)? > MAX_BODY_BYTES {
            return None;
        }
        output.push_str(&template[cursor..]);
        Some(output)
    }
}
