//! Public 目录的页内拍品详情：行入口、右侧抽屉、竞价历史与价格历史，以及目录之外的只读入口
//! 和站外的拍品直达链接。
//! 这里只保存展示快照与出价入口投影；资格判定归操作栏，命令负载、`command_id` 与未知结果
//! 仍由命令宿主唯一持有。关闭、遮罩点击与 Escape 只清理展示状态，不写 URL 或真实状态。

mod bid_history;
mod deep_link;
mod drawer;
mod entry;
mod lookup;
mod own_proxy;
mod price_history;

use dioxus::prelude::*;

use super::command_scope::CommandDialogInput;
use crate::api::CatalogRow;

pub(crate) use deep_link::{
    LotDeepLink, LotViewDisposition, LotViewRequest, LotViewTarget, dispose_lot_view,
};
pub(crate) use drawer::CatalogDetailHost;
pub(super) use entry::{CatalogViewButton, use_catalog_detail_entry};
pub(crate) use lookup::{
    LotDetailLookup, LotDetailLookupButton, LotDetailLookupHost, use_lot_detail_lookup,
};

/// 抽屉底部出价入口的投影。由行内操作栏按同一套资格判定写入，抽屉只照此渲染：
/// 目录行与抽屉因此不会对“能否出价”给出两种答案。
#[derive(Clone, PartialEq, Default)]
pub(super) enum CatalogDetailBid {
    /// 非竞价期、会话未确认、达到金额上限或行已卸载：不渲染入口（失败关闭）。
    #[default]
    Unavailable,
    /// 匿名访客且拍品可出价：渲染带安全 `return_to` 的登录链接。
    Login,
    /// 已登录：与行内出价按钮相同的命令弹窗输入与禁用判定。
    Command { input: Box<CommandDialogInput>, disabled: bool },
}

/// 操作栏写入出价入口投影的句柄。对目录模块之外不透明：其他页面的只读抽屉不接操作栏，
/// 也就看不到命令弹窗输入。
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct CatalogDetailBidSlot(pub(super) Signal<CatalogDetailBid>);

/// 抽屉展示的目录行快照与当前出价入口；同一时刻只展示一个拍品。
#[derive(Clone, PartialEq)]
pub(super) struct CatalogDetailInput {
    pub row: CatalogRow,
    pub bid: CatalogDetailBid,
    /// 该行是否提供标记入口；抽屉只在行内可标记时渲染同一个标记按钮。
    pub watchable: bool,
}

/// 抽屉内价格历史查询所用世界的来源。拍品响应不带世界，由打开抽屉的入口声明。
#[derive(Clone, PartialEq)]
pub(super) enum DetailWorld {
    /// 目录抽屉：目录属于外壳当前选中的世界。
    Selected,
    /// 入口已知拍品所在世界（出价记录页的世界筛选）。
    Known(String),
    /// 只知道拍卖 UUID（账户页）：打开价格历史时按拍卖读取其世界，不推测。
    Auction,
}

/// 页面级详情抽屉信号。信号由身份宿主持有（身份切换即清空），目录行是唯一写入方，
/// 抽屉宿主只读取并负责关闭。
#[derive(Clone, Copy)]
pub(super) struct CatalogDetailDialogContext(pub Signal<Option<CatalogDetailInput>>);
