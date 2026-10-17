//! 非敏感装备标记缓存；按稳定用户和拍卖 UUID 隔离，不保存凭据、装备链接或代理上限。

use auction_contracts::PublicLotWatchesResponse;

/// 缓存只能展示，不能提供修改命令的版本，也不能上传覆盖服务端。
pub(super) fn load(user_id: &str, auction_id: &str) -> Option<PublicLotWatchesResponse> {
    #[cfg(target_arch = "wasm32")]
    {
        let storage = web_sys::window()?.local_storage().ok()??;
        let value = storage.get_item(&key(user_id, auction_id)).ok()??;
        // 限制本地损坏或其他脚本写入的体积；拒绝而非部分解析。
        if value.len() > 2_000_000 {
            return None;
        }
        let page = serde_json::from_str(&value).ok()?;
        crate::api::valid_watch_collection(&page, user_id, auction_id).then_some(page)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (user_id, auction_id);
        None
    }
}

/// 存储禁用、配额不足等错误不改变服务器保存结果。
pub(super) fn save(page: &PublicLotWatchesResponse) {
    #[cfg(target_arch = "wasm32")]
    if let Some(storage) =
        web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        && let Ok(value) = serde_json::to_string(page)
    {
        let _ = storage.set_item(&key(&page.user_id, &page.auction_id), &value);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = page;
}

#[cfg(target_arch = "wasm32")]
fn key(user_id: &str, auction_id: &str) -> String {
    format!("hv.public.lot-watches.v1:{user_id}:{auction_id}")
}
