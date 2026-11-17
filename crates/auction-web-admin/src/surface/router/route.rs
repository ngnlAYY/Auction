//! 管理端路由表：`Route` 枚举及其 `Routable` 派生；页面入口组件由上级 `router` 模块定义。
//! 本模块只放路由枚举；上级模块对 `mod route;` 的 `unreachable_code` 例外只覆盖这里的派生代码。

use dioxus::prelude::*;

use super::{
    AboutContent, Alerts, AuctionLots, AuctionOverview, Auctions, Audit, Config, Cookies,
    Credentials, Dashboard, DashboardAlias, Deliveries, DeliveryDetail, DeliveryOverview, Login,
    MailOverview, Mails, NotFound, Outbox, PlayerSupport, Renames, Rules, RulesContent, Settlement,
    SettlementReturnDetail, SettlementReturns, Setup, Status, Users,
};
use crate::surface::auth::AdminShell;

/// 管理端路由表。
/// 路径契约见 `docs/contracts/routes.md`。管理命令的实现必须走版本化 REST 契约，
/// 不在浏览器端重建状态机。
#[derive(Clone, Debug, PartialEq, Routable)]
#[rustfmt::skip]
// 路径字面量必须与 auction-http/src/assets/routes.rs 的 Admin 清单同步，未知路径不能获得 SPA 入口。
pub(in crate::surface) enum Route {
    #[route("/setup")]
    Setup {},

    #[layout(AdminShell)]
        #[route("/")]
        Dashboard {},

        #[route("/dashboard")]
        DashboardAlias {},

        #[route("/login")]
        Login {},

        #[route("/auctions")]
        Auctions {},

        #[route("/auctions/:auction_id/lots")]
        AuctionLots { auction_id: String },

        #[route("/auction")]
        AuctionOverview {},

        #[route("/mails")]
        Mails {},

        #[route("/mail")]
        MailOverview {},

        #[route("/cookies")]
        Cookies {},

        #[route("/credentials")]
        Credentials {},

        #[route("/users")]
        Users {},

        #[route("/renames")]
        Renames {},

        #[route("/deliveries")]
        Deliveries {},

        /// 单条投递的只读证据详情；玩家报出的投递 Reference 可直接定位。
        #[route("/deliveries/:delivery_id")]
        DeliveryDetail { delivery_id: String },

        #[route("/delivery")]
        DeliveryOverview {},

        /// 玩家支持：按用户或参考编号只读汇总，一次检索定位相关记录。
        #[route("/player-support")]
        PlayerSupport {},

        #[route("/outbox")]
        Outbox {},

        #[route("/settlement")]
        Settlement {},

        /// 结算退件：被拒收或过期退回的货款、退款与流拍/撤拍寄回，按案件处理。
        #[route("/settlement-returns")]
        SettlementReturns {},

        /// 单个退件案件：资产位置、处理动作与阻断原因、领回、代管、派发链与事件。
        #[route("/settlement-returns/:return_id")]
        SettlementReturnDetail { return_id: String },

        #[route("/config")]
        Config {},

        #[route("/rules-content")]
        RulesContent {},

        #[route("/rules")]
        Rules {},

        /// “关于”文档：公开端“关于”页面的多语言内容，与规则共用同一套编辑与发布流程。
        #[route("/about-content")]
        AboutContent {},

        /// 告警台：活动/历史告警、确认、静默与处置手册锚点。
        #[route("/alerts")]
        Alerts {},

        #[route("/audit")]
        Audit {},

        #[route("/status")]
        Status {},

        #[route("/:..segments")]
        NotFound { segments: Vec<String> },
}
