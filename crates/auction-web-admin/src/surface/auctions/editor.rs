//! 场次与拍品编辑共用的列表读取、冲突反馈与字段聚焦。

use std::future::Future;
use std::rc::Rc;

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::surface::admin_api_error_message;
use crate::surface::pagination::{AdminPagination, use_admin_page};
use crate::surface::pending::set_if_alive;

/// 最新一轮列表读取的状态。保留的旧快照只用来展示。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::surface) enum DraftRead {
    Loading,
    Ready,
    Failed(String),
}

impl DraftRead {
    /// 只允许从成功完成的当前读取打开草稿。
    pub(in crate::surface) const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }

    /// 冲突触发的读取已经在途或完成时，关闭只丢弃旧草稿。
    pub(in crate::surface) const fn retry_after_close(&self, conflict: bool) -> bool {
        conflict && matches!(self, Self::Failed(_))
    }

    /// 冻结的版本只用于核对；列表换页而未找到实体不能证明其版本变化。
    pub(in crate::surface) fn version_changed(&self, opened: i64, latest: Option<i64>) -> bool {
        self.is_ready() && latest.is_some_and(|version| version != opened)
    }
}

/// 草稿由页面单独持有。这里仅在同一查询下保留成功快照，供刷新期间或编辑中的读取失败展示。
pub(in crate::surface) fn use_draft_page<Q, T, F>(
    pagination: AdminPagination,
    reload: Signal<u64>,
    editing: bool,
    request: impl Fn() -> Q + 'static,
    fetch: impl Fn(Q, i64) -> F + 'static,
) -> (DraftRead, Option<Result<Rc<T>, String>>)
where
    Q: Clone + PartialEq + 'static,
    T: PartialEq + 'static,
    F: Future<Output = Result<T, String>> + 'static,
{
    let query = use_memo(request);
    let page_size = pagination.size_signal();
    let snapshot = use_signal(|| None::<(Q, i64, Rc<T>)>);
    let result = use_admin_page(
        pagination,
        move || (query(), reload()),
        move |(identity, generation), size| {
            let future = fetch(identity.clone(), size);
            let mut snapshot = snapshot;
            async move {
                let result = future.await.map(Rc::new);
                if let Ok(page) = &result
                    && *query.peek() == identity
                    && *page_size.peek() == size
                    && *reload.peek() == generation
                {
                    // 旧查询或旧代次完成时不能覆盖当前页的保留快照。
                    snapshot.set(Some((identity, size, Rc::clone(page))));
                }
                result
            }
        },
    );

    let current = result();
    let read = match &current {
        None => DraftRead::Loading,
        Some(Ok(_)) => DraftRead::Ready,
        Some(Err(code)) => DraftRead::Failed(code.clone()),
    };
    if current.is_none() || editing && matches!(read, DraftRead::Failed(_)) {
        let snapshot = snapshot.read();
        if let Some((identity, size, page)) = snapshot.as_ref()
            && *identity == *query.read()
            && *size == page_size()
        {
            return (read, Some(Ok(Rc::clone(page))));
        }
    }
    (read, current)
}

/// 对话框内分别反馈版本冲突、写入错误与读取状态。
#[component]
pub(in crate::surface) fn DraftFeedback(
    conflict: bool,
    read: DraftRead,
    failure: Option<String>,
) -> Element {
    if conflict {
        return rsx! {
            p { role: "alert", class: "action-feedback action-feedback--error",
                {t!("admin-record-stale")}
            }
        };
    }
    rsx! {
        if let Some(message) = failure.as_deref().map(admin_api_error_message) {
            p { role: "alert", class: "action-feedback action-feedback--error", "{message}" }
        }
        match read {
            DraftRead::Ready => rsx! {},
            DraftRead::Loading => rsx! {
                p { role: "status", {t!("admin-record-refresh-loading")} }
            },
            DraftRead::Failed(code) => rsx! {
                p { role: "alert", class: "action-feedback action-feedback--error",
                    {t!("admin-record-refresh-failed", detail: admin_api_error_message(&code))}
                }
            },
        }
    }
}

/// 普通读取保留草稿；明确发生版本冲突时只能关闭草稿，再从刷新的行创建新意图。
#[component]
pub(in crate::surface) fn DraftRefreshButton(
    read: DraftRead,
    conflict: bool,
    mut reload: Signal<u64>,
    on_close: EventHandler<()>,
) -> Element {
    let loading = !conflict && read == DraftRead::Loading;
    let label = if read.retry_after_close(conflict) {
        t!("admin-record-close-and-refresh")
    } else if conflict {
        t!("admin-record-close")
    } else if matches!(read, DraftRead::Failed(_)) {
        t!("admin-retry")
    } else {
        t!("admin-record-refresh")
    };
    rsx! {
        button {
            r#type: "button",
            class: if conflict { "admin-btn-primary" } else { "" },
            disabled: loading,
            onclick: move |_| {
                if loading { return; }
                if conflict {
                    on_close.call(());
                } else {
                    reload.set(reload().saturating_add(1));
                }
            },
            "{label}"
        }
    }
}

