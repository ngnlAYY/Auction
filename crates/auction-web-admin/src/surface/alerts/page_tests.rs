//! 告警页的宿主渲染回归，替身只记录发送并提供原始回执。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use auction_web_shared::{LocaleSession, TransportError, build_bundle};
use dioxus::dioxus_core::{AttributeValue, DynamicNode, ElementId, Mutation, VNode};
use dioxus::prelude::*;
use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::api::{
    AdminApiFailure, AdminSessionWritePermission, AdminWriteFailure, acknowledge_admin_alert,
    silence_admin_alert,
};
use crate::surface::ADMIN_LOCALE_CATALOG;
use crate::surface::auctions::editor::tests::{button, click_button, mount_writer};
use crate::surface::pending::{
    PendingCommandNotices, PendingCommandPanel, use_pending_command_host, use_pending_commands,
};
use crate::surface::test_events::{dispatch_click_event, dispatch_form_event};

const N: i64 = 2;
const ALERT_ID: &str = "abababababababababababababababababababababababababababababababab";
const STALE: &str =
    r#"{"code":"alert_stale","message":"告警状态已变化","data":null,"request_id":null}"#;

#[derive(Default)]
struct Requests {
    recurred: Cell<bool>,
    reads: RefCell<Vec<String>>,
    writes: RefCell<Vec<(String, String)>>,
}

