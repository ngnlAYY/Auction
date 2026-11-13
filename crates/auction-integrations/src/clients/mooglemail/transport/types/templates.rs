//! MoogleMail 通知模板快照与渲染；闭集、默认正文和占位符规则由应用层模板 owner 维护。

use auction_application::mail_templates::LiveMailTemplates;

/// 发送路径使用的通知模板：与本进程其余部分共享同一份生效快照。
///
/// 快照由装配根从数据库周期刷新，只包含通过闭集和占位符校验的正文。每封邮件在渲染时取一次
/// 当前快照，因此模板修改只影响之后渲染的邮件；发送后的核对使用该次发送冻结的正文，不重新
/// 读取模板。默认值是内置默认正文，供没有数据库的测试与骨架模式使用。
#[derive(Debug, Clone, Default)]
pub struct MoogleMailNotificationTemplates {
    templates: LiveMailTemplates,
}

impl MoogleMailNotificationTemplates {
    /// 绑定本进程共享的生效模板快照。
    pub fn live(templates: LiveMailTemplates) -> Self {
        Self { templates }
    }

    /// 渲染统一委托给应用层模板 owner，避免发送端与创建端出现两套占位符展开规则。
    pub(crate) fn render(&self, key: &str, variables: &[(&str, &str)]) -> Option<String> {
        self.templates.current().render(key, variables)
    }
}
