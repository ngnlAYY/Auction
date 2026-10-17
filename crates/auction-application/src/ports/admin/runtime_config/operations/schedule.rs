//! 使用部署同一周时间语义校验候选排期；不访问数据库或建立轮次。

use chrono::Timelike;
use serde::{Deserialize, Serialize};

use super::{AdminRuntimeConfigError, invalid, require};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeScheduleConfig {
    /// 是否由管理页覆盖公共排期；默认关闭。
    pub managed: bool,
    /// 是否允许 Worker 自动创建后续周轮次。
    pub enabled: bool,
    pub timezone: String,
    pub preview_day: String,
    /// 预览的本地时间，格式 HH:MM。
    pub preview_time: String,
    pub open_day: String,
    /// 开拍的本地时间，格式 HH:MM。
    pub open_time: String,
    pub nominal_end_day: String,
    /// 计划结束的本地时间，格式 HH:MM。
    pub nominal_end_time: String,
    /// 排期 world 候选；运行时仍需与数据库权威启用状态取交集。
    pub worlds: Vec<String>,
}

impl Default for AdminRuntimeScheduleConfig {
    fn default() -> Self {
        Self {
            managed: false,
            enabled: false,
            timezone: "UTC".to_owned(),
            preview_day: "Thursday".to_owned(),
            preview_time: "22:00".to_owned(),
            open_day: "Friday".to_owned(),
            open_time: "00:00".to_owned(),
            nominal_end_day: "Saturday".to_owned(),
            nominal_end_time: "12:00".to_owned(),
            worlds: vec!["main".to_owned(), "isekai".to_owned()],
        }
    }
}

impl AdminRuntimeScheduleConfig {
    pub(super) fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        require(self.timezone.parse::<chrono_tz::Tz>().is_ok(), "schedule.timezone")?;
        require(
            !self.worlds.is_empty()
                && self.worlds.len() <= 2
                && self.worlds.iter().all(|world| matches!(world.as_str(), "main" | "isekai"))
                && (self.worlds.len() == 1 || self.worlds[0] != self.worlds[1]),
            "schedule.worlds",
        )?;
        let preview = week_minute(&self.preview_day, &self.preview_time)
            .ok_or_else(|| invalid("schedule.preview_time"))?;
        let open = week_minute(&self.open_day, &self.open_time)
            .ok_or_else(|| invalid("schedule.open_time"))?;
        let end = week_minute(&self.nominal_end_day, &self.nominal_end_time)
            .ok_or_else(|| invalid("schedule.nominal_end_time"))?;
        require(preview + 120 <= open && open < end, "schedule")
    }
}

fn week_minute(day: &str, time: &str) -> Option<u32> {
    let day = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]
        .iter()
        .position(|candidate| *candidate == day)?;
    if time.len() != 5 {
        return None;
    }
    let time = chrono::NaiveTime::parse_from_str(time, "%H:%M").ok()?;
    Some(u32::try_from(day).ok()? * 24 * 60 + time.hour() * 60 + time.minute())
}
