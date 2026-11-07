//! W14.3 玩家通知编排：扫描已结算事实与冻结赛季边界，冻结正文并幂等入队。
//!
//! 扫描本身不写业务事实；重复执行只会在同一稳定 command_id 上命中既有 delivery。
//! 失效与退役由 delivery 预检负责，不改变已冻结载荷或结算结果。

mod season_reminder;
mod seller_summary;

use std::sync::Arc;

use chrono::Duration;

use crate::errors::ApplicationError;
use crate::ports::core::Clock;
use crate::ports::mail::MailDeliveryEnqueuer;
use crate::ports::player_notifications::PlayerNotificationScanner;

pub use season_reminder::{
    MAX_SEASON_REMINDER_BODY_BYTES, MAX_SEASON_REMINDER_ITEMS, SEASON_REMINDER_SUBJECT,
    SeasonReminder, SeasonReminderItem, group_season_reminders, season_reminder_body,
    season_reminder_command, season_reminder_invalid_reason,
};
pub use seller_summary::{
    MAX_SELLER_SUMMARY_BODY_BYTES, MAX_SELLER_SUMMARY_LOTS, SELLER_SUMMARY_SUBJECT, SellerSummary,
    SellerSummaryLot, group_seller_summaries, seller_summary_body, seller_summary_command,
    seller_summary_invalid_reason,
};

/// 只扫描最近结算场次，不回填历史；换季提醒最多提前一周创建。
pub const SELLER_SUMMARY_LOOKBACK_SECONDS: i64 = 7 * 24 * 60 * 60;
pub const SEASON_REMINDER_LEAD_SECONDS: i64 = 7 * 24 * 60 * 60;
/// 每轮扫描的事实行上限；超出的部分下一轮继续。
pub const PLAYER_NOTIFICATION_SCAN_LIMIT: u32 = 512;

/// 一轮扫描的入队结果；只用于日志与指标，不参与幂等判断。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlayerNotificationRun {
    pub seller_summaries: usize,
    pub season_reminders: usize,
}

/// 玩家通知服务；端口由装配根注入，扫描与入队都不访问配置或环境。
pub struct PlayerNotificationService {
    scanner: Arc<dyn PlayerNotificationScanner>,
    delivery: Arc<dyn MailDeliveryEnqueuer>,
    clock: Arc<dyn Clock>,
}

impl PlayerNotificationService {
    /// 建立服务；`Clock` 只用于扫描窗口，不进入冻结正文。
    pub fn new(
        scanner: Arc<dyn PlayerNotificationScanner>,
        delivery: Arc<dyn MailDeliveryEnqueuer>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { scanner, delivery, clock }
    }

    /// 执行一轮扫描与幂等入队。单个候选正文超限时跳过，不截断结果。
    pub async fn run_once(&self) -> Result<PlayerNotificationRun, ApplicationError> {
        let now = self.clock.now();
        let mut run = PlayerNotificationRun::default();

        let settled_since = now - Duration::seconds(SELLER_SUMMARY_LOOKBACK_SECONDS);
        let facts = self
            .scanner
            .seller_summary_facts(settled_since, PLAYER_NOTIFICATION_SCAN_LIMIT)
            .await?;
        for summary in group_seller_summaries(facts)? {
            let Some(body) = seller_summary_body(&summary) else {
                continue;
            };
            let command = seller_summary_command(&summary, body)?;
            self.delivery.enqueue(command).await?;
            run.seller_summaries += 1;
        }

        let facts = self
            .scanner
            .season_reminder_facts(
                now,
                SEASON_REMINDER_LEAD_SECONDS,
                PLAYER_NOTIFICATION_SCAN_LIMIT,
            )
            .await?;
        for reminder in group_season_reminders(facts)? {
            if season_reminder_invalid_reason(now, reminder.season_boundary, reminder.items.len())
                .is_some()
            {
                continue;
            }
            let Some(body) = season_reminder_body(&reminder) else {
                continue;
            };
            let command = season_reminder_command(&reminder, body)?;
            self.delivery.enqueue(command).await?;
            run.season_reminders += 1;
        }
        Ok(run)
    }
}
