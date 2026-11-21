//! Admin lot 页面加载和错误状态。

use std::collections::HashSet;

use auction_web_shared::t;
use auction_web_shared::{AsyncState, AsyncStateKind};
use dioxus::prelude::*;

use super::super::admin_api_error_message;
use super::super::auctions::editor::use_draft_page;
use super::super::query::use_lots_query_state;
use super::super::router::Route;
use super::edit::{AdminLotEditDialog, LotEdit};
use super::presentation::lot_update_notice_label;
use super::state::LotUpdateNotice;
use crate::api::fetch_admin_lots;
use crate::surface::pagination::use_admin_pagination;
use crate::surface::pending::{PendingCommandPanel, advance_command_reload, use_pending_commands};

#[derive(Default)]
struct LotPageInvalidations {
    intake_entity: Option<String>,
    deferred_intake_entities: HashSet<String>,
}

impl LotPageInvalidations {
    fn should_reload(&mut self, entity: &str, auction_entity: &str) -> bool {
        if entity == auction_entity {
            return true;
        }
        if !entity.starts_with("intake:") {
            return false;
        }
        if let Some(confirmed) = self.intake_entity.as_deref() {
            return entity == confirmed;
        }
        // 世界由当前有效读取确认，先保留这次失效通知以便补读。
        self.deferred_intake_entities.insert(entity.to_owned());
        false
    }

    fn confirm_world(&mut self, world_id: &str) -> bool {
        let entity = format!("intake:{world_id}");
        let should_reload = self.deferred_intake_entities.contains(&entity);
        self.deferred_intake_entities.clear();
        self.intake_entity = Some(entity);
        should_reload
    }
}

/// 加载并展示一个拍卖的 Admin lot 安全投影。
pub fn page(auction_id: String) -> Element {
    // Router 会复用相同路由组件；迭代片段按拍卖身份重挂载资源与交互状态，历史跳转不能沿用旧场次。
    rsx! {
        for auction_id in [auction_id] {
            AdminAuctionLotsPage { key: "{auction_id}", auction_id }
        }
    }
}

#[component]
fn AdminAuctionLotsPage(auction_id: String) -> Element {
    // 分页与当前页排序跟地址栏双向同步（?page=&sort=&dir=），刷新与浏览器前进/后退都能恢复。
    let query = use_lots_query_state();
    let query_state = query.state;
    let requested_page = use_memo(move || query_state.read().page);
    let reload_generation = use_signal(|| 0_u64);
    let auction_entity = format!("auction:{auction_id}");
    let mut invalidations = use_signal(LotPageInvalidations::default);
    let pagination = use_admin_pagination("lots", &["page_size"], Some(100), 100_000);
    let page_size = pagination.size_signal();
    let draft = use_signal(|| None::<LotEdit>);
    let notice = use_signal(|| None::<LotUpdateNotice>);
    let edited_lot = use_memo(move || draft.read().as_ref().map(|edit| edit.lot_id().to_owned()))();
    let editing = edited_lot.is_some();
    let (read, lots) = use_draft_page(
        pagination.clone(),
        reload_generation,
        editing,
        move || (auction_id.clone(), requested_page()),
        move |(auction_id, page), size| {
            let generation = *reload_generation.peek();
            async move {
                let result = fetch_admin_lots(auction_id, page, size).await?;
                if query_state.peek().page == page
                    && *page_size.peek() == size
                    && *reload_generation.peek() == generation
                    && invalidations.write().confirm_world(&result.auction.world_id)
                {
                    // 交付快照前推进世代，确保录入前的快照不能用于编辑。
                    advance_command_reload(reload_generation);
                }
                Ok(result)
            }
        },
    );
    let visible_lots = use_memo(use_reactive((&lots,), |(lots,)| lots.and_then(Result::ok)));
    let pending = use_pending_commands(reload_generation, move |entity| {
        if let Some(lot_id) = entity.strip_prefix("lot:") {
            visible_lots
                .peek()
                .as_ref()
                .is_none_or(|page| page.lots.iter().any(|lot| lot.id == lot_id))
        } else {
            let mut invalidations = invalidations;
            invalidations
                .try_write()
                .is_ok_and(|mut state| state.should_reload(entity, &auction_entity))
        }
    });
    let latest_version = {
        let page = lots.as_ref().and_then(|result| result.as_ref().ok());
        edited_lot.as_ref().and_then(|lot_id| {
            page.and_then(|page| page.lots.iter().find(|lot| &lot.id == lot_id))
                .map(|lot| lot.state_version)
        })
    };

    let content = match lots {
        None => rsx! {
            AdminLotLoading {}
        },
        Some(Err(error)) => rsx! {
            AdminLotLoadError {
                detail: admin_api_error_message(&error),
                reload_generation,
            }
        },
        Some(Ok(page)) => {
            let mut on_page_change = query.clone();
            let mut on_sort_change = query;
            rsx! {
                super::table::AdminLotTable {
                    page: page.as_ref().clone(),
                    pagination,
                    edits_ready: read.is_ready(),
                    sort: query_state().sort,
                    direction: query_state().direction,
                    on_page_change: move |next_page| {
                        let mut next = query_state();
                        next.page = next_page;
                        on_page_change.push(next);
                    },
                    on_sort_change: move |(sort, direction)| {
                        let mut next = query_state();
                        next.sort = sort;
                        next.direction = direction;
                        on_sort_change.push(next);
                    },
                    reload_generation,
                    draft,
                    notice,
                }
            }
        }
    };
    let pending_in_dialog = edited_lot.as_ref().is_some_and(|id| {
        let entity = format!("lot:{id}");
        pending.contains(&entity) && !pending.is_busy(&entity)
    });
    rsx! {
        if !pending_in_dialog {
            PendingCommandPanel { owner: pending }
        }
        div { role: "status",
            if let Some(receipt) = notice() {
                p { class: "action-feedback action-feedback--success", {lot_update_notice_label(receipt)} }
            }
        }
        {content}
        if editing {
            AdminLotEditDialog { draft, latest_version, read, reload_generation, notice }
        }
    }
}

