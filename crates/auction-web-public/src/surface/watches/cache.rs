//! 非敏感装备标记缓存；按稳定用户和拍卖 UUID 隔离，不保存凭据、装备链接或代理上限。

use auction_contracts::PublicLotWatchesResponse;
#[cfg(all(target_arch = "wasm32", not(test)))]
use auction_web_shared::storage;
use auction_web_shared::valid_uuid;
#[cfg(any(target_arch = "wasm32", test))]
use serde::{Deserialize, Serialize};

#[cfg(any(target_arch = "wasm32", test))]
use crate::api::PublicSessionCheckTrigger;

/// 条目键前缀；索引键使用不同分隔符，按前缀淘汰时不会命中索引本身。
#[cfg(any(target_arch = "wasm32", test))]
const KEY_PREFIX: &str = "hv.public.lot-watches.v1:";
#[cfg(any(target_arch = "wasm32", test))]
const INDEX_KEY: &str = "hv.public.lot-watches.v1#index";
#[cfg(any(target_arch = "wasm32", test))]
const CACHE_VERSION: u8 = 2;
/// 只保留最近访问的三场拍卖，避免长期使用后无限占用本地配额。
#[cfg(any(target_arch = "wasm32", test))]
const MAX_AUCTIONS: usize = 3;

/// 与 Solid 共用扁平条目，旧格式缺少版本和会话修订，按冷缓存处理。
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Deserialize, Serialize)]
struct CacheEntry<Page> {
    version: u8,
    session_revision: String,
    #[serde(flatten)]
    page: Page,
}

/// 缓存只能展示，不能提供修改命令的版本，也不能上传覆盖服务端。
pub(in crate::surface) fn load(
    user_id: &str,
    auction_id: &str,
) -> Option<PublicLotWatchesResponse> {
    if !valid_uuid(user_id) || !valid_uuid(auction_id) {
        return None;
    }

    cfg_select! {
        any(target_arch = "wasm32", test) => {
            let revision = PublicSessionCheckTrigger::watch_cache_revision()?;
            let entry: CacheEntry<PublicLotWatchesResponse> = storage::get_parsed(&key(user_id, auction_id))?;
            if entry.version != CACHE_VERSION
                || entry.session_revision != revision
                || PublicSessionCheckTrigger::watch_cache_revision().as_deref() != Some(revision.as_str())
            {
                return None;
            }
            let page = entry.page;
            #[cfg(target_arch = "wasm32")]
            let page =
                crate::api::valid_watch_collection(&page, user_id, auction_id).then_some(page)?;
            Some(page)
        }
        _ => {
            let _ = (user_id, auction_id);
            None
        }
    }
}

/// 存储禁用、配额不足等错误不改变服务器保存结果。淘汰只发生在本缓存自己的前缀内，
/// 语言、主题与每页数量偏好不会被挤掉；写入失败则不记录索引也不删除任何键。
pub(in crate::surface) fn save(page: &PublicLotWatchesResponse, revision: Option<&str>) {
    if !valid_uuid(&page.user_id) || !valid_uuid(&page.auction_id) {
        return;
    }

    #[cfg(any(target_arch = "wasm32", test))]
    {
        let Some(revision) = revision else { return };
        if PublicSessionCheckTrigger::watch_cache_revision().as_deref() != Some(revision) {
            return;
        }
        let entry =
            CacheEntry { version: CACHE_VERSION, session_revision: revision.to_owned(), page };
        let Some(value) = serde_json::to_string(&entry).ok() else { return };
        let current = key(&page.user_id, &page.auction_id);
        if !storage::set_item(&current, &value) {
            return;
        }
        if PublicSessionCheckTrigger::watch_cache_revision().as_deref() != Some(revision) {
            storage::remove_item(&current);
            return;
        }
        let mut recent = recent_keys();
        recent.retain(|entry| entry != &current);
        recent.insert(0, current.clone());
        recent.truncate(MAX_AUCTIONS);
        if !storage::set_item(INDEX_KEY, &recent.join("\n")) {
            storage::remove_item(&current);
            return;
        }
        for stale in storage::keys()
            .into_iter()
            .filter(|candidate| candidate.starts_with(KEY_PREFIX) && !recent.contains(candidate))
        {
            storage::remove_item(&stale);
        }
    }
    #[cfg(not(any(target_arch = "wasm32", test)))]
    let _ = (page, revision);
}

