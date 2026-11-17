//! 投递 outbox、载荷列、发送类别投影、Admin 列表的缺省与扩展投影、出站分类开关、玩家通知预检
//! 退役、资产冲突排除（含结算正文变量形状）、发送围栏收口、现金围栏对证据拒绝原因的分类、围栏结论
//! 经真实 Worker 运行时收口的贯通用例与提交后唤醒的 PostgreSQL 18 用例。

mod account_notices;
mod admin_projection;
mod category_projection;
mod disposition;
mod evidence_rejections;
mod fence;
mod mail_commit_wakeup;
mod notice_conflicts;
mod payload_columns;
mod recipient_renames;
mod send_classes;
mod settlement_shapes;
mod worker_fence;
