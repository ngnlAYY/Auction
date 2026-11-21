//! 凭据页面的数据加载、删除编排和顶层布局。

#[cfg(all(test, not(target_arch = "wasm32")))]
use std::rc::Rc;

use auction_web_shared::t;
use auction_web_shared::{AsyncState, AsyncStateKind};
use dioxus::prelude::*;

use super::super::admin_api_error_message;
use super::super::query::use_page_query_state;
use super::labels::CredentialFeedback;
use super::{CredentialCreateForm, CredentialDeleteDialog, CredentialTable};
#[cfg(all(test, not(target_arch = "wasm32")))]
use crate::api::AdminCredentialPage;
use crate::api::{AdminCredential, delete_admin_credential, fetch_admin_credentials};
#[cfg(all(test, not(target_arch = "wasm32")))]
use crate::surface::auctions::editor::tests::RecordRead;
use crate::surface::pagination::{AdminPager, use_admin_pagination, use_versioned_admin_page};
use crate::surface::pending::{
    CommandDelivery, CommandReceipt, PendingCommandPanel, set_if_alive, try_set_if_alive,
    use_pending_commands,
};

/// 加载并展示 Admin 凭据管理页面。
pub fn page() -> Element {
    let mut reload_generation = use_signal(|| 0_u64);
    let pending_commands =
        use_pending_commands(reload_generation, |entity| entity.starts_with("credential:"));
    // 分页跟地址栏双向同步（?page=）；创建表单与删除确认对话框是瞬态，不入 URL。
    let query = use_page_query_state();
    let query_state = query.state;
    let requested_page = use_memo(move || query_state.read().page);
    let mut pending_delete = use_signal(|| None::<AdminCredential>);
    let mut delete_feedback = use_signal(|| None::<CredentialFeedback>);
    let pagination =
        use_admin_pagination("credentials", &["page_size"], Some(200), i64::from(i32::MAX));
    let credentials = use_versioned_admin_page(
        pagination.clone(),
        reload_generation,
        move || *requested_page.read(),
        |page, size| async move {
            #[cfg(all(test, not(target_arch = "wasm32")))]
            if let Some(reads) = try_consume_context::<Rc<RecordRead<AdminCredentialPage>>>() {
                return reads.fetch().await;
            }
            fetch_admin_credentials(String::new(), String::new(), String::new(), page, size).await
        },
    );
    let page_size = pagination.size_signal();
    let mut correction_query = query.clone();
    let page_correction = use_memo(move || {
        let loaded = credentials.read();
        let (generation, page) = loaded.as_ref()?.as_ref().ok()?;
        let last_page = 1 + (page.total.max(1) - 1) / page.page_size.max(1);
        (page.page > last_page).then_some((*generation, page.page, page.page_size, last_page))
    });
    // 只有当前快照能纠偏；替换失效页的历史项，避免后退再次进入同一个空页。
    use_effect(move || {
        if let Some(correction) = page_correction()
            && let Some(corrected) =
                corrected_credential_page(correction, reload_generation(), || {
                    (query_state.peek().page, page_size())
                })
        {
            let mut next = query_state.peek().clone();
            next.page = corrected;
            correction_query.replace(next);
        }
    });

    let delete_is_current = move |credential: &AdminCredential| {
        let generation = reload_generation();
        // 删除确认冻结了凭据版本；当前页重读或翻页不能把它转成对另一版本的删除。
        credentials.read().as_ref().is_some_and(|loaded| {
            loaded.as_ref().is_ok_and(|(loaded_generation, page)| {
                *loaded_generation == generation
                    && page.credentials.iter().any(|current| {
                        current.id == credential.id
                            && current.state_version == credential.state_version
                    })
            })
        })
    };
    let open_delete = move |credential: AdminCredential| {
        if delete_is_current(&credential)
            && !pending_commands.contains(&format!("credential:{}", credential.id))
        {
            delete_feedback.set(None);
            pending_delete.set(Some(credential));
        }
    };
    let confirm_delete = move |credential: AdminCredential| {
        if !delete_is_current(&credential)
            || pending_commands.contains(&format!("credential:{}", credential.id))
        {
            return;
        }
        delete_feedback.set(None);
        // 删除与行内变更共用实体门禁；关闭确认框不能丢弃未知结果的原 UUID 和版本。
        if pending_commands.submit_with_receipt(
            format!("credential:{}", credential.id),
            t!("admin-credentials-action-delete"),
            move |command_id| {
                let credential = credential.clone();
                async move {
                    #[cfg(all(test, not(target_arch = "wasm32")))]
                    if let Some(reply) = try_consume_context::<tests::DeleteReply>() {
                        return reply.0.fetch().await;
                    }
                    delete_admin_credential(command_id, &credential).await
                }
            },
            move |result, source_alive| {
                let delivered = source_alive
                    && try_set_if_alive(
                        delete_feedback,
                        Some(CredentialFeedback::Success {
                            restart_required: result.restart_required,
                        }),
                    );
                CommandDelivery::reported(delivered, || {
                    result
                        .restart_required
                        .then(|| CommandReceipt::Text(t!("admin-credentials-action-restart")))
                })
            },
            move |code| set_if_alive(delete_feedback, Some(CredentialFeedback::Error(code))),
        ) {
            pending_delete.set(None);
        }
    };

    let list_content = match &*credentials.read_unchecked() {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("admin-credentials-loading"),
                detail: "",
            }
        },
        Some(Err(error)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: admin_api_error_message(error),
                detail: "",
                retry_label: t!("admin-retry"),
                on_retry: move |_| reload_generation.set(reload_generation().saturating_add(1)),
            }
        },
        Some(Ok((loaded_generation, page))) => {
            let mut on_page_change = query.clone();
            let mut on_sort_change = query;
            rsx! {
                    CredentialTable {
                        page: page.clone(),
                        reload_generation,
                        loaded_generation: *loaded_generation,
                        sort: query_state().sort,
                        direction: query_state().direction,
                        on_sort_change: move |(sort, direction)| {
                            let mut next = query_state();
                            next.sort = sort;
                            next.direction = direction;
                            on_sort_change.push(next);
                        },
                        on_delete: open_delete,
                    }
                    if page_correction().is_none() {
                        AdminPager {
                            page: page.page,
                            page_size: page.page_size,
                            total: Some(page.total),
                            pagination,
                            label: t!("page-credentials-title"),
                            on_page_change: move |next_page| {
                                let mut next = query_state();
                                next.page = next_page;
                                on_page_change.push(next);
                            },
                        }
                    }
            }
        }
    };
    let pending = pending_delete();

    rsx! {
        section {
            class: "admin-credentials-page",
            aria_labelledby: "credentials-title",
            h1 { id: "credentials-title", {t!("page-credentials-title")} }
            CredentialCreateForm { reload_generation }
            PendingCommandPanel { owner: pending_commands }
            {list_content}
            if let Some(feedback) = delete_feedback() {
                p {
                    class: if feedback.is_success() {
                        "admin-panel__note action-feedback action-feedback--success"
                    } else {
                        "admin-panel__note action-feedback action-feedback--error"
                    },
                    role: if feedback.is_success() { "status" } else { "alert" },
                    {feedback.message()}
                }
            }
        }
        if let Some(credential) = pending {
            CredentialDeleteDialog {
                busy: pending_commands.is_busy(&format!("credential:{}", credential.id)),
                blocked: !delete_is_current(&credential)
                    || pending_commands.contains(&format!("credential:{}", credential.id)),
                credential,
                on_confirm: confirm_delete,
                on_cancel: move |_| pending_delete.set(None),
            }
        }
    }
}

