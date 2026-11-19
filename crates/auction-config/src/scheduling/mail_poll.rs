//! 候选与 Worker 装配共用的收件世界相位和首次轮询时刻。

use std::sync::Arc;

use auction_application::scheduling::RecurringSchedule;
use auction_time::{Duration, Timestamp};

use crate::{ConfigError, MoogleMailWorkerConfig, StandardCronSchedule};

/// 单个邮箱启动时冻结的相位和用于首次登记的隔离基线。
pub struct MailPollStartup<'a> {
    pub world_id: &'a str,
    pub phase: Duration,
    initial_phase: Duration,
}

impl MailPollStartup<'_> {
    /// 由调用方提供登记时钟；候选校验可用同一时刻验证各世界。
    pub fn first_due(
        &self,
        schedule: &dyn RecurringSchedule,
        now: Timestamp,
    ) -> Result<Timestamp, ConfigError> {
        // 轮询状态登记要求 Unix 纳秒可用。初始 due 仅供登记，在线切换不改写持久状态。
        next_after(schedule, now, self.initial_phase)
            .filter(|next| {
                now.timestamp_nanos_opt().is_some() && next.timestamp_nanos_opt().is_some()
            })
            .ok_or(ConfigError::InvalidMoogleMailConfig(
                "MoogleMail 首次轮询时刻必须在未来且可登记到持久状态",
            ))
    }
}

impl MoogleMailWorkerConfig {
    /// 解析最终生效的 Worker 收件日程。
    pub fn receive_schedule(&self) -> Result<StandardCronSchedule, ConfigError> {
        StandardCronSchedule::parse(&self.schedule).map_err(|_| {
            ConfigError::InvalidMoogleMailConfig(
                "schedule 必须是标准五字段 cron、受支持描述符或 @every duration",
            )
        })
    }

    /// 冻结已配置收件世界的相位；邮件 transport 关闭时无需登记。
    pub fn poll_startup(
        &self,
        schedule: &dyn RecurringSchedule,
        now: Timestamp,
    ) -> Result<Vec<MailPollStartup<'_>>, ConfigError> {
        if !self.enabled {
            return Ok(Vec::new());
        }
        // 双世界先冻结相位，方便在线切回隔离；单世界保持零相位。
        let isekai_phase = if self.additional_worlds.is_empty() {
            Duration::zero()
        } else {
            let first = next_after(schedule, now, Duration::zero()).ok_or(INVALID_PHASE)?;
            let second = next_after(schedule, first, Duration::zero()).ok_or(INVALID_PHASE)?;
            (second.signed_duration_since(first) / 2).min(Duration::seconds(150))
        };
        Ok(self
            .configured_mailboxes()
            .map(|(world_id, _)| {
                let phase = if world_id == "isekai" { isekai_phase } else { Duration::zero() };
                let initial_phase =
                    if self.poll_isolation_enabled { phase } else { Duration::zero() };
                MailPollStartup { world_id, phase, initial_phase }
            })
            .collect())
    }
}

const INVALID_PHASE: ConfigError =
    ConfigError::InvalidMoogleMailConfig("MoogleMail 日程无法计算两个未来触发时刻");

/// 使用启动时冻结的相位平移收件或维护日程。
pub struct PhasedMailSchedule {
    schedule: Arc<dyn RecurringSchedule>,
    phase: Duration,
}

impl PhasedMailSchedule {
    /// 创建保留原日程语义的平移日程。
    pub fn new(schedule: Arc<dyn RecurringSchedule>, phase: Duration) -> Self {
        Self { schedule, phase }
    }
}

impl RecurringSchedule for PhasedMailSchedule {
    fn next_after(&self, now: Timestamp) -> Option<Timestamp> {
        next_after(self.schedule.as_ref(), now, self.phase)
    }
}

fn next_after(
    schedule: &dyn RecurringSchedule,
    now: Timestamp,
    phase: Duration,
) -> Option<Timestamp> {
    // 先平移查询时刻再平移结果，保留尚未到来的偏移触发点；@every 的相位在两次平移中抵消。
    let next = schedule.next_after(now.checked_sub_signed(phase)?)?.checked_add_signed(phase)?;
    (next > now).then_some(next)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::scheduling::RecurringSchedule;
    use auction_time::{Duration, Timestamp};

    use super::PhasedMailSchedule;
    use crate::{MoogleMailWorkerConfig, MoogleMailWorldConfig};

    struct TwoOccurrences([Timestamp; 2]);

    impl RecurringSchedule for TwoOccurrences {
        fn next_after(&self, now: Timestamp) -> Option<Timestamp> {
            self.0.into_iter().find(|next| *next > now)
        }
    }

    #[test]
    fn two_world_startup_checks_the_final_phased_due_against_the_storage_limit() {
        let last = Timestamp::from_timestamp_nanos(i64::MAX).unwrap();
        let phase = Duration::seconds(150);
        let base = last.checked_sub_signed(phase).unwrap();
        let now = base.checked_sub_signed(Duration::seconds(10)).unwrap();
        let second = last.checked_add_signed(phase).unwrap();
        let schedule = Arc::new(TwoOccurrences([base, second]));
        let mut mail = MoogleMailWorkerConfig {
            enabled: true,
            additional_worlds: vec![MoogleMailWorldConfig {
                world_id: "isekai".into(),
                mailbox_key: "mooglemail:isekai".into(),
                sender_allowlist: Vec::new(),
                sender_allowlist_disabled: false,
            }],
            ..Default::default()
        };
        let startup = mail.poll_startup(schedule.as_ref(), now).unwrap();
        assert_eq!(startup[0].world_id, "main");
        assert_eq!(startup[0].phase, Duration::zero());
        assert_eq!(startup[0].first_due(schedule.as_ref(), now).unwrap(), base);
        assert_eq!(startup[1].world_id, "isekai");
        assert_eq!(startup[1].phase, phase);
        assert_eq!(startup[1].first_due(schedule.as_ref(), now).unwrap(), last);
        assert_eq!(PhasedMailSchedule::new(schedule, phase).next_after(now), Some(last));

        let following =
            |time: Timestamp| time.checked_add_signed(Duration::nanoseconds(1)).unwrap();
        let one_past_limit = TwoOccurrences([following(base), following(second)]);
        let startup = mail.poll_startup(&one_past_limit, now).unwrap();
        assert_eq!(startup[0].first_due(&one_past_limit, now).unwrap(), following(base));
        assert!(startup[1].first_due(&one_past_limit, now).is_err());

        mail.poll_isolation_enabled = false;
        let startup = mail.poll_startup(&one_past_limit, now).unwrap();
        assert_eq!(startup[1].phase, phase);
        assert_eq!(startup[1].first_due(&one_past_limit, now).unwrap(), following(base));
        mail.enabled = false;
        assert!(mail.poll_startup(&one_past_limit, now).unwrap().is_empty());
    }
}
