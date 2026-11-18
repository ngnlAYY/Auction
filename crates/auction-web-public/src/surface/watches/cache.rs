//! 非敏感装备标记缓存；按稳定用户和拍卖 UUID 隔离，不保存凭据、装备链接或代理上限。

use auction_contracts::PublicLotWatchesResponse;
#[cfg(all(target_arch = "wasm32", not(test)))]
use auction_web_shared::storage;

/// 条目键前缀；索引键使用不同分隔符，按前缀淘汰时不会命中索引本身。
#[cfg(any(target_arch = "wasm32", test))]
const KEY_PREFIX: &str = "hv.public.lot-watches.v1:";
#[cfg(any(target_arch = "wasm32", test))]
const INDEX_KEY: &str = "hv.public.lot-watches.v1#index";
/// 只保留最近访问的三场拍卖，避免长期使用后无限占用本地配额。
#[cfg(any(target_arch = "wasm32", test))]
const MAX_AUCTIONS: usize = 3;

/// 缓存只能展示，不能提供修改命令的版本，也不能上传覆盖服务端。
pub(in crate::surface) fn load(
    user_id: &str,
    auction_id: &str,
) -> Option<PublicLotWatchesResponse> {
    #[cfg(any(target_arch = "wasm32", test))]
    {
        let page: PublicLotWatchesResponse = storage::get_parsed(&key(user_id, auction_id))?;
        #[cfg(target_arch = "wasm32")]
        let page =
            crate::api::valid_watch_collection(&page, user_id, auction_id).then_some(page)?;
        Some(page)
    }
    #[cfg(not(any(target_arch = "wasm32", test)))]
    {
        let _ = (user_id, auction_id);
        None
    }
}

/// 存储禁用、配额不足等错误不改变服务器保存结果。淘汰只发生在本缓存自己的前缀内，
/// 语言、主题与每页数量偏好不会被挤掉；写入失败则不记录索引也不删除任何键。
pub(in crate::surface) fn save(page: &PublicLotWatchesResponse) {
    #[cfg(any(target_arch = "wasm32", test))]
    {
        let Some(value) = serde_json::to_string(page).ok() else { return };
        let current = key(&page.user_id, &page.auction_id);
        if !storage::set_item(&current, &value) {
            return;
        }
        let mut recent = recent_keys();
        recent.retain(|entry| entry != &current);
        recent.insert(0, current);
        recent.truncate(MAX_AUCTIONS);
        if !storage::set_item(INDEX_KEY, &recent.join("\n")) {
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
    let _ = page;
}

/// 已确认退出或换号后清除离开账号的快照。索引缺失时也不能再展示该账号的旧标记。
pub(in crate::surface) fn invalidate_lot_watches(user_id: &str) {
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
    storage::get_item(INDEX_KEY)
        .map(|index| {
            index.split('\n').filter(|key| key.starts_with(KEY_PREFIX)).map(str::to_owned).collect()
        })
        .unwrap_or_default()
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
