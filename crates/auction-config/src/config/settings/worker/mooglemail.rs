use std::path::PathBuf;

use auction_application::{AdminRuntimeMailTemplatesConfig, default_accepted_qualities};
use serde::Deserialize;

/// Worker 使用的 MoogleMail 入站轮询策略。
/// `equipment_filter` 与 `mail_templates` 等旧版 runtime 业务策略随 Worker 配置在启动时
/// 读取；它们只影响入站业务判断和通知内容，不包含 Cookie、密钥或其他敏感值。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MoogleMailWorkerConfig {
    /// 是否启用远程 MoogleMail 入站读取；默认关闭，避免配置不完整时发出外部请求。
    pub enabled: bool,
    /// 持久化邮箱标识；同一邮箱只能由一个有效租约并行处理。
    pub mailbox_key: String,
    /// 游戏世界，只允许 `main` 或 `isekai`。
    pub world_id: String,
    /// MoogleMail 未填写 `start` 时的起拍价覆盖值；省略时使用邮件专属默认值 50,000 Credits。
    pub start_price: Option<i64>,
    /// MoogleMail 拍卖装备过滤策略；品质硬性门槛始终生效，省略时使用默认 L/P。
    pub equipment_filter: MoogleMailEquipmentFilterConfig,
    /// MoogleMail 业务通知模板；与 Admin runtime 共用同一闭集与校验，省略时使用默认英文模板。
    pub mail_templates: AdminRuntimeMailTemplatesConfig,
    /// MoogleMail 异常原始响应证据的本地保存策略；不由 Admin runtime 接管。
    pub diagnostics: MoogleMailDiagnosticsConfig,
    /// 可选 `.keyenv` 路径；相对路径相对于主 Worker 配置文件解析。
    pub keyenv_file: Option<PathBuf>,
    /// MoogleMail 固定生产 origin 或显式回环联调 origin。
    pub endpoint: String,
    /// 单次 MoogleMail 响应最大字节数，不能超过 8 MiB。
    pub maximum_response_bytes: usize,
    pub minimum_request_interval_millis: u64,
    /// 各类 Send 开始的最小间距；不影响 Take/Return，与 HTTP 限速独立。
    pub send_intervals: auction_application::DeliverySendIntervals,
    /// 单次 MoogleMail HTTP 请求与响应正文读取的超时秒数。
    pub request_timeout_seconds: u64,
    pub batch_size: u16,
    pub maximum_pages: usize,
    /// 发送前基线策略；默认 shadow，切换需重启，不接入在线运行时开关。
    pub sent_baseline_mode: auction_application::SentBaselineMode,
    /// 每个 world 共用的标准五字段 cron；支持旧版描述符和时区前缀。
    pub schedule: String,
    /// 独立运行收件与维护并错峰两个世界；关闭时恢复每轮三分支并行的旧路径。
    pub poll_isolation_enabled: bool,
    /// 独立维护的 cron；省略时跟随 runtime 覆盖后实际生效的收件 schedule。
    /// 只在 poll_isolation_enabled 开启时生效，修改需要重启 Worker。
    pub maintenance_schedule: Option<String>,
    /// 完整 Read Mail 纯观察的最小间隔，默认 3600 秒，范围 60 至 86400。
    /// 每次维护结束后检查，实际间隔向上取整到维护节奏；旧路径忽略此值。
    pub read_mail_observation_interval_seconds: u64,
    /// 每个 cron 触发时刻后附加的随机抖动上限秒数。
    pub jitter_seconds: u64,
    pub lease_duration_seconds: u64,
    /// 邮箱 claim 续租周期秒数，必须小于租约。
    pub renew_interval_seconds: u64,
    pub fetch_attempts: u16,
    pub fetch_timeout_seconds: u64,
    pub process_attempts: u16,
    pub item_timeout_seconds: u64,
    /// 单次邮箱 claim 的总处理预算秒数；耗尽时只提交已完成前缀并推进到其邮件 ID。
    pub batch_timeout_seconds: u64,
    /// 内联与持久化指数退避的基础秒数。
    pub retry_base_seconds: u64,
    pub retry_maximum_seconds: u64,
    /// 发件人 allowlist；仅与详情完整名称比较，不匹配时保留待读项并失败关闭。
    /// 留空且未关闭 allowlist 时拒绝读取详情，等待配置修正。
    pub sender_allowlist: Vec<String>,
    /// 明确关闭发件人 allowlist 后才接受所有详情完整发件人。
    pub sender_allowlist_disabled: bool,
    /// 额外启用的 MoogleMail 世界；每个条目拥有独立邮箱游标和发件人策略。
    /// 顶层字段继续描述第一个世界，以保持现有单世界配置可用；新增世界通过
    /// `[[worker.mooglemail.additional_worlds]]` 声明。每个条目的存在即表示该世界启用。
    pub additional_worlds: Vec<MoogleMailWorldConfig>,
}

/// MoogleMail 异常响应原始数据的本地诊断保存策略。
/// 诊断归档只属于 PostgreSQL 完整档的 Worker 部署配置；原始正文由 Worker 使用父数据密钥
/// 派生的独立子密钥加密后保存，配置层不接触明文或密钥材料。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MoogleMailDiagnosticsConfig {
    /// 是否保存异常响应的原始诊断数据；默认启用。
    pub enabled: bool,
    /// 诊断归档目录；相对路径按主 Worker 配置文件所在目录解析。
    pub directory: PathBuf,
    /// 诊断归档保留天数；范围 1 至 365 天。
    pub retention_days: u16,
    /// 诊断归档总字节上限；范围 16 MiB 至 1 GiB。
    pub maximum_total_bytes: u64,
}

impl Default for MoogleMailDiagnosticsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: PathBuf::from("mail-diagnostics"),
            retention_days: 7,
            maximum_total_bytes: 256 * 1024 * 1024,
        }
    }
}

/// MoogleMail 拍卖装备过滤策略。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MoogleMailEquipmentFilterConfig {
    /// 是否启用前后缀、分类、类型和等级过滤；品质门槛始终启用。
    pub enabled: bool,
    /// 硬性允许品质；空数组回退默认 Legendary/Peerless。
    pub qualities: Vec<String>,
    pub prefixes: Vec<String>,
    pub suffixes: Vec<String>,
    pub categories: Vec<String>,
    pub equipment_types: Vec<String>,
    /// 最低装备等级；0 表示不启用等级下限。
    pub level_min: i32,
    /// 最高装备等级；0 表示不启用等级上限，最大允许 1000。
    pub level_max: i32,
}

impl Default for MoogleMailEquipmentFilterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            qualities: default_accepted_qualities(),
            prefixes: Vec::new(),
            suffixes: Vec::new(),
            categories: Vec::new(),
            equipment_types: Vec::new(),
            level_min: 0,
            level_max: 0,
        }
    }
}

/// 单个 MoogleMail 世界的入站轮询配置。
/// 世界配置不包含 Cookie 或其他敏感值。Worker 会为每个世界建立独立的持久化邮箱状态，
/// 但复用同一个已校验的外部 Cookie。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MoogleMailWorldConfig {
    pub world_id: String,
    /// 该世界对应的持久化邮箱标识；必须与其他世界不同。
    pub mailbox_key: String,
    pub sender_allowlist: Vec<String>,
    pub sender_allowlist_disabled: bool,
}

impl Default for MoogleMailWorldConfig {
    fn default() -> Self {
        Self {
            world_id: "isekai".to_owned(),
            mailbox_key: "mooglemail:isekai".to_owned(),
            sender_allowlist: Vec::new(),
            sender_allowlist_disabled: false,
        }
    }
}

impl Default for MoogleMailWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mailbox_key: "mooglemail:main".to_owned(),
            world_id: "main".to_owned(),
            start_price: None,
            equipment_filter: MoogleMailEquipmentFilterConfig::default(),
            mail_templates: AdminRuntimeMailTemplatesConfig::default(),
            diagnostics: MoogleMailDiagnosticsConfig::default(),
            keyenv_file: None,
            endpoint: "https://hentaiverse.org".to_owned(),
            maximum_response_bytes: 8 * 1024 * 1024,
            minimum_request_interval_millis: 1_000,
            send_intervals: auction_application::DeliverySendIntervals::default(),
            request_timeout_seconds: 15,
            batch_size: 100,
            maximum_pages: 20,
            sent_baseline_mode: auction_application::SentBaselineMode::Shadow,
            schedule: "*/5 * * * *".to_owned(),
            poll_isolation_enabled: true,
            maintenance_schedule: None,
            read_mail_observation_interval_seconds: 3600,
            jitter_seconds: 5,
            lease_duration_seconds: 60,
            renew_interval_seconds: 10,
            fetch_attempts: 3,
            fetch_timeout_seconds: 30,
            process_attempts: 3,
            item_timeout_seconds: 30,
            batch_timeout_seconds: 600,
            retry_base_seconds: 1,
            retry_maximum_seconds: 30,
            sender_allowlist: Vec::new(),
            sender_allowlist_disabled: false,
            additional_worlds: Vec::new(),
        }
    }
}

impl MoogleMailWorkerConfig {
    /// 借用已配置世界与邮箱标识，沿用 `main`、`isekai` 排序，不复制发件人名单。
    pub fn configured_mailboxes(&self) -> impl Iterator<Item = (&str, &str)> + Clone {
        self.ordered_world_settings().map(|(world_id, mailbox_key, _, _)| (world_id, mailbox_key))
    }

    /// 返回按 `main`、`isekai` 稳定排序的全部已配置世界。
    pub fn configured_worlds(&self) -> Vec<MoogleMailWorldConfig> {
        self.ordered_world_settings()
            .map(|(world_id, mailbox_key, sender_allowlist, sender_allowlist_disabled)| {
                MoogleMailWorldConfig {
                    world_id: world_id.to_owned(),
                    mailbox_key: mailbox_key.to_owned(),
                    sender_allowlist: sender_allowlist.to_owned(),
                    sender_allowlist_disabled,
                }
            })
            .collect()
    }

    /// 两种投影共用稳定分组：main 优先，其余保持原序；借用遍历不分配临时列表。
    fn ordered_world_settings(
        &self,
    ) -> impl Iterator<Item = (&str, &str, &[String], bool)> + Clone {
        let worlds = std::iter::once((
            self.world_id.as_str(),
            self.mailbox_key.as_str(),
            self.sender_allowlist.as_slice(),
            self.sender_allowlist_disabled,
        ))
        .chain(self.additional_worlds.iter().map(|world| {
            (
                world.world_id.as_str(),
                world.mailbox_key.as_str(),
                world.sender_allowlist.as_slice(),
                world.sender_allowlist_disabled,
            )
        }));
        worlds
            .clone()
            .filter(|(world_id, ..)| *world_id == "main")
            .chain(worlds.filter(|(world_id, ..)| *world_id != "main"))
    }
}
