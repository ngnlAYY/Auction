//! Admin dashboard section 映射的稳定出口。
//! 邮件、外部账号、realtime、运营水位、凭据聚合、告警和 unknown 降级分别归属相邻 owner；
//! 本文件只负责装配与稳定导出。

mod alerting;
mod credentials;
mod external_accounts;
mod fallback;
mod mail;
mod operations;
mod realtime;

pub(super) use alerting::map_alerting;
pub(super) use credentials::map_credentials;
pub(super) use external_accounts::map_external_accounts;
pub(super) use fallback::{unknown_section, unknown_section_with_code};
pub(super) use mail::{load_mail_status, map_mail_section};
pub(super) use operations::{map_delivery, map_fulfillment, map_reconciliation, map_recovery};
pub(super) use realtime::load_realtime_section;