#[component]
fn AdminLotLoading() -> Element {
    rsx! {
        section { class: "admin-auction-page", aria_labelledby: "auction-title",
            h1 { id: "auction-title", {t!("page-auction-lots-title", auction: "…")} }
            p { class: "admin-auction-intro",
                Link { to: Route::Auctions {}, {t!("admin-lots-back")} }
            }
            section {
                AsyncState {
                    kind: AsyncStateKind::Loading,
                    title: t!("admin-lots-loading"),
                    detail: "",
                }
            }
        }
    }
}

#[component]
fn AdminLotLoadError(detail: String, mut reload_generation: Signal<u64>) -> Element {
    rsx! {
        section { class: "admin-auction-page", aria_labelledby: "auction-title",
            h1 { id: "auction-title", {t!("page-auction-lots-title", auction: "…")} }
            p { class: "admin-auction-intro",
                Link { to: Route::Auctions {}, {t!("admin-lots-back")} }
            }
            section {
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: detail,
                    detail: "",
                    retry_label: t!("admin-lots-retry"),
                    on_retry: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use dioxus::dioxus_core::{AttributeValue, ElementId, Mutation};
    use dioxus::prelude::{
        WritableExt, dioxus_core, dioxus_elements, dioxus_signals, rsx, use_hook, use_signal,
    };
    use gloo_net::http::Method;

    use super::LotPageInvalidations;
    use crate::api::AdminApiFailure;
    use crate::surface::auctions::editor::tests::{
        assert_field, click_button, field_by_static_id, mount_writer, writer_form,
    };
    use crate::surface::intake::AdminIntakeForm;
    use crate::surface::pending::use_pending_commands;

    const AUCTION_ENTITY: &str = "auction:0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
    const OTHER_AUCTION_ENTITY: &str = "auction:0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c";

    #[test]
    fn lot_page_matches_only_the_current_auction_and_intake_world() {
        let mut invalidations = LotPageInvalidations::default();
        assert!(!invalidations.confirm_world("main"));
        assert!(invalidations.should_reload(AUCTION_ENTITY, AUCTION_ENTITY));
        assert!(!invalidations.should_reload(OTHER_AUCTION_ENTITY, AUCTION_ENTITY));
        assert!(invalidations.should_reload("intake:main", AUCTION_ENTITY));
        assert!(!invalidations.should_reload("intake:isekai", AUCTION_ENTITY));
    }

    #[test]
    fn first_lot_read_coalesces_only_matching_intake_into_one_reload() {
        let mut invalidations = LotPageInvalidations::default();
        for entity in ["intake:main", "intake:isekai", "intake:main"] {
            assert!(!invalidations.should_reload(entity, AUCTION_ENTITY));
        }
        assert!(invalidations.confirm_world("main"));
        assert!(!invalidations.confirm_world("main"));

        let mut foreign_only = LotPageInvalidations::default();
        assert!(!foreign_only.should_reload("intake:isekai", AUCTION_ENTITY));
        assert!(!foreign_only.confirm_world("main"));
    }

    fn locks_equipment_url(edits: &[Mutation], field: ElementId) -> bool {
        edits.iter().any(|edit| matches!(
            edit,
            Mutation::SetAttribute { id, name: "disabled", value: AttributeValue::Bool(true), .. }
                if *id == field
        ))
    }

    #[test]
    fn unknown_intake_locks_only_the_original_world_form() {
        let (mut page, edits) = mount_writer(|| {
            use_hook(|| {
                AdminApiFailure::stub_write_response(
                    "/api/v1/admin/worlds/main/auction-intake/items",
                    Method::POST,
                    503,
                    "",
                );
            });
            let generation = use_signal(|| 0);
            use_pending_commands(generation, |_| false);
            let mut world = use_signal(|| "main");
            rsx! {
                button { onclick: move |_| world.set("isekai"), {"切到异世界".to_owned()} }
                button { onclick: move |_| world.set("main"), {"回到原世界".to_owned()} }
                for world_id in [world()] {
                    AdminIntakeForm { key: "{world_id}", world_id: world_id.to_owned() }
                }
            }
        });
        const URL_FIELD: &str = "auction-intake-url";
        let url = field_by_static_id(&page, URL_FIELD);
        assert_field(&page, url, "", false);
        for (id, value) in [
            (URL_FIELD, "https://hentaiverse.org/equip/123/0123456789"),
            ("auction-intake-price", "1000"),
            ("auction-intake-player", "Player"),
        ] {
            let field = field_by_static_id(&page, id);
            page.dispatch("input", field, value);
        }
        let submitted = page.dispatch("submit", writer_form(&edits), "");
        assert!(locks_equipment_url(&submitted, url));
        click_button(&mut page, "切到异世界");
        assert_field(&page, field_by_static_id(&page, URL_FIELD), "", false);
        let returned = click_button(&mut page, "回到原世界");
        assert!(locks_equipment_url(&returned, field_by_static_id(&page, URL_FIELD)));
    }
}
