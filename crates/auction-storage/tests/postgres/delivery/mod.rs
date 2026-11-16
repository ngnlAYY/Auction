//! 投递 outbox、载荷列、发送类别投影、出站分类开关、玩家通知预检退役、资产冲突排除（含结算
//! 正文变量形状）、发送围栏收口与提交后唤醒的 PostgreSQL 18 用例。

mod account_notices;
mod category_projection;
mod disposition;
mod fence;
mod mail_commit_wakeup;
mod notice_conflicts;
mod payload_columns;
mod recipient_renames;
mod send_classes;
mod settlement_shapes;
