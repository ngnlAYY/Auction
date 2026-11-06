//! 实时事件 wire 投影：统一页、合并结果、结构化错误，以及订阅 URL/序列校验。
//! 这里不建立连接、不做防重；只有 `subscription` 才知道 HTTP 与 WebSocket 的传输细节。

#[cfg(target_arch = "wasm32")]
use url::form_urlencoded;
#[cfg(target_arch = "wasm32")]
use uuid::Uuid;

pub(crate) use auction_contracts::PublicRealtimeEventResponse as RealtimeEvent;

/// 页面消费的统一实时页；wire 上的旧版单拍卖页和显式发现页在完成目标校验后才投影到这里。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RealtimePage {
    pub events: Vec<RealtimeEvent>,
    pub next_after: i64,
    pub has_more: bool,
    pub refetch_required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RealtimePageMerge {
    /// 拍卖级变化或恢复缺口要求重读全部权威快照。
    pub refresh_required: bool,
    /// 仅已通过防重、事实和版本校验的竞价通知对应资源失效。
    pub bids_changed: bool,
    pub continue_catch_up: bool,
    pub degraded: bool,
}

/// 实时订阅失败的结构化错误：页面层按 `kind` 选择重连策略，禁止对错误文案做字符串比较。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RealtimeError {
    kind: RealtimeErrorKind,
    /// 失败明细只用于诊断日志，不参与重连决策。
    detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RealtimeErrorKind {
    /// 重连无法自愈：游标或拍卖 ID 无效、认证/权限被拒绝、目标拍卖不存在。
    Permanent,
    /// 网络抖动、服务端瞬时故障、毒帧超限或握手超时等可重试失败。
    #[cfg(target_arch = "wasm32")]
    Transient,
}

impl RealtimeError {
    pub(super) fn permanent(detail: impl Into<String>) -> Self {
        Self { kind: RealtimeErrorKind::Permanent, detail: detail.into() }
    }

    #[cfg(target_arch = "wasm32")]
    pub(super) fn transient(detail: impl Into<String>) -> Self {
        Self { kind: RealtimeErrorKind::Transient, detail: detail.into() }
    }

    pub(crate) const fn is_permanent(&self) -> bool {
        matches!(self.kind, RealtimeErrorKind::Permanent)
    }

    pub(crate) fn detail(&self) -> &str {
        &self.detail
    }
}

#[cfg(target_arch = "wasm32")]
pub(super) fn websocket_url(auction_id: Option<&str>) -> Result<String, String> {
    let window = web_sys::window().ok_or_else(|| "浏览器窗口不可用".to_owned())?;
    let location = window.location();
    let protocol = location.protocol().map_err(|_| "读取浏览器协议失败".to_owned())?;
    let host = location.host().map_err(|_| "读取浏览器主机失败".to_owned())?;
    let websocket_protocol = match protocol.as_str() {
        "http:" => "ws:",
        "https:" => "wss:",
        _ => return Err("浏览器协议不支持 WebSocket".to_owned()),
    };
    let mut query = form_urlencoded::Serializer::new(String::new());
    if let Some(auction_id) = auction_id {
        query.append_pair("auction_id", auction_id);
    } else {
        query.append_pair("scope", "auctions");
    }
    Ok(format!("{websocket_protocol}//{host}/api/v1/ws?{}", query.finish()))
}

#[cfg(target_arch = "wasm32")]
pub(super) fn validate_realtime_page_sequence(
    page: &RealtimePage,
    after: i64,
) -> Result<(), String> {
    if page.next_after < after {
        return Err("实时游标发生回退".to_owned());
    }
    let mut previous = after;
    for event in &page.events {
        if event.stream_sequence <= previous || event.stream_sequence > page.next_after {
            return Err("实时事件页序列无效".to_owned());
        }
        previous = event.stream_sequence;
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub(super) fn canonical_auction_id(value: &str) -> Result<String, String> {
    let auction_id = Uuid::parse_str(value).map_err(|_| "拍卖 ID 无效".to_owned())?;
    let canonical = auction_id.to_string();
    if auction_id.is_nil() || canonical != value {
        return Err("拍卖 ID 无效".to_owned());
    }
    Ok(canonical)
}
