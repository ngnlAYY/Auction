//! 生效模板：内置默认正文叠加数据库里的最新修订，以及三个角色共用的进程内快照。

use std::sync::{Arc, PoisonError, RwLock};

use super::{MailTemplateEvent, MailTemplateOperation};
use crate::ports::admin::runtime_config::AdminRuntimeMailTemplatesConfig;

/// 一整套生效模板。类型由模板 owner 定义，这里只给它一个不带历史前缀的名字。
pub type MailTemplateSet = AdminRuntimeMailTemplatesConfig;

/// 叠加结果：生效模板，以及正文不满足现行规则而退回默认正文的模板键。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveMailTemplates {
    pub templates: MailTemplateSet,
    /// 被跳过的模板键；供调用方告警，不含正文。
    pub skipped: Vec<String>,
}

/// 把每个模板键的最新修订叠加到内置默认正文上。
///
/// 本版不认识的键直接忽略（回退到旧程序时，新程序写入的模板不影响发信）。正文不满足现行
/// 规则的修订退回默认正文并报告：模板问题不应让通知发不出去，也不能把未审计的占位符带进邮件。
/// 同一键出现多条时取版本最高的一条，不依赖输入顺序。
pub fn effective_mail_templates(events: &[MailTemplateEvent]) -> EffectiveMailTemplates {
    let mut templates = MailTemplateSet::default();
    let mut skipped = Vec::new();
    for key in MailTemplateSet::KEYS {
        let latest = events
            .iter()
            .filter(|event| event.template_key == key)
            .max_by_key(|event| event.revision);
        let Some(event) = latest else { continue };
        let body = match (event.operation, event.body.as_deref()) {
            (MailTemplateOperation::Reset, _) => continue,
            (MailTemplateOperation::Set, Some(body)) => body,
            (MailTemplateOperation::Set, None) => {
                skipped.push(key.to_owned());
                continue;
            }
        };
        if MailTemplateSet::validate_body(key, body).is_ok() {
            templates.set_body(key, body.to_owned());
        } else {
            skipped.push(key.to_owned());
        }
    }
    EffectiveMailTemplates { templates, skipped }
}

/// 进程内共享的生效模板快照。
///
/// 创建被超提醒的事务与 Worker 的发送路径在渲染时各取一次当前快照；刷新任务整体替换，
/// 读者不会看到替换到一半的模板。尚未成功读取数据库时是内置默认正文。
#[derive(Debug, Clone)]
pub struct LiveMailTemplates(Arc<RwLock<Arc<MailTemplateSet>>>);

impl Default for LiveMailTemplates {
    fn default() -> Self {
        Self::new(MailTemplateSet::default())
    }
}

impl LiveMailTemplates {
    /// 以给定模板建立快照。
    pub fn new(templates: MailTemplateSet) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(templates))))
    }

    /// 当前生效模板。返回的是不可变快照，之后的刷新不影响已取得的引用。
    pub fn current(&self) -> Arc<MailTemplateSet> {
        // 写者只做整体替换，不会留下半更新状态；锁中毒时继续使用现有快照。
        Arc::clone(&self.0.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// 整体替换快照；内容没有变化时不替换并返回 `false`。
    pub fn replace(&self, templates: MailTemplateSet) -> bool {
        let mut current = self.0.write().unwrap_or_else(PoisonError::into_inner);
        if **current == templates {
            return false;
        }
        *current = Arc::new(templates);
        true
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    use super::{LiveMailTemplates, MailTemplateSet, effective_mail_templates};
    use crate::ports::mail_templates::{MailTemplateEvent, MailTemplateOperation};

    const PAYOUT: &str = "Sold {item_name}.\nPayout: {amount} Credits\nReference: {reference}";

    fn event(key: &str, revision: i64, body: Option<&str>) -> MailTemplateEvent {
        MailTemplateEvent {
            command_id: Uuid::from_u128(revision as u128 + 1),
            template_key: key.to_owned(),
            revision,
            operation: if body.is_some() {
                MailTemplateOperation::Set
            } else {
                MailTemplateOperation::Reset
            },
            body: body.map(str::to_owned),
            actor_id: "admin".to_owned(),
            recorded_at: Utc.with_ymd_and_hms(2026, 11, 13, 12, 0, 0).unwrap(),
        }
    }

    #[test]
    fn without_revisions_every_template_is_the_builtin_default() {
        let effective = effective_mail_templates(&[]);
        assert_eq!(effective.templates, MailTemplateSet::default());
        assert!(effective.skipped.is_empty());
        // 卖家收款邮件的内置默认正文写明卖出的装备。
        let payout = effective.templates.body("settlement_seller_payout").unwrap_or_default();
        assert!(payout.contains("Item: {item_name}"), "{payout}");
    }

    #[test]
    fn latest_revision_wins_and_reset_returns_to_the_default() {
        let defaults = MailTemplateSet::default();
        let overridden =
            effective_mail_templates(&[event("settlement_seller_payout", 1, Some(PAYOUT))]);
        assert_eq!(overridden.templates.body("settlement_seller_payout"), Some(PAYOUT));
        // 其余模板不受影响。
        assert_eq!(overridden.templates.body("lot_watch"), defaults.body("lot_watch"));

        // 输入顺序不决定结果：版本最高的一条生效。
        let reset = effective_mail_templates(&[
            event("settlement_seller_payout", 2, None),
            event("settlement_seller_payout", 1, Some(PAYOUT)),
        ]);
        assert_eq!(
            reset.templates.body("settlement_seller_payout"),
            defaults.body("settlement_seller_payout")
        );
        assert!(reset.skipped.is_empty());
    }

    #[test]
    fn unusable_revisions_fall_back_to_the_default_and_are_reported() {
        let defaults = MailTemplateSet::default();
        let effective = effective_mail_templates(&[
            // 现行规则不接受的正文（缺必需占位符）。
            event("settlement_seller_payout", 1, Some("Payout: {amount} Credits")),
            // 本版不认识的模板键：忽略，不算损坏。
            event("future_template", 1, Some("anything")),
        ]);
        assert_eq!(
            effective.templates.body("settlement_seller_payout"),
            defaults.body("settlement_seller_payout")
        );
        assert_eq!(effective.skipped, ["settlement_seller_payout"]);
    }

    #[test]
    fn snapshot_is_replaced_as_a_whole() {
        let live = LiveMailTemplates::default();
        let before = live.current();
        assert!(!live.replace(MailTemplateSet::default()), "内容没有变化时不替换");
        let mut next = MailTemplateSet::default();
        assert!(next.set_body("settlement_seller_payout", PAYOUT.to_owned()));
        assert!(live.replace(next));
        // 已取得的快照保持原样，新读者看到新模板。
        assert_ne!(before.body("settlement_seller_payout"), Some(PAYOUT));
        assert_eq!(live.current().body("settlement_seller_payout"), Some(PAYOUT));
        // 克隆的句柄共享同一份快照。
        assert_eq!(live.current().body("settlement_seller_payout"), Some(PAYOUT));
    }
}