fn corrected_credential_page(
    (generation, previous_page, previous_size, last_page): (u64, i64, i64, i64),
    current_generation: u64,
    current_query: impl FnOnce() -> (i64, i64),
) -> Option<i64> {
    if generation != current_generation {
        return None;
    }
    let (page, size) = current_query();
    (page == previous_page && size == previous_size).then_some(last_page)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::cell::Cell;
    use std::cell::RefCell;
    use std::rc::Rc;

    use auction_contracts::{AdminCredentialDeleteResponse, AdminCredentialMutationResponse};
    use auction_web_shared::query::QueryLocation;
    use auction_web_shared::{QueryWriteMode, TransportError};
    use dioxus::core::{AttributeValue, Mutation};
    use dioxus::prelude::*;
    use gloo_net::http::Method;

    use super::{corrected_credential_page, page};
    use crate::api::{
        AdminCredentialPage, AdminWriteFailure, WriteDispatchStub, WriteTransportStub,
    };
    use crate::surface::auctions::editor::tests::{
        RecordRead, button, click_button, command_button, has_live_text, init_i18n, mount_writer,
        settle_writer,
    };
    use crate::surface::credentials::form::import_tests::{IMPORT_CREATED, PATH, submit_import};
    use crate::surface::credentials::table::tests::{CredentialReply, credential};
    use crate::surface::pending::{PendingCommandHost, PendingCommandNotices};
    use crate::surface::realtime::text_attributes;

    const FIRST: &str = "/cookies?page_size=10";
    const SECOND: &str = "/cookies?page_size=10&page=2";

    // 用实际历史写入模式推进内存栈，覆盖同页翻页与程序化纠偏。
    struct History {
        entries: RefCell<Vec<String>>,
        writes: RefCell<Vec<(QueryWriteMode, String)>>,
    }

    impl QueryLocation for History {
        fn current(&self) -> String {
            self.entries.borrow().last().unwrap().clone()
        }

        fn write(&self, location: &str, mode: QueryWriteMode) -> bool {
            let mut entries = self.entries.borrow_mut();
            match mode {
                QueryWriteMode::Push => entries.push(location.to_owned()),
                QueryWriteMode::Replace => *entries.last_mut().unwrap() = location.to_owned(),
            }
            self.writes.borrow_mut().push((mode, location.to_owned()));
            true
        }
    }

    #[derive(Clone)]
    pub(super) struct DeleteReply(
        pub(super) Rc<RecordRead<AdminCredentialDeleteResponse, AdminWriteFailure>>,
    );

    #[test]
    fn credential_unknown_row_writes_keep_the_entity_locked_until_success() {
        let credential = credential();
        for delete in [false, true] {
            let reads = Rc::new(RecordRead::<AdminCredentialPage>::new());
            let patch = Rc::new(RecordRead::new());
            let deletion = Rc::new(RecordRead::new());
            let (mut view, _) = mount_writer({
                let reads = reads.clone();
                let patch = patch.clone();
                let deletion = deletion.clone();
                move || {
                    init_i18n();
                    use_context_provider(|| reads.clone());
                    use_context_provider(|| CredentialReply(patch.clone()));
                    use_context_provider(|| DeleteReply(deletion.clone()));
                    let host = use_context::<PendingCommandHost>();
                    rsx! {
                        PendingCommandNotices { host }
                        {page()}
                    }
                }
            });
            reads.respond(Ok(AdminCredentialPage {
                credentials: vec![credential.clone()],
                page: 1,
                page_size: 10,
                total: 1,
                restart_required: false,
            }));
            settle_writer(&mut view);
            // 未决面板也会渲染动作名及其截断浮层，先定位行内控件。
            let actions = ["停用", "改名", "删除"].map(|action| {
                let (target, disabled) = button(&view, action).unwrap();
                assert!(!disabled, "{action}");
                (action, target)
            });
            let edits = if delete {
                let dialog = click_button(&mut view, "删除");
                let confirm = dialog
                    .iter()
                    .rev()
                    .find_map(|edit| match edit {
                        Mutation::NewEventListener { name, id } if name == "click" => Some(*id),
                        _ => None,
                    })
                    .unwrap();
                view.dispatch("click", confirm, "")
            } else {
                click_button(&mut view, "停用")
            };
            let command_id =
                text_attributes(&edits, "data-command-id").next().unwrap().1.parse().unwrap();
            if delete {
                deletion.respond(Err(AdminWriteFailure::outcome_unknown("invalid_response")));
            } else {
                patch.respond(Err("invalid_response".to_owned()));
            }
            let unknown = settle_writer(&mut view);
            let warning = "操作已完成。请重启 Worker 以应用凭据变更。";
            assert!(!has_live_text(&view, warning));
            assert_eq!(reads.started(), 1);
            assert!(command_button(&view, command_id, "原样重试").is_some());
            for (action, target) in actions {
                let rendered = edits
                    .iter()
                    .chain(&unknown)
                    .filter_map(|edit| match edit {
                        Mutation::SetAttribute { id, name: "disabled", value, .. }
                            if *id == target =>
                        {
                            Some(value)
                        }
                        _ => None,
                    })
                    .next_back();
                assert_eq!(rendered, Some(&AttributeValue::Bool(true)), "{action}");
                view.dispatch("click", target, "");
            }
            assert_eq!(
                (patch.started(), deletion.started()),
                (usize::from(!delete), usize::from(delete))
            );
            click_button(&mut view, "原样重试");
            if delete {
                assert_eq!(deletion.started(), 2);
                deletion.respond(Ok(AdminCredentialDeleteResponse {
                    credential_id: credential.id.clone(),
                    state_version: 2,
                    restart_required: true,
                }));
            } else {
                assert_eq!(patch.started(), 2);
                patch.respond(Ok(AdminCredentialMutationResponse {
                    credential: crate::api::AdminCredential {
                        state_version: 2,
                        status: "disabled".to_owned(),
                        ..credential.clone()
                    },
                    restart_required: true,
                }));
            }
            settle_writer(&mut view);
            assert!(has_live_text(&view, warning));
            assert!(command_button(&view, command_id, "原样重试").is_none());
            assert_eq!(reads.started(), 2);
        }
    }

    #[component]
    fn MountedCredentialPage() -> Element {
        page()
    }

    #[test]
    fn credential_import_reloads_the_list_once_after_returning_to_the_page() {
        let reads = Rc::new(RecordRead::<AdminCredentialPage>::new());
        let writes = Rc::new(Cell::new(0));
        let (mut view, mounted) = mount_writer({
            let reads = reads.clone();
            let writes = writes.clone();
            move || {
                init_i18n();
                use_context_provider(|| reads.clone());
                let writes = writes.clone();
                use_hook(move || {
                    WriteTransportStub::install(
                        || Ok("csrf-fixture".to_owned()),
                        move |url, method, _, _| {
                            assert_eq!(url, PATH);
                            assert_eq!(*method, Method::POST);
                            let previous = writes.get();
                            writes.set(previous + 1);
                            match previous {
                                0 => WriteDispatchStub::Failed {
                                    built: true,
                                    error: TransportError::Network(None),
                                },
                                1 => WriteDispatchStub::Response {
                                    status: 201,
                                    body: Some(IMPORT_CREATED),
                                },
                                _ => panic!("出现额外的导入派发"),
                            }
                        },
                    );
                });
                let mut visible = use_signal(|| true);
                let host = use_context::<PendingCommandHost>();
                rsx! {
                    button { onclick: move |_| visible.toggle(), {"切换凭据页".to_owned()} }
                    PendingCommandNotices { host }
                    if visible() { MountedCredentialPage {} }
                }
            }
        });
        let empty = AdminCredentialPage {
            credentials: Vec::new(),
            page: 1,
            page_size: 10,
            total: 0,
            restart_required: true,
        };
        assert_eq!(reads.started(), 1);
        reads.respond(Ok(empty.clone()));
        settle_writer(&mut view);

        let submitted = submit_import(&mut view, &mounted, false);
        let command_id = text_attributes(&submitted, "data-command-id")
            .next()
            .expect("缺少未决导入记录")
            .1
            .parse()
            .unwrap();
        assert_eq!(reads.started(), 1, "导入结果未知时不重读");
        click_button(&mut view, "切换凭据页");
        assert_eq!(reads.started(), 1, "离开凭据页后不重读");
        click_button(&mut view, "切换凭据页");
        assert_eq!(reads.started(), 2, "回页时仅启动初始读取");
        reads.respond(Ok(empty.clone()));
        settle_writer(&mut view);
        assert_eq!(reads.started(), 2, "未决记录尚未了结时不重读");
        assert_eq!(writes.get(), 1);

        let (retry, disabled) =
            command_button(&view, command_id, "原样重试").expect("回页后应保留原命令");
        assert!(!disabled);
        view.dispatch("click", retry, "");
        assert_eq!(writes.get(), 2);
        assert!(command_button(&view, command_id, "原样重试").is_none());
        assert_eq!(reads.started(), 3, "导入了结应使当前凭据列表恰好重读一次");
        let imported = credential();
        assert!(!has_live_text(&view, &imported.label));
        reads.respond(Ok(AdminCredentialPage {
            credentials: vec![imported.clone()],
            total: 1,
            ..empty
        }));
        settle_writer(&mut view);
        assert!(has_live_text(&view, &imported.label));
        assert_eq!(reads.started(), 3, "重读完成后不再次读取");
    }

    #[test]
    fn credential_page_correction_requires_the_current_read_generation() {
        let empty_second_page = (1, 2, 10, 1);
        assert_eq!(corrected_credential_page(empty_second_page, 2, || (2, 10)), None);
        assert_eq!(corrected_credential_page(empty_second_page, 1, || (2, 10)), Some(1));
        assert_eq!(corrected_credential_page(empty_second_page, 1, || (3, 10)), None);
        assert_eq!(corrected_credential_page(empty_second_page, 1, || (2, 20)), None);
    }

    #[test]
    fn credential_pagination_pushes_navigation_and_replaces_only_the_invalid_page() {
        let history = Rc::new(History {
            entries: RefCell::new(vec!["/audit".to_owned(), FIRST.to_owned()]),
            writes: RefCell::new(Vec::new()),
        });
        let reads = Rc::new(RecordRead::<AdminCredentialPage>::new());
        let (mut view, _) = mount_writer({
            let reads = reads.clone();
            let location: Rc<dyn QueryLocation> = history.clone();
            move || {
                init_i18n();
                use_context_provider(|| location.clone());
                use_context_provider(|| reads.clone());
                page()
            }
        });
        history.writes.borrow_mut().clear();
        assert_eq!(reads.started(), 1);
        let result = |page, total| AdminCredentialPage {
            credentials: Vec::new(),
            page,
            page_size: 10,
            total,
            restart_required: true,
        };
        reads.respond(Ok(result(1, 11)));
        let edits = settle_writer(&mut view);
        assert!(history.writes.borrow().is_empty());

        let next =
            text_attributes(&edits, "aria-label").find(|(_, label)| *label == "下一页").unwrap().0;
        view.dispatch("click", next, "");
        assert_eq!(history.writes.take(), vec![(QueryWriteMode::Push, SECOND.to_owned())]);
        assert_eq!(reads.started(), 2);
        reads.respond_unavailable();
        settle_writer(&mut view);
        assert!(history.writes.borrow().is_empty());
        assert_eq!(history.current(), SECOND);

        click_button(&mut view, "重试");
        assert_eq!(reads.started(), 3);
        reads.respond(Ok(result(2, 10)));
        settle_writer(&mut view);
        assert_eq!(history.writes.take(), vec![(QueryWriteMode::Replace, FIRST.to_owned())]);
        assert_eq!(reads.started(), 4);
        reads.respond(Ok(result(1, 10)));
        settle_writer(&mut view);
        assert!(history.writes.borrow().is_empty());
        assert_eq!(*history.entries.borrow(), ["/audit", FIRST, FIRST]);
    }
}