/// 冲突后的权威读取失败才在关闭时重试，未完成的读取继续约束新草稿。
pub(in crate::surface) fn close_edit_draft<T: 'static>(
    mut draft: Signal<Option<T>>,
    mut reload: Signal<u64>,
    retry: bool,
) {
    draft.set(None);
    if retry {
        reload.set(reload().saturating_add(1));
    }
}

/// 保留回执分类证据，避免把准备阶段的同名错误当成写命令的版本拒绝。
pub(in crate::surface) async fn observe_draft_write<T>(
    request: impl Future<Output = Result<T, String>>,
    feedback: Signal<Option<String>>,
) -> Result<T, String> {
    let result = request.await;
    set_if_alive(feedback, result.as_ref().err().cloned());
    result
}

/// 从未决区完成的旧命令只收起自己的草稿。
pub(in crate::surface) fn close_matching_draft<T: 'static>(
    mut draft: Signal<Option<T>>,
    matches: impl FnOnce(&T) -> bool,
) {
    if let Ok(mut current) = draft.try_write()
        && current.as_ref().is_some_and(matches)
    {
        *current = None;
    }
}

/// 动态字段 ID 也须等错误节点及 ARIA 属性写入 DOM 后才聚焦。
pub(in crate::surface) fn focus_draft_field_after_render(_field_id: String) {
    #[cfg(target_arch = "wasm32")]
    spawn(async move {
        auction_web_shared::timer::TimeoutFuture::new(0).await;
        auction_web_shared::a11y::focus_element_by_id(&_field_id);
    });
}

#[cfg(test)]
pub(in crate::surface) mod tests {
    use std::assert_matches;
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::future::poll_fn;
    use std::rc::Rc;
    use std::task::{Poll, Waker};

    use auction_contracts::ApiErrorResponse;
    use auction_web_shared::TransportError;
    use auction_web_shared::i18n::{DEFAULT_ADMIN_LOCALE, LocaleSession, build_bundle};
    use dioxus::dioxus_core::{AttributeValue, DynamicNode, ElementId, Mutation, VNode};
    use dioxus::prelude::{Element, VirtualDom, use_context_provider, use_hook, use_memo};
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;
    use gloo_net::http::Method;
    use uuid::Uuid;

    use crate::api::{AdminApiFailure, AdminSessionWritePermission};
    use crate::surface::ADMIN_LOCALE_CATALOG;
    use crate::surface::realtime::{TestPage, submit_forms};

    /// 冲突态只有标题栏显示「取消」；它和 Esc 共用 Dialog 的 on_close。
    pub(in crate::surface) const DIALOG_TITLE_CLOSE: &str = "取消";

    type WriteResponse = Result<(u16, String), TransportError>;
    type TestRoot = Rc<dyn Fn() -> Element>;

    thread_local! {
        static WRITE_ROOT: RefCell<Option<TestRoot>> = RefCell::new(None);
    }

    /// 复用 Admin 现有的进程级事件转换器，测试只安装本页的请求替身。
    pub(in crate::surface) fn mount_writer(
        root: impl Fn() -> Element + 'static,
    ) -> (TestPage, Vec<Mutation>) {
        mount_writer_at("", root)
    }

    pub(in crate::surface) fn mount_writer_at(
        bookmark: &str,
        root: impl Fn() -> Element + 'static,
    ) -> (TestPage, Vec<Mutation>) {
        WRITE_ROOT.with(|slot| assert!(slot.replace(Some(Rc::new(root))).is_none()));
        TestPage::mount(writer_root, bookmark)
    }

    fn writer_root() -> Element {
        let root = use_hook(|| WRITE_ROOT.with(|slot| slot.take().expect("测试页工厂")));
        let permission = use_memo(|| true);
        use_context_provider(|| AdminSessionWritePermission(permission));
        root()
    }

    pub(in crate::surface) fn settle_writer(page: &mut TestPage) -> Vec<Mutation> {
        // 让资源的完成事件传播到对话框，不向表单派发提交。
        page.dispatch("input", ElementId(0), "")
    }

    pub(in crate::surface) fn writer_form(edits: &[Mutation]) -> ElementId {
        submit_forms(edits).next().expect("编辑表单已挂载")
    }

