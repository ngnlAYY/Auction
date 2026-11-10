//! 首次部署的页面内草稿与字段元数据；保留原始数字输入，不持久化或格式化凭据。

/// 字段顺序同时决定渲染、错误摘要和首个错误焦点的顺序。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum SetupField {
    DatabaseHost,
    DatabasePort,
    DatabaseUsername,
    DatabasePassword,
    DatabaseName,
    AdminActor,
    AdminSecret,
    AdminConfirmation,
    PublicOrigin,
    AdminOrigin,
    PublicListenerPort,
    AdminListenerPort,
    ScheduleTimezone,
    SchedulePreviewDay,
    SchedulePreviewTime,
    ScheduleOpenDay,
    ScheduleOpenTime,
    ScheduleEndDay,
    ScheduleEndTime,
    StartPrice,
    LogFilter,
    WorkerConcurrency,
    WorkerPoll,
    RealtimePoll,
}

impl SetupField {
    pub(super) const DATABASE: [Self; 5] = [
        Self::DatabaseHost,
        Self::DatabasePort,
        Self::DatabaseUsername,
        Self::DatabasePassword,
        Self::DatabaseName,
    ];
    pub(super) const ADMIN: [Self; 3] =
        [Self::AdminActor, Self::AdminSecret, Self::AdminConfirmation];
    pub(super) const BASIC: [Self; 3] = [Self::PublicOrigin, Self::AdminOrigin, Self::StartPrice];
    pub(super) const LISTENER: [Self; 2] = [Self::PublicListenerPort, Self::AdminListenerPort];
    pub(super) const SCHEDULE: [Self; 7] = [
        Self::ScheduleTimezone,
        Self::SchedulePreviewDay,
        Self::SchedulePreviewTime,
        Self::ScheduleOpenDay,
        Self::ScheduleOpenTime,
        Self::ScheduleEndDay,
        Self::ScheduleEndTime,
    ];
    pub(super) const WEEKDAYS: [(&'static str, &'static str); 7] = [
        ("Monday", "setup-day-monday"),
        ("Tuesday", "setup-day-tuesday"),
        ("Wednesday", "setup-day-wednesday"),
        ("Thursday", "setup-day-thursday"),
        ("Friday", "setup-day-friday"),
        ("Saturday", "setup-day-saturday"),
        ("Sunday", "setup-day-sunday"),
    ];
    pub(super) const ADVANCED: [Self; 4] =
        [Self::LogFilter, Self::WorkerConcurrency, Self::WorkerPoll, Self::RealtimePoll];

    pub(super) fn all() -> impl Iterator<Item = Self> {
        Self::DATABASE
            .into_iter()
            .chain(Self::ADMIN)
            .chain(Self::BASIC)
            .chain(Self::LISTENER)
            .chain(Self::SCHEDULE)
            .chain(Self::ADVANCED)
    }

    /// 控件 ID 复用既有标签键，辅助文案固定使用 `-help` 后缀。
    pub(super) const fn id(self) -> &'static str {
        match self {
            Self::DatabaseHost => "setup-database-host",
            Self::DatabasePort => "setup-database-port",
            Self::DatabaseUsername => "setup-database-username",
            Self::DatabasePassword => "setup-database-password",
            Self::DatabaseName => "setup-database-name",
            Self::AdminActor => "setup-admin-actor",
            Self::AdminSecret => "setup-admin-secret",
            Self::AdminConfirmation => "setup-admin-confirmation",
            Self::PublicOrigin => "setup-public-origin",
            Self::AdminOrigin => "setup-admin-origin",
            Self::PublicListenerPort => "setup-public-listener-port",
            Self::AdminListenerPort => "setup-admin-listener-port",
            Self::ScheduleTimezone => "setup-schedule-timezone",
            Self::SchedulePreviewDay => "setup-schedule-preview-day",
            Self::SchedulePreviewTime => "setup-schedule-preview-time",
            Self::ScheduleOpenDay => "setup-schedule-open-day",
            Self::ScheduleOpenTime => "setup-schedule-open-time",
            Self::ScheduleEndDay => "setup-schedule-end-day",
            Self::ScheduleEndTime => "setup-schedule-end-time",
            Self::StartPrice => "setup-start-price",
            Self::LogFilter => "setup-log-filter",
            Self::WorkerConcurrency => "setup-worker-concurrency",
            Self::WorkerPoll => "setup-worker-poll",
            Self::RealtimePoll => "setup-realtime-poll",
        }
    }

    pub(super) const fn number_bounds(self) -> Option<(u64, u64, u64)> {
        match self {
            Self::DatabasePort | Self::PublicListenerPort | Self::AdminListenerPort => {
                Some((1, 65_535, 1))
            }
            Self::StartPrice => Some((1_000, 1_000_000_000, 1_000)),
            Self::WorkerConcurrency => Some((1, 8, 1)),
            Self::WorkerPoll => Some((11, 300, 1)),
            Self::RealtimePoll => Some((100, 5_000, 1)),
            _ => None,
        }
    }

    /// 与配置 owner 的 UTF-8 字节上限对齐；HTML maxlength 仅作宽松输入辅助。
    pub(super) const fn maximum_bytes(self) -> Option<usize> {
        match self {
            Self::DatabaseHost => Some(253),
            Self::DatabaseUsername | Self::DatabaseName | Self::AdminActor => Some(128),
            Self::DatabasePassword => Some(4096),
            Self::AdminSecret | Self::AdminConfirmation => Some(512),
            Self::LogFilter => Some(256),
            Self::ScheduleTimezone => Some(128),
            _ if self.is_weekday() => Some(9),
            _ if self.is_schedule_time() => Some(5),
            _ => None,
        }
    }

    pub(super) const fn input_type(self) -> &'static str {
        match self {
            Self::DatabasePassword | Self::AdminSecret | Self::AdminConfirmation => "password",
            Self::PublicOrigin | Self::AdminOrigin => "url",
            _ if self.is_schedule_time() => "time",
            _ if self.number_bounds().is_some() => "number",
            _ => "text",
        }
    }

    pub(super) const fn autocomplete(self) -> &'static str {
        match self {
            Self::AdminActor => "username",
            Self::DatabasePassword | Self::AdminSecret | Self::AdminConfirmation => "new-password",
            _ => "off",
        }
    }

    pub(super) const fn is_weekday(self) -> bool {
        matches!(self, Self::SchedulePreviewDay | Self::ScheduleOpenDay | Self::ScheduleEndDay)
    }

    pub(super) const fn is_schedule_time(self) -> bool {
        matches!(self, Self::SchedulePreviewTime | Self::ScheduleOpenTime | Self::ScheduleEndTime)
    }

    /// 联动字段修改时一并清除旧错误，避免密码、Origin、端口或排期顺序提示过期。
    pub(super) fn related(self, other: Self) -> bool {
        self == other
            || (matches!(self, Self::AdminSecret | Self::AdminConfirmation)
                && matches!(other, Self::AdminSecret | Self::AdminConfirmation))
            || (matches!(self, Self::PublicOrigin | Self::AdminOrigin)
                && matches!(other, Self::PublicOrigin | Self::AdminOrigin))
            || (Self::LISTENER.contains(&self) && Self::LISTENER.contains(&other))
            || (Self::SCHEDULE.contains(&self) && Self::SCHEDULE.contains(&other))
    }
}

