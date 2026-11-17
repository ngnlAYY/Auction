//! Public 路由表：`Route` 枚举及其 `Routable` 派生。
//! 本模块只放路由枚举；上级模块对 `mod route;` 的 `unreachable_code` 例外只覆盖这里的派生代码。

use dioxus::prelude::*;

use crate::surface::pages::History;
use crate::surface::shell::PublicShell;
use crate::surface::{
    About, Account, AuctionIndex, AuctionList, AuctionPage, BidLog, Blacklist, Compensation,
    CompensationPool, Forum, Fulfillment, Login, LotDetail, Me, NotFound, Rules, Status,
};

/// 公开端路由表。
/// 路径逐条登记已发布的公开端 route（保持 URL 兼容）与经评审的 Public 扩展；增删路径时同步
/// `docs/contracts/routes.md`。
#[derive(Clone, Debug, PartialEq, Routable)]
#[rustfmt::skip]
// 路径字面量必须与 auction-http/src/assets/routes.rs 的 Public 清单同步，未知路径不能获得 SPA 入口。
pub(crate) enum Route {
    #[layout(PublicShell)]
        #[route("/")]
        AuctionIndex {},

        #[route("/auction")]
        AuctionPage {},

        #[route("/auctions")]
        AuctionList {},

        #[route("/lots/:lot_code")]
        LotDetail { lot_code: String },

        #[route("/bidlog")]
        BidLog {},

        #[route("/fulfillment")]
        Fulfillment {},

        #[route("/history")]
        History {},

        #[route("/login?:return_to")]
        Login { return_to: Option<String> },

        #[route("/me")]
        Me {},

        #[route("/account")]
        Account {},

        #[route("/rules")]
        Rules {},

        #[route("/about")]
        About {},

        #[route("/blacklist")]
        Blacklist {},

        #[route("/compensation-pool")]
        CompensationPool {},

        #[route("/compensation")]
        Compensation {},

        #[route("/forum")]
        Forum {},

        #[route("/status")]
        Status {},
    #[end_layout]

    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}