    pub(in crate::surface) fn button(page: &TestPage, label: &str) -> Option<(ElementId, bool)> {
        let dom = page.dom();
        button_in(dom, dom.base_scope().root_node(), label)
    }

    /// 表单提交按钮的点击由表单处理，按钮自身没有 `onclick`。
    pub(in crate::surface) fn submit_button_disabled(page: &TestPage, label: &str) -> Option<bool> {
        let dom = page.dom();
        find_in_tree(dom, dom.base_scope().root_node(), &|vnode| {
            vnode.dynamic_nodes.iter().enumerate().find_map(|(index, node)| {
                if !matches!(node, DynamicNode::Text(text) if text.value == label) {
                    return None;
                }
                let (_, button_path) = vnode.template.node_paths[index].split_last()?;
                vnode
                    .template
                    .attr_paths
                    .iter()
                    .zip(&vnode.dynamic_attrs)
                    .filter(|(path, _)| **path == button_path)
                    .flat_map(|(_, attributes)| attributes.iter())
                    .find_map(|attribute| match (&attribute.value, attribute.name) {
                        (AttributeValue::Bool(disabled), "disabled") => Some(*disabled),
                        _ => None,
                    })
            })
        })
    }

    pub(in crate::surface) fn click_button(page: &mut TestPage, label: &str) -> Vec<Mutation> {
        let (id, disabled) = button(page, label).unwrap_or_else(|| panic!("缺少按钮「{label}」"));
        assert!(!disabled, "按钮「{label}」应可点击");
        page.dispatch("click", id, "")
    }

    pub(in crate::surface) fn has_live_text(page: &TestPage, expected: &str) -> bool {
        let dom = page.dom();
        find_in_tree(dom, dom.base_scope().root_node(), &|node| {
            node.dynamic_nodes
                .iter()
                .any(|node| matches!(node, DynamicNode::Text(text) if text.value == expected))
                .then_some(())
        })
        .is_some()
    }

    pub(in crate::surface) fn assert_field(
        page: &TestPage,
        field: ElementId,
        value: &str,
        read_only: bool,
    ) {
        let dom = page.dom();
        let attribute = |name| {
            find_in_tree(dom, dom.base_scope().root_node(), &|node| {
                node.dynamic_attrs.iter().enumerate().find_map(|(index, attrs)| {
                    if node.mounted_dynamic_attribute(index, dom) != Some(field) {
                        return None;
                    }
                    attrs.iter().find(|attr| attr.name == name).map(|attr| attr.value.clone())
                })
            })
        };
        assert_matches!(attribute("value"), Some(AttributeValue::Text(text)) if text == value);
        assert_eq!(matches!(attribute("readonly"), Some(AttributeValue::Bool(true))), read_only);
        assert!(!matches!(attribute("disabled"), Some(AttributeValue::Bool(true))));
        if read_only && let Some(AttributeValue::Text(kind)) = attribute("type") {
            assert_eq!(kind, "text");
        }
    }

    pub(in crate::surface) fn command_button(
        page: &TestPage,
        command_id: Uuid,
        label: &str,
    ) -> Option<(ElementId, bool)> {
        let dom = page.dom();
        let command_id = command_id.to_string();
        find_in_tree(dom, dom.base_scope().root_node(), &|row| {
            let matches_command = row.dynamic_attrs.iter().flatten().any(|attr| {
                attr.name == "data-command-id"
                    && matches!(&attr.value, AttributeValue::Text(value) if *value == command_id)
            });
            matches_command.then(|| button_in(dom, row, label)).flatten()
        })
    }

    fn button_in(dom: &VirtualDom, root: &VNode, label: &str) -> Option<(ElementId, bool)> {
        find_in_tree(dom, root, &|vnode| {
            vnode.dynamic_nodes.iter().enumerate().find_map(|(index, node)| {
                if !matches!(node, DynamicNode::Text(text) if text.value == label) {
                    return None;
                }
                let (_, button_path) = vnode.template.node_paths[index].split_last()?;
                let attributes_at_path = vnode
                    .template
                    .attr_paths
                    .iter()
                    .zip(&vnode.dynamic_attrs)
                    .enumerate()
                    .filter(|(_, (path, _))| **path == button_path);
                let listener = attributes_at_path.clone().find_map(|(index, (_, attrs))| {
                    attrs.iter().any(|attr| attr.name == "onclick").then_some(index)
                })?;
                let disabled =
                    attributes_at_path.flat_map(|(_, (_, attrs))| attrs.iter()).any(|attr| {
                        attr.name == "disabled" && matches!(attr.value, AttributeValue::Bool(true))
                    });
                Some((vnode.mounted_dynamic_attribute(listener, dom)?, disabled))
            })
        })
    }