/// 已确认退出或换号后清除离开账号的快照。索引缺失时也不能再展示该账号的旧标记。
pub(in crate::surface) fn invalidate_lot_watches(user_id: &str) {
    if !valid_uuid(user_id) {
        return;
    }

    #[cfg(any(target_arch = "wasm32", test))]
    {
        let prefix = format!("{KEY_PREFIX}{user_id}:");
        for entry in storage::keys().into_iter().filter(|entry| entry.starts_with(&prefix)) {
            storage::remove_item(&entry);
        }
        let mut recent = recent_keys();
        let previous_count = recent.len();
        recent.retain(|entry| !entry.starts_with(&prefix));
        if recent.len() != previous_count {
            let _ = storage::set_item(INDEX_KEY, &recent.join("\n"));
        }
    }
    #[cfg(not(any(target_arch = "wasm32", test)))]
    let _ = user_id;
}

/// 读取最近保存的条目键；索引缺失、损坏或来自旧版本时按空索引处理。
#[cfg(any(target_arch = "wasm32", test))]
fn recent_keys() -> Vec<String> {
    storage::get_item(INDEX_KEY).map_or_default(|index| {
        let mut seen = std::collections::BTreeSet::new();
        index
            .split('\n')
            .filter(|key| valid_cache_key(key) && storage::get_item(key).is_some())
            .filter(|key| seen.insert(*key))
            .map(str::to_owned)
            .collect()
    })
}

#[cfg(any(target_arch = "wasm32", test))]
fn valid_cache_key(key: &str) -> bool {
    let Some(identities) = key.strip_prefix(KEY_PREFIX) else { return false };
    let Some((user_id, auction_id)) = identities.split_once(':') else { return false };
    !auction_id.contains(':') && valid_uuid(user_id) && valid_uuid(auction_id)
}

#[cfg(any(target_arch = "wasm32", test))]
fn key(user_id: &str, auction_id: &str) -> String {
    format!("{KEY_PREFIX}{user_id}:{auction_id}")
}

#[cfg(test)]
mod storage {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use serde::de::DeserializeOwned;

    thread_local! {
        static ITEMS: RefCell<BTreeMap<String, String>> = const { RefCell::new(BTreeMap::new()) };
    }

    pub(super) fn get_item(key: &str) -> Option<String> {
        ITEMS.with_borrow(|items| items.get(key).cloned())
    }

    pub(super) fn get_parsed<T: DeserializeOwned>(key: &str) -> Option<T> {
        serde_json::from_str(&get_item(key)?).ok()
    }

    pub(super) fn set_item(key: &str, value: &str) -> bool {
        ITEMS.with_borrow_mut(|items| items.insert(key.to_owned(), value.to_owned()));
        true
    }

    pub(super) fn keys() -> Vec<String> {
        ITEMS.with_borrow(|items| items.keys().cloned().collect())
    }

    pub(super) fn remove_item(key: &str) {
        ITEMS.with_borrow_mut(|items| items.remove(key));
    }
}

#[cfg(test)]
mod tests {
    use auction_contracts::{PublicLotWatchResponse, PublicLotWatchesResponse};
    use dioxus::prelude::*;
    use serde_json::{Value, json};

    use super::{key, load, save, storage};
    use crate::surface::commands::session_fixture;

    const USER: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000001";
    const AUCTION: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000005";
    const LOT: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000006";
    const REVISION_KEY: &str = "hv.public.session-revision";

    fn cache_context() -> Element {
        session_fixture::use_session_fixture(USER);
        rsx! {}
    }