/// 固定字段集的短生命周期草稿；不实现 Debug，避免敏感输入被意外记录。
#[derive(Clone, PartialEq)]
pub(super) struct SetupDraft {
    values: [String; 24],
    pub(super) metrics_enabled: bool,
    pub(super) schedule_enabled: bool,
}

impl Default for SetupDraft {
    fn default() -> Self {
        let mut draft = Self {
            values: std::array::from_fn(|_| String::new()),
            metrics_enabled: true,
            schedule_enabled: false,
        };
        for (field, value) in [
            (SetupField::DatabaseHost, "127.0.0.1"),
            (SetupField::DatabasePort, "5432"),
            (SetupField::DatabaseName, "auction"),
            (SetupField::PublicListenerPort, "8090"),
            (SetupField::AdminListenerPort, "8091"),
            (SetupField::StartPrice, "1000"),
            (SetupField::LogFilter, "info,auction=debug"),
            (SetupField::WorkerConcurrency, "4"),
            (SetupField::WorkerPoll, "15"),
            (SetupField::RealtimePoll, "250"),
        ] {
            draft.set(field, value.to_owned());
        }
        draft.generate_admin_secret();
        // 排期草稿与首次 wire 的兼容默认值一致，恢复进阶参数不触碰这些值。
        let schedule = auction_contracts::SetupScheduleRequest::default();
        for (field, value) in [
            (SetupField::ScheduleTimezone, schedule.timezone),
            (SetupField::SchedulePreviewDay, schedule.preview_day),
            (SetupField::SchedulePreviewTime, schedule.preview_time),
            (SetupField::ScheduleOpenDay, schedule.open_day),
            (SetupField::ScheduleOpenTime, schedule.open_time),
            (SetupField::ScheduleEndDay, schedule.nominal_end_day),
            (SetupField::ScheduleEndTime, schedule.nominal_end_time),
        ] {
            draft.set(field, value);
        }
        draft
    }
}

impl SetupDraft {
    pub(super) fn value(&self, field: SetupField) -> &str {
        &self.values[field as usize]
    }

    pub(super) fn set(&mut self, field: SetupField, value: String) {
        self.values[field as usize] = value;
    }

    /// 两个独立 v4 UUID 共含 244 位随机熵；使用既有系统/浏览器安全随机源，不降级。
    pub(super) fn generate_admin_secret(&mut self) {
        let secret = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        self.set(SetupField::AdminSecret, secret.clone());
        self.set(SetupField::AdminConfirmation, secret);
    }

    /// 只恢复性能与日志草稿；凭据、站点、监听端口和业务开关不受影响。
    pub(super) fn reset_advanced(&mut self) {
        let defaults = Self::default();
        for field in SetupField::ADVANCED {
            self.set(field, defaults.value(field).to_owned());
        }
    }

    /// 完成部署后清空所有密码，包括仅用于浏览器确认、不进入 DTO 的重复输入。
    pub(super) fn clear_passwords(&mut self) {
        for field in
            [SetupField::DatabasePassword, SetupField::AdminSecret, SetupField::AdminConfirmation]
        {
            self.values[field as usize].clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::surface::setup::draft as parent;

    #[test]
    fn setup_generates_and_replaces_random_admin_secret() {
        let mut draft = SetupDraft::default();
        let original = draft.value(SetupField::AdminSecret).to_owned();
        assert_eq!(original.len(), 64);
        assert!(original.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(draft.value(SetupField::AdminConfirmation), original);
        draft.reset_advanced();
        assert_eq!(draft.value(SetupField::AdminSecret), original);
        draft.generate_admin_secret();
        assert_ne!(draft.value(SetupField::AdminSecret), original);
        assert_eq!(
            draft.value(SetupField::AdminSecret),
            draft.value(SetupField::AdminConfirmation)
        );
        draft.clear_passwords();
        assert!(draft.value(SetupField::AdminSecret).is_empty());
        assert!(draft.value(SetupField::AdminConfirmation).is_empty());
    }
}
