//! Admin 结算报表筛选的 URL 状态与浏览器历史同步。

#[cfg(target_arch = "wasm32")]
use std::{cell::RefCell, rc::Rc};

use dioxus::prelude::*;
#[cfg(target_arch = "wasm32")]
use uuid::Uuid;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SettlementQuery {
    pub world_id: String,
    pub auction_id: String,
    pub sequence: Option<i64>,
    pub page: i64,
}

impl Default for SettlementQuery {
    fn default() -> Self {
        Self { world_id: "all".to_owned(), auction_id: String::new(), sequence: None, page: 1 }
    }
}

pub(super) fn read_settlement_query() -> SettlementQuery {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return SettlementQuery::default();
        };
        let search = window.location().search().unwrap_or_default();
        let raw = search.strip_prefix('?').unwrap_or_default();
        let mut query = SettlementQuery::default();
        for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
            match key.as_ref() {
                "world" if matches!(value.as_ref(), "main" | "isekai") => {
                    query.world_id = value.into_owned();
                }
                "auction" => {
                    if let Ok(id) = Uuid::parse_str(&value)
                        && !id.is_nil()
                        && id.to_string() == value
                    {
                        query.auction_id = value.into_owned();
                    }
                }
                "sequence" => {
                    if let Ok(sequence) = value.parse::<i64>()
                        && sequence > 0
                    {
                        query.sequence = Some(sequence);
                    }
                }
                "page" => {
                    if let Ok(page) = value.parse::<i64>()
                        && page > 0
                    {
                        query.page = page;
                    }
                }
                _ => {}
            }
        }
        // UUID 不能独立定位其所在目录页；canonical 地址始终把 auction 与 sequence 成对写入。
        if query.sequence.is_none() {
            query.auction_id.clear();
        }
        return query;
    }

    #[cfg(not(target_arch = "wasm32"))]
    SettlementQuery::default()
}

pub(super) fn replace_settlement_query(query: &SettlementQuery) {
    write_settlement_query(query, true);
}

/// 用户主动切换筛选或分页时创建可返回的浏览器历史项。
pub(super) fn push_settlement_query(query: &SettlementQuery) {
    write_settlement_query(query, false);
}

fn write_settlement_query(query: &SettlementQuery, replace: bool) {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(history) = window.history() else {
            return;
        };
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        if query.world_id != "all" {
            serializer.append_pair("world", &query.world_id);
        }
        if let Some(sequence) = query.sequence {
            serializer.append_pair("sequence", &sequence.to_string());
        }
        if !query.auction_id.is_empty() {
            serializer.append_pair("auction", &query.auction_id);
        }
        if query.page > 1 {
            serializer.append_pair("page", &query.page.to_string());
        }
        let encoded = serializer.finish();
        let pathname = window.location().pathname().unwrap_or_else(|_| "/settlement".to_owned());
        let hash = window.location().hash().unwrap_or_default();
        let url = if encoded.is_empty() {
            format!("{pathname}{hash}")
        } else {
            format!("{pathname}?{encoded}{hash}")
        };
        let state = history.state().unwrap_or(JsValue::NULL);
        if replace {
            let _ = history.replace_state_with_url(&state, "", Some(&url));
        } else {
            let _ = history.push_state_with_url(&state, "", Some(&url));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    let _ = (query, replace);
}

/// 监听同一路径的浏览器返回/前进，确保筛选状态不会与地址栏分离。
pub(super) fn use_settlement_location_revision() -> Signal<u64> {
    let revision = use_signal(|| 0_u64);
    #[cfg(target_arch = "wasm32")]
    {
        let guard = use_hook(|| Rc::new(RefCell::new(None::<SettlementPopstateGuard>)));
        let cleanup_guard = Rc::clone(&guard);
        use_drop(move || {
            if let Some(guard) = cleanup_guard.borrow_mut().take() {
                guard.cleanup();
            }
        });
        let effect_guard = Rc::clone(&guard);
        use_effect(move || {
            if effect_guard.borrow().is_some() {
                return;
            }
            let Some(window) = web_sys::window() else {
                return;
            };
            let mut revision = revision;
            let callback = Closure::<dyn FnMut()>::new(move || {
                revision.set(revision().saturating_add(1));
            });
            if window
                .add_event_listener_with_callback("popstate", callback.as_ref().unchecked_ref())
                .is_ok()
            {
                effect_guard.replace(Some(SettlementPopstateGuard { window, callback }));
            }
        });
    }
    revision
}

#[cfg(target_arch = "wasm32")]
struct SettlementPopstateGuard {
    window: web_sys::Window,
    callback: Closure<dyn FnMut()>,
}

#[cfg(target_arch = "wasm32")]
impl SettlementPopstateGuard {
    fn cleanup(self) {
        let _ = self.window.remove_event_listener_with_callback(
            "popstate",
            self.callback.as_ref().unchecked_ref(),
        );
    }
}