impl Requests {
    fn reply(&self, url: &str, body: Option<&str>) -> Result<(u16, String), TransportError> {
        if let Some(body) = body {
            let mut writes = self.writes.borrow_mut();
            writes.push((url.to_owned(), body.to_owned()));
            return match writes.len() {
                1 => Ok((503, r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#.to_owned())),
                2 => Ok((409, STALE.to_owned())),
                3 => {
                    let (fingerprint, action) =
                        url.strip_prefix("/api/v1/admin/alerts/").unwrap().split_once('/').unwrap();
                    let silenced_until = (action == "silence").then_some("2026-11-19T12:00:00Z");
                    Ok((
                        200,
                        json!({"code":"ok", "message":"ok", "data":{"alert":{
                            "id": fingerprint, "action": action, "replayed": false,
                            "silenced_until": silenced_until
                        }}})
                        .to_string(),
                    ))
                }
                _ => panic!("意外的写请求：{url}"),
            };
        }
        if url == "/api/v1/admin/alerts/webhooks?limit=50" {
            return Ok((200, r#"{"code":"ok","message":"ok","data":{"entries":[]}}"#.to_owned()));
        }
        let (path, query) = url.split_once('?').unwrap();
        assert_eq!(path, "/api/v1/admin/alerts");
        let pairs: std::collections::BTreeMap<_, _> =
            url::form_urlencoded::parse(query.as_bytes()).collect();
        assert_eq!(pairs["include"], "incident_generation");
        assert_eq!(pairs["page"], "1");
        let status = pairs["status"].as_ref();
        self.reads.borrow_mut().push(status.to_owned());
        let mut body: Value = serde_json::from_str(include_str!(
            "../../../../auction-contracts/tests/fixtures/admin/alerts-list.json"
        ))
        .unwrap();
        let data = &mut body["data"];
        let mut alert = data["alerts"][0].clone();
        if self.recurred.get() {
            alert["incident_generation"] = json!(N + 1);
            alert["count"] = json!(18);
        }
        data["page_size"] = json!(pairs["page_size"].parse::<i64>().unwrap());
        data["active_total"] = json!(1);
        if status == "resolved" {
            data["total"] = json!(0);
            data["alerts"] = json!([]);
        } else {
            data["total"] = json!(1);
            data["alerts"] = json!([alert]);
        }
        Ok((200, body.to_string()))
    }
}

fn test_page() -> Element {
    let catalog = &ADMIN_LOCALE_CATALOG;
    let bundle = try_use_init_i18n_from_bundle(|| {
        build_bundle(&catalog.default_locale, catalog.default_ftl, &catalog.default_locale, None)
    })
    .unwrap();
    use_context_provider(|| LocaleSession::new(bundle, catalog.default_locale.clone()));
    let writes_allowed = use_memo(|| true);
    use_context_provider(|| AdminSessionWritePermission(writes_allowed));
    let requests = consume_context::<Rc<Requests>>();
    use_hook(move || {
        AdminApiFailure::stub_alert_responses(move |url, body| requests.reply(url, body));
    });
    let mut alerts = use_signal(|| true);
    let host = use_pending_command_host();
    rsx! {
        button { onclick: move |_| alerts.toggle(), "切换测试路由" }
        PendingCommandNotices { host }
        if alerts() { AlertRoute {} }
    }
}

#[component]
fn AlertRoute() -> Element {
    super::page()
}

#[derive(Clone, Copy)]
struct Control {
    id: ElementId,
    disabled: bool,
}

struct Page(VirtualDom);

impl Page {
    fn mount(requests: Rc<Requests>) -> Self {
        let mut page = Self(VirtualDom::new(test_page).with_root_context(requests));
        page.0.rebuild_to_vec();
        page.render();
        page
    }

    fn root(&self) -> &VNode {
        self.0.get_scope(ScopeId::APP).unwrap().root_node()
    }

    fn component(&self, name: &str) -> Option<&VNode> {
        find_component(&self.0, self.root(), name)
    }

    fn controls(&self, node: &VNode, listener: &str) -> Vec<Control> {
        let mut controls = Vec::new();
        collect_controls(&self.0, node, listener, &mut controls);
        controls
    }

    fn event(&mut self, name: &str, target: ElementId, value: &str) -> Vec<Mutation> {
        if name == "click" {
            dispatch_click_event(&self.0, target);
        } else {
            dispatch_form_event(&self.0, target, name, value);
        }
        self.render()
    }

    fn filter(&mut self, status: &str) -> Vec<Mutation> {
        let select = self.controls(self.component("AlertRoute").unwrap(), "onchange");
        assert_eq!(select.len(), 1);
        self.event("change", select[0].id, status)
    }

    fn reason(&self) -> ElementId {
        self.controls(self.component("AlertReasonInput").unwrap(), "oninput")[0].id
    }

    fn row_actions(&self, disabled: bool) -> [ElementId; 2] {
        let [confirm, silence] = self.controls(self.component("AlertRow").unwrap(), "onclick")[..]
        else {
            panic!("活动告警应有确认与静默两个按钮");
        };
        assert_eq!([confirm.disabled, silence.disabled], [disabled, disabled]);
        [confirm.id, silence.id]
    }

    fn set_silence_hours(&mut self, hours: &str) {
        let selectors = self.controls(self.component("AlertRow").unwrap(), "onchange");
        assert_eq!(selectors.len(), 1);
        self.event("change", selectors[0].id, hours);
    }

    fn navigate(&mut self) -> Vec<Mutation> {
        let button = self.controls(self.root(), "onclick")[0].id;
        self.event("click", button, "")
    }

    fn pending_action(&self) -> ElementId {
        let actions = self.controls(self.component("PendingCommandPanel").unwrap(), "onclick");
        assert_eq!(actions.len(), 1);
        assert!(!actions[0].disabled);
        actions[0].id
    }

    fn render(&mut self) -> Vec<Mutation> {
        let mut edits = Vec::new();
        for _ in 0..3 {
            edits.extend(self.0.render_immediate_to_vec().edits);
        }
        edits
    }
}

fn find_component<'a>(dom: &'a VirtualDom, node: &'a VNode, name: &str) -> Option<&'a VNode> {
    node.dynamic_nodes.iter().enumerate().find_map(|(index, child)| match child {
        DynamicNode::Component(component) => {
            let root = component.mounted_scope(index, node, dom)?.try_root_node()?;
            if component.name.rsplit("::").next() == Some(name) {
                Some(root)
            } else {
                find_component(dom, root, name)
            }
        }
        DynamicNode::Fragment(nodes) => {
            nodes.iter().find_map(|node| find_component(dom, node, name))
        }
        _ => None,
    })
}

fn collect_controls(dom: &VirtualDom, node: &VNode, listener: &str, controls: &mut Vec<Control>) {
    let slots = node.template.attr_paths.iter().zip(node.dynamic_attrs.iter());
    for (index, (path, attrs)) in slots.clone().enumerate() {
        if attrs.iter().any(|attr| attr.name == listener) {
            let id = node.mounted_dynamic_attribute(index, dom).unwrap();
            let disabled = slots.clone().filter(|(other, _)| other == &path).any(|(_, attrs)| {
                attrs
                    .iter()
                    .any(|attr| attr.name == "disabled" && attr.value == AttributeValue::Bool(true))
            });
            controls.push(Control { id, disabled });
        }
    }
    for child in &node.dynamic_nodes {
        if let DynamicNode::Fragment(nodes) = child {
            for node in nodes {
                collect_controls(dom, node, listener, controls);
            }
        }
    }
}

fn updated_text(edits: &[Mutation], prefix: &str) -> usize {
    edits
        .iter()
        .filter(|edit| match edit {
            Mutation::CreateTextNode { value, .. } | Mutation::SetText { value, .. } => {
                value.starts_with(prefix)
            }
            _ => false,
        })
        .count()
}

fn check_retry(action: &str, action_index: usize) {
    let requests = Rc::new(Requests::default());
    let mut page = Page::mount(requests.clone());
    page.event("input", page.reason(), "  第一轮备注  ");
    if action == "silence" {
        page.set_silence_hours("8");
    }
    let timed_out = page.event("click", page.row_actions(false)[action_index], "");
    assert_eq!(updated_text(&timed_out, "曾有请求结果未知"), 1);
    assert_eq!(updated_text(&timed_out, "原样重试"), 2);
    page.row_actions(true);
    page.pending_action();
    assert_eq!(*requests.reads.borrow(), ["active"]);

    requests.recurred.set(true);
    let reread = page.filter("all");
    assert_eq!(updated_text(&reread, "18"), 1);
    let retry = page.pending_action();
    page.row_actions(true);
    assert_eq!(*requests.reads.borrow(), ["active", "all"]);
    assert_eq!(requests.writes.borrow().len(), 1);
    page.event("input", page.reason(), "第二轮备注");

    let rejected = page.event("click", retry, "");
    // 页面反馈与未决面板都说明要以重读后的告警为准。
    assert_eq!(updated_text(&rejected, "告警状态已变化。请核对重新读取后的列表"), 2);
    assert_eq!(updated_text(&rejected, "关闭已拒绝操作"), 1);
    assert_eq!(*requests.reads.borrow(), ["active", "all", "all"]);
    {
        let writes = requests.writes.borrow();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0], writes[1]);
    }

    if action == "silence" {
        page.set_silence_hours("1");
    }
    let succeeded = page.event("click", page.row_actions(false)[action_index], "");
    assert_eq!(updated_text(&succeeded, "操作已记录"), 1);
    assert!(page.controls(page.component("PendingCommandPanel").unwrap(), "onclick").is_empty());
    assert_eq!(*requests.reads.borrow(), ["active", "all", "all", "all"]);
    let writes = requests.writes.borrow();
    assert_eq!(writes.len(), 3);
    let url = format!("/api/v1/admin/alerts/{ALERT_ID}/{action}");
    assert_eq!(writes[0].0, url);
    assert_eq!(writes[2].0, url);
    let original: Value = serde_json::from_str(&writes[0].1).unwrap();
    let next: Value = serde_json::from_str(&writes[2].1).unwrap();
    assert_eq!(original["incident_generation"], N);
    assert_eq!(next["incident_generation"], N + 1);
    assert_eq!(original["reason"], "第一轮备注");
    assert_eq!(next["reason"], "第二轮备注");
    if action == "silence" {
        assert_eq!(original["silence_seconds"], 28_800);
        assert_eq!(next["silence_seconds"], 3_600);
    }
    let original_id = Uuid::parse_str(original["command_id"].as_str().unwrap()).unwrap();
    let next_id = Uuid::parse_str(next["command_id"].as_str().unwrap()).unwrap();
    assert_ne!(original_id, next_id);
}

#[test]
fn acknowledge_retries_original_incident_and_allows_new_command_after_stale() {
    check_retry("acknowledge", 0);
}

#[test]
fn silence_retries_original_incident_and_allows_new_command_after_stale() {
    check_retry("silence", 1);
}

#[test]
fn silence_cannot_replace_unknown_after_leaving_and_returning() {
    let requests = Rc::new(Requests::default());
    let mut page = Page::mount(requests.clone());
    page.event("input", page.reason(), "原始备注");
    page.event("click", page.row_actions(false)[1], "");
    page.navigate();
    assert!(page.component("AlertRoute").is_none());
    assert!(page.component("PendingCommandNotices").is_some());
    page.navigate();
    let [acknowledge, silence] = page.row_actions(true);
    page.event("click", acknowledge, "");
    page.event("click", silence, "");
    assert_eq!(requests.writes.borrow().len(), 1);

    page.navigate();
    let shell = page.component("PendingCommandNotices").unwrap();
    let [retry, abandon] = page.controls(shell, "onclick")[..] else {
        panic!("切页后外壳仍应提供原样重试与放弃");
    };
    assert!(!retry.disabled && !abandon.disabled);
    let rejected = page.event("click", retry.id, "");
    assert_eq!(updated_text(&rejected, "关闭回执"), 1);
    let writes = requests.writes.borrow();
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[0], writes[1]);
    assert_eq!(*requests.reads.borrow(), ["active", "active"]);
}

#[test]
fn reason_announcement_survives_empty_filter_until_the_draft_is_corrected() {
    let requests = Rc::new(Requests::default());
    let mut page = Page::mount(requests.clone());
    let too_long = "告".repeat(683);
    let edits = page.event("input", page.reason(), &too_long);
    assert_eq!(updated_text(&edits, "备注过长"), 0);
    let edits = page.event("blur", page.reason(), "");
    // 可见错误和 live region 各有一条相同的译文。
    assert_eq!(updated_text(&edits, "备注过长"), 2);

    let empty = page.filter("resolved");
    assert_eq!(updated_text(&empty, "没有符合条件的告警"), 1);
    assert!(page.component("AlertReasonInput").is_none());
    let remounted = page.filter("active");
    let reason = page.reason();
    assert!(remounted.iter().any(|edit| matches!(edit,
        Mutation::SetAttribute { id, name: "value", value: AttributeValue::Text(value), .. }
            if *id == reason && *value == too_long
    )));
    page.event("focus", reason, "");
    let edits = page.event("blur", reason, "");
    assert_eq!(updated_text(&edits, "备注过长"), 0);
    assert_eq!(updated_text(&remounted, "备注过长"), 1);

    let blocked_submit = page.event("click", page.row_actions(false)[0], "");
    assert!(requests.writes.borrow().is_empty());
    assert_eq!(updated_text(&blocked_submit, "备注过长"), 1);

    page.event("input", reason, "已经修正");
    page.event("input", reason, &too_long);
    let edits = page.event("blur", reason, "");
    assert_eq!(updated_text(&edits, "备注过长"), 2);
    let edits = page.event("blur", reason, "");
    assert_eq!(updated_text(&edits, "备注过长"), 0);
}

fn check_preflight_stale(action: &'static str) {
    let requests = Rc::new(Requests::default());
    let attempts = Rc::new(Cell::new(0));
    let (mut page, _) = mount_writer({
        let requests = requests.clone();
        let attempts = attempts.clone();
        move || {
            let requests = requests.clone();
            use_hook(move || {
                AdminApiFailure::stub_alert_responses(move |url, body| requests.reply(url, body));
            });
            let entity = format!("alert:{ALERT_ID}");
            let reload = use_signal(|| 0_u64);
            let owner = use_pending_commands(reload, |entity| entity.starts_with("alert:"));
            let attempts = attempts.clone();
            let label = "处置告警";
            rsx! {
                PendingCommandPanel { owner }
                button {
                    disabled: owner.contains(&entity),
                    onclick: move |_| {
                        let attempts = attempts.clone();
                        owner.submit(entity.clone(), label.into(),
                            move |command_id| {
                                let attempt = attempts.get() + 1;
                                attempts.set(attempt);
                                async move {
                                    if attempt == 2 {
                                        // 告警写响应替身在 CSRF 复核之前截获 POST；从只读失败边界交给宿主。
                                        let before_write = AdminApiFailure::from_http_body(409, STALE);
                                        return Err(AdminWriteFailure::not_sent(before_write.code));
                                    }
                                    if action == "acknowledge" {
                                        acknowledge_admin_alert(command_id, ALERT_ID, N, "原始备注").await
                                    } else {
                                        silence_admin_alert(command_id, ALERT_ID, N, 3_600, "原始备注").await
                                    }
                                }
                            },
                            |_| panic!("冲突流程不应交付成功回执"),
                            |_| (),
                        );
                    },
                    "{label}"
                }
            }
        }
    });
    click_button(&mut page, "处置告警");
    click_button(&mut page, "原样重试");
    assert_eq!(attempts.get(), 2);
    assert_eq!(requests.writes.borrow().len(), 1);
    assert!(button(&page, "关闭已拒绝操作").is_none());
    assert!(button(&page, "处置告警").unwrap().1);

    click_button(&mut page, "原样重试");
    assert_eq!(attempts.get(), 3);
    assert!(button(&page, "关闭已拒绝操作").is_some());
    assert!(!button(&page, "处置告警").unwrap().1);
    let writes = requests.writes.borrow();
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[0], writes[1]);
    assert_eq!(writes[0].0, format!("/api/v1/admin/alerts/{ALERT_ID}/{action}"));
}

#[test]
fn acknowledge_preflight_stale_retains_the_unknown_command() {
    check_preflight_stale("acknowledge");
}

#[test]
fn silence_preflight_stale_retains_the_unknown_command() {
    check_preflight_stale("silence");
}
