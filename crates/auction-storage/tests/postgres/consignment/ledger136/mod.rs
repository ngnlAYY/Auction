//! 000136 的 PG18 测试：购买状态机、确领证明释放占用、COD 分账守恒、现金退件阻断、唯一键、拒收与新购阻断、
//! 运行账号授权与 restore 重放、135 到 136 的升级不变量。

mod acl;
pub(crate) mod fixture;
pub(crate) mod funding;
mod listing_event;
pub(in crate::consignment) mod purchase;
mod refusal;
mod release;
pub(super) mod returns;
mod upgrade;
