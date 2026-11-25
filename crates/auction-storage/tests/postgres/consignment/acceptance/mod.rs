//! 寄售验收：第 5 期“旧在途退款”（旧义务在带寄售来源列的库上仍交还拍卖原案件，寄售主题不被拍卖 owner 接收），
//! 以及两世界订单从上架到卖家确领的端到端贯通（`two_worlds`）。

pub(in crate::consignment) mod chain;
mod consignment_subject;
mod legacy_refund;
mod two_worlds;
