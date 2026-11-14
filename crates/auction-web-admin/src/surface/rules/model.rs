//! 会话内站点文档（规则、关于）草稿、版本基线和已提交发布意图；两种文档各有一份，互不影响。

use auction_web_shared::SUPPORTED_LOCALES;
use dioxus::prelude::*;

use super::draft::DEFAULT_RULES_LOCALE;
use crate::api::{AdminDocumentKind, AdminRules, AdminRulesCandidate};
use crate::surface::pending::{PendingCommands, use_owned_pending_commands};
use crate::surface::presentation::use_unload_warning;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RuleNotice {
    Validated,
    Published,
    ActionError(String),
    Conflict,
}

/// 丢弃确认只保存用户选择的下一步，不承载发布命令或草稿正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RulesTransition {
    Select { locale: String, add: bool },
    Reload,
}

/// 草稿所属的权威版本；比较正文而不依赖可碰撞的摘要。
#[derive(Clone, PartialEq, Eq)]
pub(super) struct RulesBaseline {
    locale: String,
    revision: i64,
    title: String,
    markdown: String,
}

impl RulesBaseline {
    pub(super) fn from_document(document: &AdminRules) -> Self {
        Self {
            locale: document.locale.clone(),
            revision: document.revision,
            title: document.title.clone(),
            markdown: document.markdown.clone(),
        }
    }

    pub(super) fn matches_document(&self, document: &AdminRules) -> bool {
        self.locale == document.locale
            && self.revision == document.revision
            && self.title == document.title
            && self.markdown == document.markdown
    }

    pub(super) fn matches_candidate(&self, candidate: &AdminRulesCandidate) -> bool {
        self.locale == candidate.locale
            && self.title == candidate.title
            && self.markdown == candidate.markdown
    }

    pub(super) fn matches_locale(&self, locale: &str) -> bool {
        self.locale == locale
    }
}

/// 当前认证身份独占草稿、校验状态和已提交发布意图，站内路由不持有它们。
#[derive(Clone, Copy)]
pub(super) struct RulesDraft {
    pub(super) target: Signal<String>,
    pub(super) locales: Signal<Vec<String>>,
    pub(super) draft: Signal<Option<AdminRulesCandidate>>,
    pub(super) baseline: Signal<Option<RulesBaseline>>,
    pub(super) checked_candidate: Signal<Option<AdminRulesCandidate>>,
    pub(super) notice: Signal<Option<RuleNotice>>,
    pub(super) busy: Signal<bool>,
    pub(super) reload_generation: Signal<u64>,
    pub(super) pending: PendingCommands,
    pub(super) dirty: Memo<bool>,
}

/// 两种文档的草稿 owner；页面按自己的文档种类取用。
#[derive(Clone, Copy)]
pub(super) struct DocumentDrafts {
    rules: RulesDraft,
    about: RulesDraft,
}

impl DocumentDrafts {
    pub(super) fn get(self, kind: AdminDocumentKind) -> RulesDraft {
        match kind {
            AdminDocumentKind::Rules => self.rules,
            AdminDocumentKind::About => self.about,
        }
    }
}

/// 身份变化销毁 owner；站内导航只卸载读取组件，不取消已提交命令。
pub(in crate::surface) fn use_rules_draft() {
    let rules = use_document_draft();
    let about = use_document_draft();
    use_context_provider(|| DocumentDrafts { rules, about });
    // 任一文档有未发布的修改都要在离开页面前提醒。
    let dirty = use_memo(move || (rules.dirty)() || (about.dirty)());
    use_unload_warning(dirty);
}

fn use_document_draft() -> RulesDraft {
    let target = use_signal(|| DEFAULT_RULES_LOCALE.to_owned());
    let locales = use_signal(|| {
        SUPPORTED_LOCALES.iter().map(|locale| locale.code.to_owned()).collect::<Vec<_>>()
    });
    let draft = use_signal(|| None::<AdminRulesCandidate>);
    let baseline = use_signal(|| None::<RulesBaseline>);
    let checked_candidate = use_signal(|| None::<AdminRulesCandidate>);
    let notice = use_signal(|| None::<RuleNotice>);
    let busy = use_signal(|| false);
    let reload_generation = use_signal(|| 0_u64);
    let pending = use_owned_pending_commands(reload_generation, None);
    let dirty = use_memo(move || {
        let draft = draft.read();
        let baseline = baseline.read();
        match (draft.as_ref(), baseline.as_ref()) {
            (Some(candidate), Some(baseline)) => !baseline.matches_candidate(candidate),
            (Some(_), None) => true,
            (None, _) => false,
        }
    });
    RulesDraft {
        target,
        locales,
        draft,
        baseline,
        checked_candidate,
        notice,
        busy,
        reload_generation,
        pending,
        dirty,
    }
}
