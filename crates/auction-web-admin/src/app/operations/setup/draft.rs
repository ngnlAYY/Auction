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
    pub(super) const ADVANCED: [Self; 4] =
        [Self::LogFilter, Self::WorkerConcurrency, Self::WorkerPoll, Self::RealtimePoll];

    pub(super) fn all() -> impl Iterator<Item = Self> {
        Self::DATABASE.into_iter().chain(Self::ADMIN).chain(Self::BASIC).chain(Self::ADVANCED)
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
            Self::StartPrice => "setup-start-price",
            Self::LogFilter => "setup-log-filter",
            Self::WorkerConcurrency => "setup-worker-concurrency",
            Self::WorkerPoll => "setup-worker-poll",
            Self::RealtimePoll => "setup-realtime-poll",
        }
    }

    pub(super) const fn number_bounds(self) -> Option<(u64, u64, u64)> {
        match self {
            Self::DatabasePort => Some((1, 65_535, 1)),
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
            _ => None,
        }
    }

    pub(super) const fn input_type(self) -> &'static str {
        match self {
            Self::DatabasePassword | Self::AdminSecret | Self::AdminConfirmation => "password",
            Self::PublicOrigin | Self::AdminOrigin => "url",
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

    /// 联动字段修改时一并清除旧错误，避免密码确认或 Origin 冲突提示过期。
    pub(super) fn related(self, other: Self) -> bool {
        self == other
            || (matches!(self, Self::AdminSecret | Self::AdminConfirmation)
                && matches!(other, Self::AdminSecret | Self::AdminConfirmation))
            || (matches!(self, Self::PublicOrigin | Self::AdminOrigin)
                && matches!(other, Self::PublicOrigin | Self::AdminOrigin))
    }
}

/// 固定字段集的短生命周期草稿；不实现 Debug，避免敏感输入被意外记录。
#[derive(Clone, PartialEq)]
pub(super) struct SetupDraft {
    values: [String; 15],
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
            (SetupField::StartPrice, "1000"),
            (SetupField::LogFilter, "info,auction=debug"),
            (SetupField::WorkerConcurrency, "4"),
            (SetupField::WorkerPoll, "15"),
            (SetupField::RealtimePoll, "250"),
        ] {
            draft.set(field, value.to_owned());
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

    /// 完成部署后清空所有密码，包括仅用于浏览器确认、不进入 DTO 的重复输入。
    pub(super) fn clear_passwords(&mut self) {
        for field in
            [SetupField::DatabasePassword, SetupField::AdminSecret, SetupField::AdminConfirmation]
        {
            self.values[field as usize].clear();
        }
    }
}