    fn find_in_tree<T>(
        dom: &VirtualDom,
        vnode: &VNode,
        find: &impl Fn(&VNode) -> Option<T>,
    ) -> Option<T> {
        find(vnode).or_else(|| {
            vnode.dynamic_nodes.iter().enumerate().find_map(|(index, node)| match node {
                DynamicNode::Component(component) => {
                    let child = component.mounted_scope(index, vnode, dom)?.try_root_node()?;
                    find_in_tree(dom, child, find)
                }
                DynamicNode::Fragment(children) => {
                    children.iter().find_map(|child| find_in_tree(dom, child, find))
                }
                DynamicNode::Text(_) | DynamicNode::Placeholder(_) => None,
            })
        })
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(in crate::surface) struct CapturedWrite {
        pub(in crate::surface) method: Method,
        pub(in crate::surface) url: String,
        pub(in crate::surface) body: Vec<u8>,
    }

    impl CapturedWrite {
        pub(in crate::surface) fn command_id(&self) -> Uuid {
            let data: serde_json::Value = serde_json::from_slice(&self.body).unwrap();
            let command_id = data["command_id"].as_str().unwrap().parse::<Uuid>().unwrap();
            assert_eq!(command_id.get_version_num(), 4);
            command_id
        }
    }

    #[derive(Clone, Default)]
    pub(in crate::surface) struct WriteScript {
        replies: Rc<RefCell<VecDeque<WriteResponse>>>,
        writes: Rc<RefCell<Vec<CapturedWrite>>>,
    }

    impl WriteScript {
        pub(in crate::surface) fn install(&self) {
            let script = self.clone();
            AdminApiFailure::stub_auction_edit(move |method, url, body| {
                script.writes.borrow_mut().push(CapturedWrite {
                    method,
                    url: url.to_owned(),
                    body: body.to_vec(),
                });
                script.replies.borrow_mut().pop_front().expect("请求对应一条脚本回执")
            });
        }

        pub(in crate::surface) fn reply_http(&self, status: u16, body: impl Into<String>) {
            self.replies.borrow_mut().push_back(Ok((status, body.into())));
        }

        pub(in crate::surface) fn reply_conflict(&self, code: &str) {
            self.reply_http(
                409,
                serde_json::to_string(&ApiErrorResponse {
                    code: code.to_owned(),
                    message: "版本已变化".to_owned(),
                    data: None,
                    request_id: Some("review-fixture".to_owned()),
                })
                .unwrap(),
            );
        }

        pub(in crate::surface) fn reply_timeout(&self) {
            self.replies.borrow_mut().push_back(Err(TransportError::Timeout));
        }

        pub(in crate::surface) fn take_writes(&self) -> Vec<CapturedWrite> {
            self.writes.take()
        }
    }

    pub(in crate::surface) struct RecordRead<T> {
        started: Cell<usize>,
        result: RefCell<Option<Result<T, String>>>,
        waiting: RefCell<Option<Waker>>,
    }

    impl<T> RecordRead<T> {
        pub(in crate::surface) fn new() -> Self {
            Self { started: Cell::new(0), result: RefCell::new(None), waiting: RefCell::new(None) }
        }

        pub(in crate::surface) fn started(&self) -> usize {
            self.started.get()
        }

        pub(in crate::surface) async fn fetch(&self) -> Result<T, String> {
            self.started.set(self.started.get() + 1);
            poll_fn(|context| {
                if let Some(result) = self.result.borrow_mut().take() {
                    Poll::Ready(result)
                } else {
                    self.waiting.replace(Some(context.waker().clone()));
                    Poll::Pending
                }
            })
            .await
        }

        pub(in crate::surface) fn respond(&self, result: Result<T, String>) {
            assert!(self.result.replace(Some(result)).is_none());
            self.waiting.borrow_mut().take().expect("读取已启动").wake();
        }

        pub(in crate::surface) fn respond_unavailable(&self) {
            self.respond(Err(AdminApiFailure::from_http_body(
                503,
                r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
            ).code));
        }
    }

    pub(in crate::surface) fn init_i18n() {
        let bundle = try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_ADMIN_LOCALE,
                ADMIN_LOCALE_CATALOG.default_ftl,
                &DEFAULT_ADMIN_LOCALE,
                None,
            )
        })
        .unwrap();
        let session = use_hook(|| LocaleSession::new(bundle, DEFAULT_ADMIN_LOCALE));
        use_context_provider(|| session);
    }

    pub(in crate::surface) fn has_text(edits: &[Mutation], text: &str) -> bool {
        edits.iter().any(|edit| match edit {
            Mutation::CreateTextNode { value, .. } | Mutation::SetText { value, .. } => {
                value.contains(text)
            }
            _ => false,
        })
    }
}
