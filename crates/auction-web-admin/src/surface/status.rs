//! Admin 服务状态说明页。
//! 旧版此路由是静态产品说明，不读取 live/ready；运行诊断继续由 Dashboard 和运维端点拥有。

use auction_web_shared::t;
use dioxus::prelude::*;

/// 渲染旧版静态状态说明。
pub(super) fn page() -> Element {
    rsx! {
        section { aria_labelledby: "admin-status-title",
            h1 { id: "admin-status-title", {t!("page-status-title")} }
            p { {t!("page-status-description")} }
        }
    }
}
