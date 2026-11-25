//! 寄售运维命令。

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub(crate) enum ConsignmentCommand {
    /// 按当前订单事实补刷全部寄售合同的买卖双方权利状态；一次性的上线补刷。
    ///
    /// 只追加派生的权利状态版本，与续办共用权利锁；已与事实一致的权利不追加，重复执行返回 0。
    /// 遍历超出订单分页上限时输出已处理部分并以非零退出。
    RefreshRights,
}
