//! 站点文档（规则、关于）页面加载、语言切换和 workspace 装配。

use auction_web_shared::t;
use auction_web_shared::{AsyncState, AsyncStateKind};
use dioxus::prelude::*;

use super::draft::{missing_rules_document, notice_text, to_draft};
use super::model::{DocumentDrafts, RuleNotice, RulesBaseline, RulesDraft};
use super::workspace::AdminRulesWorkspace;
use crate::api::{AdminDocumentKind, fetch_admin_rules};
use crate::surface::admin_api_error_message;
use crate::surface::pending::{PendingCommandPanel, use_pending_commands};

/// 读取并展示一种 Admin 站点文档；语言切换会让请求带上旧版约定的 `lang` 查询参数。
pub fn page(kind: AdminDocumentKind) -> Element {
    let RulesDraft {
        target,
        mut draft,
        mut baseline,
        mut checked_candidate,
        mut notice,
        busy,
        reload_generation,
        dirty,
    } = use_context::<DocumentDrafts>().get(kind);
    let pending =
        use_pending_commands(reload_generation, move |entity| entity == kind.pending_key());
    let rules = use_resource(move || {
        let generation = reload_generation();
        let locale = target();
        async move {
            #[cfg(test)]
            if let Some(error) = try_consume_context::<std::rc::Rc<tests::Publication>>()
                .and_then(|publication| publication.take_read_error())
            {
                return ((locale, generation), Err(error));
            }
            let result = fetch_admin_rules(kind, locale.clone()).await;
            ((locale, generation), result)
        }
    });

    // 回页时先核对权威语言与版本；草稿有修改时绝不被读取结果覆盖。
    use_effect(move || {
        let requested_locale = target();
        let requested_generation = reload_generation();
        let snapshot = rules.read();
        if rules.state().cloned() != UseResourceState::Ready {
            return;
        }
        let Some(((loaded_locale, loaded_generation), result)) = snapshot.as_ref() else { return };
        if loaded_locale != &requested_locale || *loaded_generation != requested_generation {
            return;
        }
        let missing;
        let document = match result {
            Ok(document) if document.locale == requested_locale => document,
            Err(error) if error.status == Some(404) => {
                missing = missing_rules_document(requested_locale.clone());
                &missing
            }
            _ => return,
        };
        let has_edits = baseline.peek().as_ref().is_some_and(|base| {
            base.matches_locale(&requested_locale)
                && draft.peek().as_ref().is_some_and(|candidate| !base.matches_candidate(candidate))
        });
        if has_edits {
            if baseline.peek().as_ref().is_none_or(|base| !base.matches_document(document)) {
                checked_candidate.set(None);
                notice.set(None);
            }
            return;
        }
        baseline.set(Some(RulesBaseline::from_document(document)));
        draft.set(Some(to_draft(document, &requested_locale)));
        checked_candidate.set(None);
        // 成功反馈属于已完成的发布，不因后续权威快照落地而消失。
        if !matches!(notice.peek().as_ref(), Some(RuleNotice::Published)) {
            notice.set(None);
        }
    });

    // Resource 重读时保留旧值；旧语言或旧世代的响应均不能装配编辑器。
    let snapshot = rules.read_unchecked();
    let current = if rules.state().cloned() == UseResourceState::Ready {
        snapshot
            .as_ref()
            .filter(|((loaded_locale, generation), _)| {
                loaded_locale == &target() && *generation == reload_generation()
            })
            .map(|(_, result)| result)
    } else {
        None
    };
    let baseline_ready =
        baseline.read().as_ref().is_some_and(|base| base.matches_locale(&target()));
    let needs_baseline = current.is_some_and(|result| match result {
        Ok(_) => true,
        Err(error) => error.status == Some(404),
    });
    let current = if needs_baseline && !baseline_ready { None } else { current };
    let content = match current {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("admin-rules-loading"),
                detail: "",
            }
        },
        Some(Err(error)) if error.status == Some(404) => {
            let document = missing_rules_document(target());
            rsx! {
                p { class: "admin-rules-status", role: "status", {t!("admin-rules-missing")} }
                AdminRulesWorkspace {
                    kind,
                    document,
                    target: target(),
                    target_signal: target,
                    draft,
                    checked_candidate,
                    baseline,
                    notice,
                    busy,
                    reload_generation,
                    pending,
                    dirty,
                }
            }
        }
        Some(Err(error)) => rsx! {
            AdminRulesLoadError {
                detail: admin_api_error_message(&error.code),
                reload_generation,
            }
        },
        Some(Ok(document)) => rsx! {
            AdminRulesWorkspace {
                kind,
                document: document.clone(),
                target: target(),
                target_signal: target,
                draft,
                checked_candidate,
                baseline,
                notice,
                busy,
                reload_generation,
                pending,
                dirty,
            }
        },
    };

    rsx! {
        section {
            class: "admin-rules-page",
            aria_labelledby: "rules-title",
            h1 { id: "rules-title",
                match kind {
                    AdminDocumentKind::Rules => t!("page-rules-title"),
                    AdminDocumentKind::About => t!("page-about-title"),
                }
            }
            p { class: "admin-rules-intro",
                match kind {
                    AdminDocumentKind::Rules => t!("admin-rules-description"),
                    AdminDocumentKind::About => t!("admin-about-description"),
                }
            }
            PendingCommandPanel { owner: pending }
            // 发布清理草稿后编辑器会卸载，已接受的发布仍由文档页面播报。
            div { role: "status",
                if matches!(notice(), Some(RuleNotice::Published)) {
                    p { class: "admin-rules-ok", {notice_text(&RuleNotice::Published)} }
                }
            }
            {content}
        }
    }
}

