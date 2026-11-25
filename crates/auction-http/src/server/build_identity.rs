//! 进程构建号：HTTP 响应头与 WebSocket 首帧使用同一份值。
//!
//! 构建号的取值与校验由 `auction-observability` 唯一拥有（发布构建注入 `AUCTION_BUILD_ID`）；浏览器构建号由 `auction-web-shared` 用同一规则
//! 展开。两者只用于发现部署后仍打开的旧页面，不参与鉴权、幂等或缓存决定。契约见
//! `docs/contracts/routes.md` 与 `docs/contracts/realtime.md`。

use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue};

/// 每个 HTTP 响应都携带的构建号响应头；浏览器用它比对自身构建号。
pub(crate) const BUILD_ID_HEADER: &str = "x-auction-build";

/// 与 `x-auction-build` 同时出现的“独立入口构建号”响应头：入口指针指向旧壳时，前者报告
/// 旧壳构建号让旧页面保持安静，后者报告独立入口（新版 Solid）的构建号供新页面比对。
pub(crate) const INDEPENDENT_BUILD_ID_HEADER: &str = "x-auction-build-independent";

/// 构建号规则由 `auction-observability` 唯一拥有；这里只为本 crate 的调用点转发。
pub(crate) fn is_valid(value: &str) -> bool {
    auction_observability::is_valid_build_id(value)
}

pub(crate) fn build_id() -> &'static str {
    auction_observability::process_build_id()
}

/// 一个 surface 对浏览器报告的两个构建号：默认入口所指 build 与独立入口所指 build。
///
/// 未配置挂载时只写历史的 `x-auction-build`，响应字节与此前一致；配置了挂载才同时写独立构建号
/// （指针构建与独立入口构建可能不同：旧页面比对 `build`，新页面比对 `independent`）。
/// 值已限定为 HTTP 头与 JSON 安全字符。
#[derive(Clone, Debug)]
pub(crate) struct BuildReport {
    build: Arc<str>,
    independent: Arc<str>,
    mounted: bool,
}

impl BuildReport {
    /// 进程自身构建号同时作为两个值：没有挂载配置时的报告。
    pub(crate) fn process() -> Self {
        let id: Arc<str> = Arc::from(build_id());
        Self { build: Arc::clone(&id), independent: id, mounted: false }
    }

    /// 挂载配置下的报告；任一值不合法返回 `None`，调用方失败关闭，不能把任意文本写进响应头或首帧。
    pub(crate) fn new(build: &str, independent: &str) -> Option<Self> {
        (is_valid(build) && is_valid(independent)).then(|| Self {
            build: Arc::from(build),
            independent: Arc::from(independent),
            mounted: true,
        })
    }

    /// 挂载配置下两个头同时出现；无挂载只写 `x-auction-build`。
    pub(crate) fn write_headers(&self, headers: &mut HeaderMap) {
        if let (Ok(build), Ok(independent)) =
            (HeaderValue::from_str(&self.build), HeaderValue::from_str(&self.independent))
        {
            headers.insert(BUILD_ID_HEADER, build);
            if self.mounted {
                headers.insert(INDEPENDENT_BUILD_ID_HEADER, independent);
            }
        }
    }
}

/// WebSocket 建连后的第一个服务端文本帧；浏览器据此在部署后提示刷新。
///
/// 只有客户端在连接 URL 上声明 `independent_build=1` 且两个构建号不同时才追加可选字段
/// `independent_build`；旧壳不声明，帧字节与历史一致。
pub(crate) fn build_hello_frame(report: &BuildReport, independent_declared: bool) -> String {
    if !independent_declared || report.build == report.independent {
        format!(r#"{{"type":"hello","build":"{}"}}"#, report.build)
    } else {
        format!(
            r#"{{"type":"hello","build":"{}","independent_build":"{}"}}"#,
            report.build, report.independent
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_id_is_always_a_safe_header_value() {
        let value = build_id();
        assert!(is_valid(value), "{value}");
        assert!(!value.contains('"'), "{value}");
    }

    #[test]
    fn hello_frame_carries_exactly_the_build_id() {
        assert_eq!(
            build_hello_frame(&BuildReport::process(), true),
            format!(r#"{{"type":"hello","build":"{0}"}}"#, build_id())
        );
    }

    #[test]
    fn hello_frame_adds_the_independent_build_only_when_declared_and_different() {
        let report = BuildReport::new("legacy-1", "solid-2").expect("合法构建号");
        assert_eq!(
            build_hello_frame(&report, true),
            r#"{"type":"hello","build":"legacy-1","independent_build":"solid-2"}"#
        );
        // 旧壳不声明能力：字节与历史一致。
        assert_eq!(build_hello_frame(&report, false), r#"{"type":"hello","build":"legacy-1"}"#);
        let same = BuildReport::new("solid-2", "solid-2").expect("合法构建号");
        assert_eq!(build_hello_frame(&same, true), r#"{"type":"hello","build":"solid-2"}"#);
        assert!(BuildReport::new("legacy-1", "bad value").is_none());
    }

    #[test]
    fn validation_matches_the_browser_closed_set() {
        assert!(is_valid("0.1.0"));
        assert!(is_valid("build_2026-11-04.abc"));
        assert!(!is_valid(""));
        assert!(!is_valid("two words"));
        assert!(!is_valid("a/b"));
        for dir_name in [".", "..", ".hidden", "-x", "_x"] {
            assert!(!is_valid(dir_name), "{dir_name}");
        }
        assert!(is_valid("a.b"));
        assert!(!is_valid(&"a".repeat(65)));
    }

    #[test]
    fn websocket_hello_contract_fixture() {
        use serde_json::{Value, json};

        let frame: Value = serde_json::from_str(&build_hello_frame(&BuildReport::process(), false))
            .expect("首帧 JSON");
        let mut expected = crate::server::contract_fixtures::wire_fixture("public/websocket-hello");
        assert_eq!(expected["build"], "fixture-build");
        // 期望构建号取本制品的发布值，实际帧保留生产编码的完整字段。
        expected["build"] = json!(build_id());
        assert_eq!(frame, expected);
    }
}
