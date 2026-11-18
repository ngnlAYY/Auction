//! Admin 审计历史页面。
//! 审计事实只读且追加式；筛选和分页请求通过 HTTP 安全 DTO 完成，浏览器不修改记录、不接触
//! 数据库模型，也不把 metadata 当作可执行内容。

use std::cmp::Ordering;

use auction_time::Timestamp;
use auction_web_shared::{AsyncState, AsyncStateKind, Time, TruncatedText};
use auction_web_shared::{a11y::focus_element_by_id_after_render, t};
use dioxus::prelude::*;

use super::admin_api_error_message;
use super::pagination::{AdminPager as Pager, use_admin_page, use_admin_pagination};
use super::query::{AuditQuery, use_audit_query_state};
use super::realtime::{
    FilterValue, QueryFilterInput, QueryFilterSelect, fetch_filtered, normalize_filter_input,
    normalize_filter_text,
};
use super::sort::{MobileSortControl, SortDirection, SortableTh, sort_rows};
use crate::api::{
    AdminAuditFilter, AdminAuditPage, AdminAuditRecord, datetime_local_value, fetch_admin_audit,
};

pub(super) fn page() -> Element {
    // 已应用的筛选与分页跟地址栏双向同步；表单草稿是提交前的瞬态，只在提交时入 URL。
    let query = use_audit_query_state();
    let query_state = query.state;
    let requested_query = use_memo(move || {
        let current: &AuditQuery = &query_state.read();
        current.page_request()
    });
    let applied_filter = use_memo(move || query_state.read().filter.clone());
    let mut refresh = use_signal(|| 0_u64);
    let pagination = use_admin_pagination("audit", &["page_size"], Some(200), i64::from(i32::MAX));
    let audit = use_admin_page(
        pagination.clone(),
        move || {
            let _ = refresh();
            requested_query()
        },
        |request, size| {
            fetch_filtered(request, move |(page, filter)| fetch_admin_audit(page, filter, size))
        },
    );

    let mut apply_query = query.clone();
    let apply_filter = move |next_filter: AdminAuditFilter| {
        // 提交筛选回到第一页并保留排序；查询资源响应状态变化自动重新加载。
        apply_query.push(query_state().with_filter(next_filter));
    };
    let content = match &*audit.read_unchecked() {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("admin-audit-loading"),
                detail: "",
            }
        },
        Some(Err(error)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: admin_api_error_message(error),
                detail: "",
                retry_label: t!("admin-audit-retry"),
                on_retry: move |_| refresh.set(refresh().saturating_add(1)),
            }
        },
        Some(Ok(page)) => {
            let mut on_page_change = query.clone();
            let mut on_sort_change = query;
            rsx! {
            AuditTable {
                page: page.clone(),
                sort: query_state().sort,
                direction: query_state().direction,
                on_sort_change: move |(sort, direction)| {
                    let mut next = query_state();
                    next.sort = sort;
                    next.direction = direction;
                    on_sort_change.push(next);
                },
            }
            Pager {
                pagination,
                page: page.page.unwrap_or_else(|| query_state().page).max(1),
                page_size: page.page_size,
                total: page.total,
                has_more: page.has_more,
                label: t!("page-audit-title"),
                on_page_change: move |next_page| {
                    let mut next = query_state();
                    next.page = next_page;
                    on_page_change.push(next);
                },
            }
            }
        }
    };
    rsx! {
        section { class: "admin-audit-page", aria_labelledby: "admin-audit-title",
            h1 { id: "admin-audit-title", {t!("page-audit-title")} }
            AuditFilters { applied: applied_filter, on_submit: apply_filter }
            if requested_query.read().is_some() {
                {content}
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuditField {
    Actor,
    Entity,
    Action,
    RequestId,
    ActorType,
    Source,
    Result,
    From,
    To,
}

impl AuditField {
    const ALL: [Self; 9] = [
        Self::Actor,
        Self::Entity,
        Self::Action,
        Self::RequestId,
        Self::ActorType,
        Self::Source,
        Self::Result,
        Self::From,
        Self::To,
    ];

    fn names(self) -> (&'static str, &'static str) {
        match self {
            Self::Actor => ("actor", "audit-actor"),
            Self::Entity => ("entity", "audit-entity"),
            Self::Action => ("action", "audit-action"),
            Self::RequestId => ("request_id", "audit-request-id"),
            Self::ActorType => ("actor_type", "audit-actor-type"),
            Self::Source => ("source", "audit-source"),
            Self::Result => ("result", "audit-result"),
            Self::From => ("from", "audit-from"),
            Self::To => ("to", "audit-to"),
        }
    }

    fn label(self) -> String {
        match self {
            Self::Actor => t!("admin-audit-actor-id"),
            Self::Entity => t!("admin-audit-entity-identity"),
            Self::Action => t!("admin-audit-action"),
            Self::RequestId => t!("admin-audit-request-id"),
            Self::ActorType => t!("admin-audit-actor-type"),
            Self::Source => t!("admin-audit-source"),
            Self::Result => t!("admin-audit-result"),
            Self::From => t!("admin-audit-from"),
            Self::To => t!("admin-audit-to"),
        }
    }

    fn choices(self) -> Option<&'static [&'static str]> {
        match self {
            Self::ActorType => Some(&["public_user", "admin", "worker", "system"]),
            Self::Source => Some(&["public", "admin", "cli", "worker", "system"]),
            Self::Result => Some(&["success", "rejected", "failure"]),
            _ => None,
        }
    }

    fn choice_label(self, value: &str) -> String {
        match self {
            Self::ActorType => audit_actor_type_label(value),
            Self::Source => audit_source_label(value),
            Self::Result => audit_result_label(value),
            _ => value.to_owned(),
        }
    }

    fn normalize(self, value: &str) -> Option<String> {
        let maximum = match self {
            Self::Actor => 256,
            Self::Entity => 385,
            Self::Action | Self::RequestId => 128,
            Self::From | Self::To => {
                return AdminAuditFilter::absolute_time(normalize_filter_input(value)?);
            }
            _ => {
                let value = normalize_filter_input(value)?;
                return (value.is_empty() || self.choices()?.contains(&value))
                    .then(|| value.to_owned());
            }
        };
        normalize_filter_text(value, maximum).map(str::to_owned)
    }
}

/// 审计地址与表单的筛选状态，只有每个字段都有效时才构造 API 筛选。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct AuditFilterState([FilterValue; AuditField::ALL.len()]);

impl From<AdminAuditFilter> for AuditFilterState {
    fn from(mut filter: AdminAuditFilter) -> Self {
        filter.normalize_text();
        Self(
            [
                filter.actor,
                filter.entity,
                filter.action,
                filter.request_id,
                filter.actor_type,
                filter.source,
                filter.result,
                filter.occurred_from,
                filter.occurred_to,
            ]
            .map(FilterValue::Text),
        )
    }
}

impl AuditFilterState {
    pub(super) fn restore(pairs: &[(String, String)]) -> Self {
        Self(AuditField::ALL.map(|field| {
            FilterValue::restore(pairs, field.names().0, |value| field.normalize(value))
        }))
    }

    pub(super) fn append_pairs(&self, pairs: &mut Vec<(String, String)>) {
        for (field, value) in AuditField::ALL.into_iter().zip(&self.0) {
            value.append_pairs(pairs, field.names().0);
        }
    }

    pub(super) fn request(&self) -> Option<AdminAuditFilter> {
        self.prepare().ok()
    }

    fn prepare(&self) -> Result<AdminAuditFilter, AuditField> {
        let mut values = AuditField::ALL.map(|_| String::new());
        for (field, (value, normalized)) in
            AuditField::ALL.into_iter().zip(self.0.iter().zip(&mut values))
        {
            *normalized = value.text().and_then(|text| field.normalize(text)).ok_or(field)?;
        }
        let [
            actor,
            entity,
            action,
            request_id,
            actor_type,
            source,
            result,
            occurred_from,
            occurred_to,
        ] = values;
        Ok(AdminAuditFilter {
            actor,
            entity,
            action,
            request_id,
            actor_type,
            source,
            result,
            occurred_from,
            occurred_to,
        })
    }
}

#[component]
fn AuditFilters(
    applied: Memo<AuditFilterState>,
    on_submit: EventHandler<AdminAuditFilter>,
) -> Element {
    let (mut draft, mut submitted) = use_audit_filter_draft(applied);
    let (primary, advanced) = AuditField::ALL.split_at(AuditField::ActorType as usize);
    let advanced_active = draft.read().0[primary.len()..]
        .iter()
        .any(|value| value.text().is_none_or(|text| !text.is_empty()));
    rsx! {
        form {
            class: "admin-audit-filter",
            autocomplete: "off",
            onsubmit: move |event| {
                event.prevent_default();
                let next = draft.read().prepare();
                match next {
                    Ok(filter) => {
                        submitted.set(false);
                        draft.set(filter.clone().into());
                        on_submit.call(filter);
                    }
                    Err(field) => {
                        submitted.set(true);
                        focus_element_by_id_after_render(field.names().1);
                    }
                }
            },
            p { class: "admin-audit-filter__hint", {t!("admin-audit-filter-hint")} }
            div { class: "admin-audit-filter__primary",
                for field in primary.iter().copied() {
                    AuditFilterControl { key: "{field:?}", field, draft, show_errors: submitted() }
                }
            }
            details { class: "admin-audit-filter__advanced", open: advanced_active,
                summary { {t!("admin-audit-advanced-filters")} }
                div { class: "admin-audit-filter__advanced-grid",
                    for field in advanced.iter().copied() {
                        AuditFilterControl { key: "{field:?}", field, draft, show_errors: submitted() }
                    }
                }
            }
            div { class: "admin-audit-filter__actions",
                button { class: "admin-btn-primary", r#type: "submit", {t!("admin-audit-load")} }
                button {
                    r#type: "button",
                    onclick: move |_| {
                        draft.set(AuditFilterState::default());
                        submitted.set(false);
                        on_submit.call(AdminAuditFilter::default());
                    },
                    {t!("admin-audit-clear")}
                }
            }
        }
    }
}

#[component]
fn AuditFilterControl(
    field: AuditField,
    mut draft: Signal<AuditFilterState>,
    show_errors: bool,
) -> Element {
    let (name, id) = field.names();
    let slot = draft.read();
    let value = &slot.0[field as usize];
    let on_input = move |value| draft.write().0[field as usize] = FilterValue::Text(value);
    if let Some(options) = field.choices() {
        let choices = std::iter::once(("", t!("admin-audit-all"), false))
            .chain(options.iter().map(|&key| (key, field.choice_label(key), false)))
            .collect();
        return rsx! { QueryFilterSelect { id, name, label: field.label(), value: value.clone(), choices, on_change: on_input } };
    }
    let local_time = matches!(field, AuditField::From | AuditField::To);
    let input_type = if local_time { "datetime-local" } else { "search" };
    let input = value.input_value();
    let displayed = if local_time { audit_time_input(input) } else { input.to_owned() };
    let invalid_draft =
        show_errors && value.text().is_some_and(|text| field.normalize(text).is_none());
    rsx! {
        QueryFilterInput {
            id, name, label: field.label(), value: displayed,
            input_type,
            error_values: value.error_values(invalid_draft), on_input,
        }
    }
}

fn use_audit_filter_draft(
    applied: Memo<AuditFilterState>,
) -> (Signal<AuditFilterState>, Signal<bool>) {
    let mut draft = use_signal(move || applied.peek().clone());
    let mut submitted = use_signal(|| false);
    // 草稿只订阅已应用的筛选，排序、翻页和重读不能抹去未提交输入。
    use_effect(move || {
        let next = applied();
        if *draft.peek() != next {
            draft.set(next);
        }
        submitted.set(false);
    });
    (draft, submitted)
}

fn audit_time_input(value: &str) -> String {
    // 本地文本只作为输入框显示值，未编辑的时间边界仍按链接里的精确时刻提交。
    if Timestamp::parse_from_rfc3339(value).is_ok() {
        datetime_local_value(value).unwrap_or_default()
    } else {
        value.to_owned()
    }
}

#[component]
fn AuditTable(
    page: AdminAuditPage,
    /// 当前排序列（空串 = 服务端原始顺序，即发生时间倒序）与方向。
    sort: String,
    direction: SortDirection,
    on_sort_change: EventHandler<(String, SortDirection)>,
) -> Element {
    let empty = page.records.is_empty();
    let mut records = page.records;
    sort_audit_records(&mut records, &sort, direction);
    rsx! {
        MobileSortControl {
            columns: vec![
                ("at", t!("admin-audit-at")),
                ("actor", t!("admin-audit-actor")),
                ("action", t!("admin-audit-action")),
                ("entity", t!("admin-audit-entity")),
                ("source", t!("admin-audit-source")),
                ("result", t!("admin-audit-result")),
            ],
            sort: sort.clone(),
            direction,
            on_sort_change,
        }
        div { class: "admin-shell__table-wrap admin-audit-table-wrap",
            table { class: "admin-shell__table admin-audit-table",
                aria_label: t!("page-audit-title"),
                auction_web_shared::TableColumns { layout: "admin-audit", widths: vec![17, 12, 18, 15, 6, 6, 10, 16] }
                thead {
                    tr {
                        SortableTh {
                            column: "at",
                            label: t!("admin-audit-at"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-audit-table__time",
                        }
                        SortableTh {
                            column: "actor",
                            label: t!("admin-audit-actor"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                        SortableTh {
                            column: "action",
                            label: t!("admin-audit-action"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                        SortableTh {
                            column: "entity",
                            label: t!("admin-audit-entity"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                        SortableTh {
                            column: "source",
                            label: t!("admin-audit-source"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                        SortableTh {
                            column: "result",
                            label: t!("admin-audit-result"),
                            active: sort,
                            direction,
                            on_sort: on_sort_change,
                        }
                        th { scope: "col", {t!("admin-audit-request-id")} }
                        th { class: "admin-audit-table__metadata", scope: "col", {t!("admin-audit-metadata")} }
                    }
                }
                tbody {
                    for record in records {
                        AuditRow { record }
                    }
                }
            }
        }
        if empty {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("admin-audit-empty"),
                detail: "",
            }
        }
    }
}

#[component]
fn AuditRow(record: AdminAuditRecord) -> Element {
    let actor_type = audit_actor_type_label(&record.actor_type);
    let source = audit_source_label(&record.source);
    let result = audit_result_label(&record.result);
    let result_class = match record.result.as_str() {
        "success" => "admin-audit-result admin-audit-result--success",
        "rejected" => "admin-audit-result admin-audit-result--rejected",
        "failure" => "admin-audit-result admin-audit-result--failure",
        _ => "admin-audit-result admin-audit-result--neutral",
    };
    let request_id = record.request_id.unwrap_or_else(|| t!("value-empty"));
    let metadata = metadata_text(&record.metadata);
    let actor_identity = format!("{}({actor_type})", record.actor_id);
    let entity_identity = format!("{}({})", record.entity_id, record.entity_type);
    rsx! {
        tr { class: "admin-audit__row",
            td {
                class: "admin-audit-table__time",
                "data-label": t!("admin-audit-at"),
                Time { value: record.occurred_at.clone() }
            }
            td {
                "data-label": t!("admin-audit-actor"),
                TruncatedText {
                    value: actor_identity,
                    class_name: "admin-audit-table__value admin-audit-identity",
                }
            }
            td {
                "data-label": t!("admin-audit-action"),
                TruncatedText {
                    value: record.action,
                    class_name: "admin-audit-table__value",
                }
            }
            td {
                "data-label": t!("admin-audit-entity"),
                TruncatedText {
                    value: entity_identity,
                    class_name: "admin-audit-table__value admin-audit-identity admin-audit-code",
                }
            }
            td {
                "data-label": t!("admin-audit-source"),
                span { class: "admin-audit-source", "{source}" }
            }
            td {
                "data-label": t!("admin-audit-result"),
                span { class: result_class, "{result}" }
            }
            td {
                "data-label": t!("admin-audit-request-id"),
                TruncatedText {
                    value: request_id,
                    class_name: "admin-audit-table__value admin-audit-code",
                }
            }
            td {
                class: "admin-audit-table__metadata admin-audit-metadata",
                "data-label": t!("admin-audit-metadata"),
                TruncatedText {
                    value: metadata,
                    size: "wide",
                    class_name: "admin-audit-table__value admin-audit-code",
                }
            }
        }
    }
}

fn metadata_text(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned())
}

fn audit_actor_type_label(value: &str) -> String {
    match value {
        "public_user" => t!("admin-audit-actor-public-user"),
        "admin" => t!("admin-audit-actor-admin"),
        "worker" => t!("admin-audit-actor-worker"),
        "system" => t!("admin-audit-actor-system"),
        _ => value.to_owned(),
    }
}

fn audit_source_label(value: &str) -> String {
    match value {
        "public" => t!("admin-audit-source-public"),
        "admin" => t!("admin-audit-source-admin"),
        "cli" => t!("admin-audit-source-cli"),
        "worker" => t!("admin-audit-source-worker"),
        "system" => t!("admin-audit-source-system"),
        _ => value.to_owned(),
    }
}

fn audit_result_label(value: &str) -> String {
    match value {
        "success" => t!("admin-audit-result-success"),
        "rejected" => t!("admin-audit-result-rejected"),
        "failure" => t!("admin-audit-result-failure"),
        _ => value.to_owned(),
    }
}

/// 对当前页审计记录排序；`column` 为空或不在允许集合时保持服务端原始顺序（发生时间倒序）。
fn sort_audit_records(records: &mut [AdminAuditRecord], column: &str, direction: SortDirection) {
    sort_rows(records, column, direction, |left, right, column| match column {
        "at" => left.occurred_at.cmp(&right.occurred_at),
        "actor" => left.actor_id.cmp(&right.actor_id),
        "action" => left.action.cmp(&right.action),
        "entity" => left.entity_id.cmp(&right.entity_id),
        "source" => left.source.cmp(&right.source),
        "result" => left.result.cmp(&right.result),
        _ => Ordering::Equal,
    });
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use dioxus::dioxus_core::Mutation;
    use dioxus::prelude::*;

    use super::{AuditField, AuditFilterState, audit_time_input, page, use_audit_filter_draft};
    use crate::api::AdminAuditFilter;
    use crate::surface::query::AuditQuery;
    use crate::surface::realtime::{
        FilterValue, TestPage, get_parameter, selected_options, submit_forms, text_attributes,
    };

    type Control = Rc<Cell<Option<(Signal<AuditQuery>, Signal<AuditFilterState>)>>>;

    fn draft_harness() -> Element {
        let query = use_signal(|| audit_query("first"));
        let applied = use_memo(move || query.read().filter.clone());
        let (draft, _) = use_audit_filter_draft(applied);
        consume_context::<Control>().set(Some((query, draft)));
        rsx! { "{draft.read().0[AuditField::Actor as usize].input_value()}" }
    }

    fn audit_query(actor: &str) -> AuditQuery {
        AuditQuery {
            filter: AdminAuditFilter { actor: actor.to_owned(), ..AdminAuditFilter::default() }
                .into(),
            ..AuditQuery::default()
        }
    }

    #[test]
    fn audit_bookmark_selects_the_options_used_by_the_first_get() {
        let (page, edits) = TestPage::mount(page, "actor_type=worker&source=cli&result=failure");
        let gets = page.take_gets();
        assert_eq!(gets.len(), 1, "{gets:?}");
        assert!(gets[0].starts_with("/api/v1/admin/audit?"));
        for (key, value) in [("actor_type", "worker"), ("source", "cli"), ("result", "failure")] {
            assert_eq!(get_parameter(&gets[0], key).as_deref(), Some(value));
        }
        assert_eq!(selected_options(&edits), ["worker", "cli", "failure"]);
    }

    #[test]
    fn restored_audit_actor_shows_every_original_and_waits_for_submission() {
        for originals in [&["", "alice"][..], &["alice\n"][..]] {
            let (mut mounted, setup) = TestPage::mount(page, "actor=first");
            let form = submit_forms(&setup).next().unwrap();
            let initial = mounted.take_gets();
            assert_eq!(initial.len(), 1, "{initial:?}");
            assert_eq!(get_parameter(&initial[0], "actor").as_deref(), Some("first"));
            let invalid_address = originals
                .iter()
                .map(|value| ("actor".to_owned(), (*value).to_owned()))
                .collect::<Vec<_>>();
            let edits = mounted.restore(invalid_address.clone());
            mounted.assert_no_gets();
            let input = text_attributes(&edits, "aria-describedby")
                .find_map(|(id, value)| (value == "audit-actor-error").then_some(id))
                .unwrap();
            for original in originals {
                let escaped = format!("{original:?}");
                assert!(edits.iter().any(|edit| matches!(edit,
                    Mutation::CreateTextNode { value, .. } if value == &escaped
                )));
            }
            mounted.dispatch("submit", form, "");
            assert_eq!(mounted.address(), invalid_address);
            mounted.assert_no_gets();
            mounted.dispatch("input", input, "fixed");
            assert_eq!(mounted.address(), invalid_address);
            mounted.assert_no_gets();
            mounted.dispatch("submit", form, "");
            let gets = mounted.take_gets();
            assert_eq!(gets.len(), 1, "{gets:?}");
            assert_eq!(get_parameter(&gets[0], "actor").as_deref(), Some("fixed"));
        }
    }

    #[test]
    fn draft_tracks_history_filter_changes_and_preserves_edits_on_page_and_sort_changes() {
        let control = Control::default();
        let mut dom = VirtualDom::new(draft_harness).with_root_context(control.clone());
        dom.rebuild_in_place();
        dom.render_immediate_to_vec();
        let (mut location, mut draft) = control.get().unwrap();
        dom.in_runtime(|| {
            draft.write().0[AuditField::Actor as usize] = FilterValue::Text("unsaved".to_owned());
        });

        for query in [
            audit_query("first"),
            AuditQuery { sort: "actor".to_owned(), ..audit_query("first") },
            AuditQuery { page: 2, ..audit_query("first") },
        ] {
            dom.in_runtime(|| location.set(query));
            dom.render_immediate_to_vec();
            assert_eq!(draft.peek().0[AuditField::Actor as usize].input_value(), "unsaved");
        }
        for expected in ["second", "first", "second"] {
            dom.in_runtime(|| location.set(audit_query(expected)));
            dom.render_immediate_to_vec();
            assert_eq!(draft.peek().0[AuditField::Actor as usize].input_value(), expected);
        }
    }

    #[test]
    fn returning_from_a_sort_preserves_the_draft_after_applying_a_padded_actor() {
        let control = Control::default();
        let mut dom = VirtualDom::new(draft_harness).with_root_context(control.clone());
        dom.rebuild_in_place();
        dom.render_immediate_to_vec();
        let (mut location, mut draft) = control.get().unwrap();

        let submitted = location.peek().clone().with_filter(AdminAuditFilter {
            actor: " alice ".to_owned(),
            ..AdminAuditFilter::default()
        });
        let history = submitted.restored_from_history();
        dom.in_runtime(|| location.set(submitted));
        dom.render_immediate_to_vec();
        dom.in_runtime(|| {
            draft.write().0[AuditField::Actor as usize] = FilterValue::Text("unsaved".to_owned());
            location.write().sort = "actor".to_owned();
        });
        dom.render_immediate_to_vec();
        assert_eq!(draft.peek().0[AuditField::Actor as usize].input_value(), "unsaved");

        dom.in_runtime(|| location.set(history));
        dom.render_immediate_to_vec();
        assert_eq!(draft.peek().0[AuditField::Actor as usize].input_value(), "unsaved");
        assert_eq!(location.peek().page_request().unwrap().1.actor, "alice");
    }

    #[test]
    fn prefill_displays_browser_local_minutes_and_keeps_the_exact_time_on_submit() {
        // 宿主测试的本地时区按 UTC；WASM 由浏览器提供时区。
        let bound = "2026-11-18T09:05:37.500123+08:00";
        assert_eq!(audit_time_input(bound), "2026-11-18T01:05");
        assert_eq!(
            AdminAuditFilter::absolute_time(bound).as_deref(),
            Some("2026-11-18T01:05:37.500123Z")
        );
        assert_eq!(audit_time_input("2026-11-18T17:25"), "2026-11-18T17:25");
        assert_eq!(audit_time_input(""), "");
    }
}
