//! 排期纯规则：本地钟点与 IANA 时区解析、周场次、赛季早鸟窗口及赛季前场次。
//!
//! 本模块只做纯计算，不访问数据库、进程配置或外部服务；已冻结的轮次事实不在
//! 这里改写，调用方只把结论用于尚未建立的下一轮。

mod calendar;
mod early_bird;
mod source;

pub use calendar::{AuctionScheduleCalendar, AuctionScheduleSlot, AuctionWeeklySchedule};
pub use early_bird::{AuctionEarlyBirdCalendar, AuctionEarlyBirdConfig, AuctionEarlyBirdSeason};
pub use source::{AuctionEarlyBirdSlot, AuctionScheduleSource, AuctionScheduleTimes};