#[component]
pub(super) fn AdminRulesLoadError(detail: String, mut reload_generation: Signal<u64>) -> Element {
    rsx! {
        AsyncState {
            kind: AsyncStateKind::Error,
            title: detail,
            detail: "",
            retry_label: t!("admin-rules-retry"),
            on_retry: move |_| reload_generation.set(reload_generation().saturating_add(1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use dioxus::core::Mutation;
    use dioxus::prelude::*;

    use super::page;
    use crate::api::{AdminDocumentKind, AdminRules, AdminRulesCandidate};
    use crate::surface::auctions::draft_page_tests::{
        RecordRead, click_button, has_live_text, init_i18n, mount_writer, settle_writer,
    };
    use crate::surface::rules::model::DocumentDrafts;

    pub(super) struct Publication {
        document: RefCell<AdminRules>,
        reply: RecordRead<AdminRules>,
        reads: Cell<usize>,
        read_error: RefCell<Option<crate::api::AdminApiFailure>>,
    }

    impl Publication {
        pub(super) fn take_read_error(&self) -> Option<crate::api::AdminApiFailure> {
            self.read_error.take()
        }
    }

    #[component]
    fn DocumentPage(kind: AdminDocumentKind) -> Element {
        page(kind)
    }

    fn check_publication_read(kind: AdminDocumentKind, remount: bool) {
        let publication = Rc::new(Publication {
            document: RefCell::new(AdminRules {
                locale: "en".to_owned(),
                title: "原始标题".to_owned(),
                markdown: "原始正文".to_owned(),
                published_by: "operator".to_owned(),
                published_at: "2026-11-18T00:00:00Z".to_owned(),
                revision: 1,
            }),
            reply: RecordRead::new(),
            reads: Cell::new(0),
            read_error: RefCell::new(None),
        });
        let (mut mounted, mut edits) = mount_writer({
            let publication = publication.clone();
            move || {
                init_i18n();
                super::super::model::use_rules_draft();
                use_context_provider(|| publication.clone());
                let current = use_context::<DocumentDrafts>().get(kind);
                let read = publication.clone();
                let write = publication.clone();
                use_hook(move || {
                    AdminDocumentKind::stub_documents(
                        move |requested, locale| {
                            assert_eq!(requested, kind);
                            let document = read.document.borrow();
                            assert_eq!(locale, document.locale);
                            read.reads.set(read.reads.get() + 1);
                            document.clone()
                        },
                        move |requested, request| {
                            assert_eq!(requested, kind);
                            assert_eq!(request.expected_revision, 1);
                            assert_eq!(request.title, "新标题");
                            let write = write.clone();
                            async move { write.reply.fetch().await.unwrap() }
                        },
                    );
                });
                let mut open = use_signal(|| true);
                rsx! {
                    button { onclick: move |_| open.toggle(), {"切换测试路由".to_owned()} }
                    button { onclick: move |_| {
                        let mut draft = current.draft;
                        let mut checked = current.checked_candidate;
                        let candidate = AdminRulesCandidate {
                            locale: "en".to_owned(), title: "新标题".to_owned(), markdown: "新正文".to_owned(),
                        };
                        draft.set(Some(candidate.clone()));
                        checked.set(Some(candidate));
                    }, {"准备已校验草稿".to_owned()} }
                    if open() { DocumentPage { kind } }
                }
            }
        });
        edits.extend(settle_writer(&mut mounted));
        assert!(has_live_text(&mounted, "保存并发布"));
        let publish = edits
            .iter()
            .rev()
            .find_map(|edit| match edit {
                Mutation::NewEventListener { name, id } if name == "click" => Some(*id),
                _ => None,
            })
            .unwrap();
        click_button(&mut mounted, "准备已校验草稿");
        mounted.dispatch("click", publish, "");
        if remount {
            click_button(&mut mounted, "切换测试路由");
            click_button(&mut mounted, "切换测试路由");
            assert_eq!(publication.reads.get(), 2);
        } else {
            publication.read_error.replace(Some(crate::api::AdminApiFailure::from_http_body(
                503,
                r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
            )));
        }
        assert!(has_live_text(&mounted, "保存并发布"));

        let document = {
            let mut document = publication.document.borrow_mut();
            document.revision = 2;
            document.title = "新标题".to_owned();
            document.markdown = "新正文".to_owned();
            document.clone()
        };
        publication.reply.respond(Ok(document));
        settle_writer(&mut mounted);
        if remount {
            assert_eq!(publication.reads.get(), 3);
        } else {
            assert!(!has_live_text(&mounted, "保存并发布"));
            assert!(has_live_text(&mounted, "所需服务暂时不可用。请稍后再试。"));
            let receipt = mounted.dom().in_scope(dioxus::core::ScopeId::APP, || {
                crate::surface::rules::draft::notice_text(
                    &crate::surface::rules::model::RuleNotice::Published,
                )
            });
            assert!(has_live_text(&mounted, &receipt));
            click_button(&mut mounted, "重试");
            assert!(has_live_text(&mounted, &receipt));
        }
        assert!(has_live_text(&mounted, "保存并发布"));
        assert!(!has_live_text(&mounted, "正在加载文档……"));
    }

    #[test]
    fn rules_reload_after_returning_before_publication_settles() {
        check_publication_read(AdminDocumentKind::Rules, true);
    }

    #[test]
    fn about_reloads_after_returning_before_publication_settles() {
        check_publication_read(AdminDocumentKind::About, true);
    }

    #[test]
    fn publication_receipts_survive_a_failed_read_for_both_document_kinds() {
        for kind in [AdminDocumentKind::Rules, AdminDocumentKind::About] {
            check_publication_read(kind, false);
        }
    }
}
