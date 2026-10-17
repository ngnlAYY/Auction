//! Public 竞价与代理命令的稳定出口。
//! 普通竞价和代理创建/降价分别归属相邻 owner；本文件只负责模块装配与路由导出。

mod bid;
mod proxy;

// 编译期约束浏览器公开常量与领域权威边界，避免只改前端或只改后端造成上限漂移。
const _: () = assert!(auction_contracts::MAX_PUBLIC_BID_AMOUNT == auction_domain::MAX_BID_AMOUNT);

pub(crate) use bid::place_bid;
pub(crate) use proxy::{create_proxy_authorization, lower_proxy_authorization};