    #[test]
    fn solid_written_fixture_round_trips_byte_for_byte() {
        let sample = include_str!("cache_entry.v2.fixture.json");
        let entry: Value = serde_json::from_str(sample).unwrap();
        let page = PublicLotWatchesResponse {
            user_id: "00000000-0000-4000-8000-000000000001".to_owned(),
            auction_id: "00000000-0000-4000-8000-000000000021".to_owned(),
            watches: vec![PublicLotWatchResponse {
                lot_id: "00000000-0000-4000-8000-000000000023".to_owned(),
                watched: true,
                channel: "mm".to_owned(),
                version: 1,
            }],
        };
        let (user, auction) = (page.user_id.as_str(), page.auction_id.as_str());
        let entry_key = key(user, auction);
        session_fixture::reset();
        let mut dom = VirtualDom::new(cache_context);
        dom.rebuild_in_place();
        dom.in_scope(ScopeId::APP, || {
            session_fixture::set_external_revision(REVISION_KEY, user);
            assert!(storage::set_item(&entry_key, sample));
            assert_eq!(load(user, auction), Some(page.clone()));
            storage::remove_item(&entry_key);
            save(&page, entry["session_revision"].as_str());
            assert_eq!(storage::get_item(&entry_key).as_deref(), Some(sample));

            let mut wrong_version = entry.clone();
            wrong_version["version"] = json!(1);
            let mut wrong_revision = entry.clone();
            wrong_revision["session_revision"] = json!("0");
            let mut missing_revision = entry;
            missing_revision.as_object_mut().unwrap().remove("session_revision");
            for invalid in [wrong_version, wrong_revision, missing_revision] {
                assert!(storage::set_item(&entry_key, &invalid.to_string()));
                assert_eq!(load(user, auction), None);
            }
        });
    }

    #[test]
    fn shared_cache_wire_format_rejects_an_old_version_or_a_delayed_write() {
        session_fixture::reset();
        let mut dom = VirtualDom::new(cache_context);
        dom.rebuild_in_place();
        dom.in_scope(ScopeId::APP, || {
            let page = PublicLotWatchesResponse {
                user_id: USER.to_owned(),
                auction_id: AUCTION.to_owned(),
                watches: vec![PublicLotWatchResponse {
                    lot_id: LOT.to_owned(), watched: true, channel: "mm".to_owned(), version: 3,
                }],
            };
            let entry_key = key(USER, AUCTION);
            let old_format = serde_json::to_string(&page).unwrap();
            assert!(storage::set_item(&entry_key, &old_format));
            assert_eq!(load(USER, AUCTION), None);

            let revision = "302240678275694148452353";
            session_fixture::set_external_revision(REVISION_KEY, "00000000-0000-4000-8000-000000000001");
            save(&page, Some(revision));
            let saved: Value = storage::get_parsed(&entry_key).unwrap();
            assert_eq!(saved, json!({
                "version": 2, "session_revision": revision, "user_id": USER, "auction_id": AUCTION,
                "watches": [{"lot_id": LOT, "watched": true, "channel": "mm", "version": 3}],
            }));
            assert_eq!(load(USER, AUCTION), Some(page.clone()));

            let mut wrong_version = saved.clone();
            wrong_version["version"] = json!(1);
            assert!(storage::set_item(&entry_key, &wrong_version.to_string()));
            assert_eq!(load(USER, AUCTION), None);

            session_fixture::set_external_revision(REVISION_KEY, "00000000-0000-4000-8000-000000000002");
            // 模拟另一页在写入前检查通过，但本页先完成换代。
            assert!(storage::set_item(&entry_key, &saved.to_string()));
            assert_eq!(load(USER, AUCTION), None);
            storage::remove_item(&entry_key);
            save(&page, Some(revision));
            assert!(storage::get_item(&entry_key).is_none());
            save(&page, Some("302240678275694148452354"));
            assert_eq!(load(USER, AUCTION), Some(page));
        });
    }
}
