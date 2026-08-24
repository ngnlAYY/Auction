//! Worker、论坛和 MoogleMail 配置模型。

use super::*;

/// Auction 主循环及其它兼容轮询 owner 的共享调度边界。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkerConfig {
    /// Worker control Unix socket 配置；默认关闭，避免无权限环境启动失败。
    pub control: WorkerControlConfig,
    /// Auction 主循环并发数；省略时 PostgreSQL 为 8、SQLite Lite 为 1。
    pub concurrency: Option<u16>,
    /// Auction 主循环及兼容 owner 的轮询间隔秒数；省略时为 60 秒。
    pub poll_interval_seconds: Option<u64>,
    /// Auction 生命周期和结算各自的单轮批次上限；范围 1-1000，默认 100。
    pub batch_limit: u16,
    /// 每轮完成后附加的随机抖动上限秒数；范围 0-86400，默认 5。
    pub jitter_seconds: u64,
    /// Auction 单个生命周期或结算任务的总处理超时秒数；默认 30。
    pub item_timeout_seconds: u64,
    /// Recovery 各阶段的持久化重试间隔秒数；省略时为 60 秒，最大不超过 86400 秒。
    pub recovery_retry_delay_seconds: u64,
    /// Delivery loop 的独立批次、lease 和超时边界。
    pub delivery: DeliveryWorkerConfig,
    /// Fulfillment loop 的独立批次、并发、lease 和超时边界。
    pub fulfillment: FulfillmentWorkerConfig,
    /// Recovery loop 的独立批次、并发、lease 和超时边界。
    pub recovery: RecoveryWorkerConfig,
    /// 可选 MoogleMail 轮询配置；省略时关闭远程读取。
    pub mooglemail: MoogleMailWorkerConfig,
    /// 可选 E-Hentai 论坛主题同步配置；省略时关闭远程写入。
    pub forum: ForumSyncWorkerConfig,
    /// 可选 operations alert 运维告警配置；省略时关闭告警扫描与 MoogleMail 通知。
    pub operations_alert: OperationsAlertWorkerConfig,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            control: WorkerControlConfig::default(),
            concurrency: None,
            poll_interval_seconds: None,
            batch_limit: 100,
            jitter_seconds: 5,
            item_timeout_seconds: 30,
            recovery_retry_delay_seconds: 60,
            delivery: DeliveryWorkerConfig::default(),
            fulfillment: FulfillmentWorkerConfig::default(),
            recovery: RecoveryWorkerConfig::default(),
            mooglemail: MoogleMailWorkerConfig::default(),
            forum: ForumSyncWorkerConfig::default(),
            operations_alert: OperationsAlertWorkerConfig::default(),
        }
    }
}

/// Delivery outbox 的独立领取、续租与外部调用边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeliveryWorkerConfig {
    /// 单轮最多领取的 delivery 数量；范围 1-1000，默认 100。
    pub batch_limit: u16,
    /// 单轮 delivery 并发数；范围 1-batch_limit，默认 8。
    pub concurrency: u16,
    /// Delivery 独立轮询间隔秒数；范围 1-86400，默认 60。
    pub poll_interval_seconds: u64,
    /// 每轮完成后附加的随机抖动上限秒数；范围 0-86400，默认 5。
    pub jitter_seconds: u64,
    /// claim lease 秒数；范围 1-86400，默认 60。
    pub lease_duration_seconds: u64,
    /// sending 期间续租间隔秒数；必须小于 lease，默认 10。
    pub renew_interval_seconds: u64,
    /// lease 过期后等待外部结果写回的宽限秒数；必须大于请求超时，默认 120。
    pub sending_grace_seconds: u64,
    /// 单次完整 DeliveryTransport 调用总超时秒数；默认 30。
    pub request_timeout_seconds: u64,
    /// 成功、失败或人工对账结果写回的超时秒数；默认 5。
    pub finalize_timeout_seconds: u64,
}

impl Default for DeliveryWorkerConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval_seconds: 60,
            jitter_seconds: 5,
            lease_duration_seconds: 60,
            renew_interval_seconds: 10,
            sending_grace_seconds: 120,
            request_timeout_seconds: 30,
            finalize_timeout_seconds: 5,
        }
    }
}

/// Settlement fulfillment Worker 的独立领取与处理边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FulfillmentWorkerConfig {
    /// 单轮最多处理的 fulfillment 数量；范围 1-1000，默认 100。
    pub batch_limit: u16,
    /// fulfillment action wave 的并发数；范围 1-batch_limit，默认 8。
    pub concurrency: u16,
    /// 两轮处理之间的固定轮询秒数；范围 1-86400，默认 60。
    pub poll_interval_seconds: u64,
    /// 每轮完成后附加的随机抖动上限秒数；范围 0-86400，默认 5。
    pub jitter_seconds: u64,
    /// fulfillment claim lease 秒数；范围 1-86400，默认 60。
    pub lease_duration_seconds: u64,
    /// 单个 fulfillment 的总处理超时秒数；必须小于 lease，默认 30。
    pub item_timeout_seconds: u64,
    /// 依赖暂不可用后的持久化重试间隔秒数；范围 1-86400，默认 60。
    pub retry_delay_seconds: u64,
}

impl Default for FulfillmentWorkerConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval_seconds: 60,
            jitter_seconds: 5,
            lease_duration_seconds: 60,
            item_timeout_seconds: 30,
            retry_delay_seconds: 60,
        }
    }
}

/// Recovery Worker 的独立调度和批次边界。
///
/// 旧版 recovery loop 使用独立的 `batch_limit` 和 `concurrency`，不能复用拍卖主循环的
/// 全局并发，否则一个高峰期会同时放大退回、action、重列和退款处理量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecoveryWorkerConfig {
    /// 每个 recovery phase 单轮最多领取的记录数；范围 1-1000，默认 100。
    pub batch_limit: u16,
    /// recovery action wave 的并发数；范围 1-batch_limit，默认 8。
    pub concurrency: u16,
    /// 两轮五阶段处理之间的固定轮询秒数；范围 1-86400，默认 60。
    pub poll_interval_seconds: u64,
    /// 每轮完成后附加的随机抖动上限秒数；范围 0-86400，默认 5。
    pub jitter_seconds: u64,
    /// recovery action claim lease 秒数；范围 1-86400，默认 60。
    pub lease_duration_seconds: u64,
    /// 单个 recovery 记录或 action 的总处理超时秒数；必须小于 lease，默认 30。
    pub item_timeout_seconds: u64,
    /// 余额低水位阈值；低于该值时写入 `low`，默认 100 credits。
    pub low_balance_threshold: i64,
    /// recovery health 两次远端余额读取之间的最小间隔秒数；默认 60 秒。
    pub health_refresh_interval_seconds: u64,
    /// 外部余额证据超过该年龄后标记为 stale；必须不小于刷新间隔，默认 300 秒。
    pub evidence_stale_after_seconds: u64,
}

impl Default for RecoveryWorkerConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval_seconds: 60,
            jitter_seconds: 5,
            lease_duration_seconds: 60,
            item_timeout_seconds: 30,
            low_balance_threshold: 100,
            health_refresh_interval_seconds: 60,
            evidence_stale_after_seconds: 300,
        }
    }
}

/// Worker 使用的 operations alert 扫描、阈值和通知批次配置。
///
/// 所有字段都有默认值，因此 TOML 可以只覆盖需要调整的字段；启用告警时必须显式提供
/// 合法的 `recipient`。通知正文只由应用层从健康摘要生成，不接受配置注入。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OperationsAlertWorkerConfig {
    /// 是否启用运维告警扫描和 MoogleMail 通知；省略时关闭。
    pub enabled: bool,
    /// scan、dispatch 与 reconcile 各自的轮询间隔秒数；范围 1-86400，省略时为 60 秒。
    pub scan_interval_seconds: u64,
    /// 读取四类健康来源的超时秒数，范围 1-30；省略时为 5 秒。
    pub source_timeout_seconds: u64,
    /// 同一轮观察结果合并到通知批次前的窗口秒数，范围 1-3600；省略时为 120 秒。
    pub batch_window_seconds: u64,
    /// 同一告警 signal 的通知冷却秒数，不能小于批次窗口；省略时为 1800 秒。
    pub cooldown_seconds: u64,
    /// 告警批次 claim 租约秒数，必须覆盖来源读取超时；省略时为 120 秒。
    pub claim_lease_seconds: u64,
    /// 告警投递失败后的持久化重试间隔秒数；省略时为 300 秒。
    pub retry_delay_seconds: u64,
    /// 单轮最多处理的 signal 数量，范围 1-64；省略时为 64。
    pub max_signals_per_scan: usize,
    /// 单个 MoogleMail 通知批次最多包含的 signal 数量，范围 1-64；省略时为 20。
    pub max_batch_items: usize,
    /// 单个 MoogleMail 通知正文的最大字节数，范围 256-16384；省略时为 4096。
    pub max_body_bytes: usize,
    /// delivery pending 数量阈值；达到后产生告警，省略时为 1。
    pub delivery_pending_count_threshold: i64,
    /// delivery pending 最老记录年龄阈值秒数，范围 1-2592000；省略时为 900 秒。
    pub delivery_pending_age_seconds: u64,
    /// delivery manual_reconcile 数量阈值；达到后产生告警，省略时为 1。
    pub delivery_manual_reconcile_count_threshold: i64,
    /// settlement fulfillment backlog 数量阈值；达到后产生告警，省略时为 1。
    pub fulfillment_backlog_count_threshold: i64,
    /// settlement fulfillment backlog 最老记录年龄阈值秒数，省略时为 900 秒。
    pub fulfillment_backlog_age_seconds: u64,
    /// recovery payment overdue 数量阈值；达到后产生告警，省略时为 1。
    pub recovery_overdue_count_threshold: i64,
    /// recovery payment overdue 最老记录年龄阈值秒数，省略时为 900 秒。
    pub recovery_overdue_age_seconds: u64,
    /// 运维告警通知使用的游戏世界，只允许 main 或 isekai；省略时为 main。
    pub world_id: String,
    /// 运维告警通知收件人；告警启用时必填，省略时为空字符串。
    pub recipient: String,
}

impl Default for OperationsAlertWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            scan_interval_seconds: 60,
            source_timeout_seconds: 5,
            batch_window_seconds: 2 * 60,
            cooldown_seconds: 30 * 60,
            claim_lease_seconds: 120,
            retry_delay_seconds: 5 * 60,
            max_signals_per_scan: 64,
            max_batch_items: 20,
            max_body_bytes: 4 << 10,
            delivery_pending_count_threshold: 1,
            delivery_pending_age_seconds: 15 * 60,
            delivery_manual_reconcile_count_threshold: 1,
            fulfillment_backlog_count_threshold: 1,
            fulfillment_backlog_age_seconds: 15 * 60,
            recovery_overdue_count_threshold: 1,
            recovery_overdue_age_seconds: 15 * 60,
            world_id: "main".to_owned(),
            recipient: String::new(),
        }
    }
}

impl OperationsAlertWorkerConfig {
    /// 将已校验的 TOML 设置转换为应用层告警策略。
    pub fn policy(&self) -> OperationsAlertPolicy {
        OperationsAlertPolicy {
            source_timeout: std::time::Duration::from_secs(self.source_timeout_seconds),
            batch_window: std::time::Duration::from_secs(self.batch_window_seconds),
            cooldown: std::time::Duration::from_secs(self.cooldown_seconds),
            claim_lease: std::time::Duration::from_secs(self.claim_lease_seconds),
            retry_delay: std::time::Duration::from_secs(self.retry_delay_seconds),
            max_signals_per_scan: self.max_signals_per_scan,
            max_batch_items: self.max_batch_items,
            max_body_bytes: self.max_body_bytes,
            delivery_pending_count_threshold: self.delivery_pending_count_threshold,
            delivery_pending_age_threshold: std::time::Duration::from_secs(
                self.delivery_pending_age_seconds,
            ),
            delivery_manual_reconcile_count_threshold: self
                .delivery_manual_reconcile_count_threshold,
            fulfillment_backlog_count_threshold: self.fulfillment_backlog_count_threshold,
            fulfillment_backlog_age_threshold: std::time::Duration::from_secs(
                self.fulfillment_backlog_age_seconds,
            ),
            recovery_overdue_count_threshold: self.recovery_overdue_count_threshold,
            recovery_overdue_age_threshold: std::time::Duration::from_secs(
                self.recovery_overdue_age_seconds,
            ),
        }
    }

    /// 将已校验的 TOML 设置转换为应用层告警收件目标。
    pub fn target(&self) -> OperationsAlertTarget {
        OperationsAlertTarget { world_id: self.world_id.clone(), recipient: self.recipient.clone() }
    }
}

/// Worker control Unix socket 的非敏感配置。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkerControlConfig {
    /// 是否启用 Worker control；省略时关闭。
    pub enabled: bool,
    /// Unix socket 所属组名；省略时使用旧版默认组 `auction-worker-control`。
    pub group: String,
    /// 允许访问控制面的内核认证 GID 白名单；省略时为空。
    pub allowed_peer_gids: Vec<u32>,
    /// 允许访问控制面的内核认证 UID 白名单；省略时为空。
    pub allowed_peer_uids: Vec<u32>,
    /// 预先建立的规范化绝对运行目录；省略时使用 `/run/hv-auction/worker`。
    pub runtime_directory: String,
    /// 规范化绝对 socket 路径；省略时使用 `/run/hv-auction/worker/control.sock`。
    pub socket_path: String,
}

impl Default for WorkerControlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            group: "auction-worker-control".to_owned(),
            allowed_peer_gids: Vec::new(),
            allowed_peer_uids: Vec::new(),
            runtime_directory: "/run/hv-auction/worker".to_owned(),
            socket_path: "/run/hv-auction/worker/control.sock".to_owned(),
        }
    }
}

/// Worker 使用的 E-Hentai 论坛主题同步策略。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ForumSyncWorkerConfig {
    /// 是否启用论坛主题创建/幂等查找；默认关闭，避免配置不完整时触发远程 POST。
    pub enabled: bool,
    /// active ForumCookie 凭据所属的 world scope；允许 main、isekai 或 shared。
    pub credential_world: String,
    /// 可选固定论坛 endpoint；省略时使用 `https://forums.e-hentai.org`。
    pub endpoint: Option<String>,
    /// 可选公开端 origin；配置后论坛主题会附带指向拍卖页面的绝对链接。
    pub public_base_url: Option<String>,
    /// 是否把 main/isekai 都映射到旧版测试板块 4；生产环境必须保持 false。
    pub test_board_mode: bool,
    /// 单轮最多领取的主题同步任务数，范围 1-100。
    pub batch_size: u16,
    /// 单次响应最大字节数，范围 1-8 MiB。
    pub maximum_response_bytes: usize,
    /// 连续论坛请求之间的最小间隔毫秒数。
    pub minimum_request_interval_millis: u64,
    /// 单次论坛 HTTP 请求超时秒数，范围 1-300。
    pub request_timeout_seconds: u64,
    /// 单个论坛同步任务的总处理超时秒数，必须小于 Worker 租约。
    pub item_timeout_seconds: u64,
    /// 单个论坛发布 claim 的租约秒数；每个主题在实际领取时单独计算。
    pub lease_duration_seconds: u64,
}

impl Default for ForumSyncWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            credential_world: "shared".to_owned(),
            endpoint: None,
            public_base_url: None,
            test_board_mode: false,
            batch_size: 100,
            maximum_response_bytes: 2 * 1024 * 1024,
            minimum_request_interval_millis: 750,
            request_timeout_seconds: 30,
            item_timeout_seconds: 93,
            lease_duration_seconds: 300,
        }
    }
}

/// Worker 使用的 MoogleMail 入站轮询策略。
///
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
    /// MoogleMail 拍卖邮件使用的起拍价覆盖值；省略时继承 `[auction.economics].start_price`。
    pub start_price: Option<i64>,
    /// MoogleMail 拍卖装备过滤策略；省略时关闭过滤但仍要求来源标记为可交易。
    pub equipment_filter: MoogleMailEquipmentFilterConfig,
    /// MoogleMail 业务通知模板；省略时使用旧版默认英文模板。
    pub mail_templates: MoogleMailNotificationTemplatesConfig,
    /// 可选 `.keyenv` 路径；相对路径相对于主 Worker 配置文件解析。
    pub keyenv_file: Option<PathBuf>,
    /// MoogleMail 固定生产 origin 或显式回环联调 origin。
    pub endpoint: String,
    /// 单次 MoogleMail 响应最大字节数，不能超过 8 MiB。
    pub maximum_response_bytes: usize,
    /// 同一客户端连续远端请求的最小间隔毫秒数。
    pub minimum_request_interval_millis: u64,
    /// 单次 MoogleMail HTTP 请求与响应正文读取的超时秒数。
    pub request_timeout_seconds: u64,
    /// 单次从 MoogleMail 读取的最大邮件数。
    pub batch_size: u16,
    /// 单次有界分页最多读取的页数。
    pub maximum_pages: usize,
    /// 每个 world 共用的标准五字段 cron；支持旧版描述符和时区前缀。
    pub schedule: String,
    /// 每个 cron 触发时刻后附加的随机抖动上限秒数。
    pub jitter_seconds: u64,
    /// 邮箱 claim 租约秒数；长批次处理期间会周期续租。
    pub lease_duration_seconds: u64,
    /// 邮箱 claim 续租周期秒数，必须小于租约。
    pub renew_interval_seconds: u64,
    /// 一轮远程读取的内联尝试次数。
    pub fetch_attempts: u16,
    /// 每次远程读取尝试的总超时秒数。
    pub fetch_timeout_seconds: u64,
    /// 单封邮件业务处理的内联尝试次数。
    pub process_attempts: u16,
    /// 单次业务处理尝试的总超时秒数。
    pub item_timeout_seconds: u64,
    /// 内联与持久化指数退避的基础秒数。
    pub retry_base_seconds: u64,
    /// 内联与持久化指数退避的最大秒数。
    pub retry_maximum_seconds: u64,
    /// 发件人 allowlist；未显式关闭 allowlist 时只读取其中的详情。
    pub sender_allowlist: Vec<String>,
    /// 明确关闭发件人 allowlist 后才读取所有发件人详情。
    pub sender_allowlist_disabled: bool,
    /// 额外启用的 MoogleMail 世界；每个条目拥有独立邮箱游标和发件人策略。
    ///
    /// 顶层字段继续描述第一个世界，以保持现有单世界配置可用；新增世界通过
    /// `[[worker.mooglemail.additional_worlds]]` 声明。每个条目的存在即表示该世界启用。
    pub additional_worlds: Vec<MoogleMailWorldConfig>,
}

/// MoogleMail 业务通知模板配置。
///
/// 模板键与旧版 runtime 的闭集保持一致；字段采用结构化配置而不是自由键值表，避免
/// 拼写错误或把未审计的占位符带入远端邮件。模板正文只允许 `{name}` 形式的已知占位符。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MoogleMailNotificationTemplatesConfig {
    /// 拍卖邮件包含被拒绝资产时的正文。
    pub auction_assets_rejected: String,
    /// 拍卖邮件没有附件时的正文。
    pub auction_no_attachments: String,
    /// 拍卖装备不符合条件时的正文；支持 `{reason_code}`。
    pub auction_equipment_rejected: String,
    /// 用户邮件包含拍卖资产时的正文。
    pub user_assets_rejected: String,
    /// 注册成功或恢复成功时发送登录 key；支持 `{login_key}`、`{username}`。
    pub register_key: String,
    /// 注册结果通知；支持 `{result_code}`、`{username}`。
    pub register_result: String,
    /// 重绑结果通知；支持 `{result_code}`、`{username}`。
    pub rebind_result: String,
    /// 主世界竞得装备并使用货到付款时的正文；支持 `{item_name}`、`{amount}`、`{reference}`。
    pub settlement_winner_equipment_cod: String,
    /// 异世界竞得付款载体时的正文；支持 `{amount}`、`{reference}`。
    pub settlement_payment_carrier: String,
    /// 结算付款超时时的正文；支持 `{reference}`。
    pub settlement_payment_reminder: String,
    /// 结算装备交付或退回时的正文；支持 `{item_name}`、`{reference}`。
    pub settlement_equipment_handoff: String,
    /// 卖家收款时的正文；支持 `{amount}`、`{reference}`。
    pub settlement_seller_payout: String,
    /// 买家退款时的正文；支持 `{amount}`、`{reference}`。
    pub settlement_buyer_refund: String,
    /// 运维告警正文；支持 `{summary}`。
    pub operations_alert: String,
}

impl Default for MoogleMailNotificationTemplatesConfig {
    fn default() -> Self {
        Self {
            auction_assets_rejected: "Auction assets rejected".to_owned(),
            auction_no_attachments: "Auction has no attachments".to_owned(),
            auction_equipment_rejected: "Auction equipment rejected: {reason_code}".to_owned(),
            user_assets_rejected: "User assets rejected".to_owned(),
            register_key: "Registration key {login_key} for {username}".to_owned(),
            register_result: "Registration result {result_code} {username}".to_owned(),
            rebind_result: "Rebind result {result_code} {username}".to_owned(),
            settlement_winner_equipment_cod:
                "Auction item {item_name}; COD {amount}; reference {reference}".to_owned(),
            settlement_payment_carrier: "Auction payment request {amount}; reference {reference}"
                .to_owned(),
            settlement_payment_reminder: "Auction payment is overdue; reference {reference}"
                .to_owned(),
            settlement_equipment_handoff: "Auction item {item_name}; reference {reference}"
                .to_owned(),
            settlement_seller_payout: "Auction seller payout {amount}; reference {reference}"
                .to_owned(),
            settlement_buyer_refund: "Auction buyer refund {amount}; reference {reference}"
                .to_owned(),
            operations_alert: "Operations alert: {summary}".to_owned(),
        }
    }
}

impl MoogleMailNotificationTemplatesConfig {
    /// 转换为 transport 启动时使用的稳定模板快照。
    pub fn values(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("auction_assets_rejected".to_owned(), self.auction_assets_rejected.clone()),
            ("auction_no_attachments".to_owned(), self.auction_no_attachments.clone()),
            ("auction_equipment_rejected".to_owned(), self.auction_equipment_rejected.clone()),
            ("user_assets_rejected".to_owned(), self.user_assets_rejected.clone()),
            ("register_key".to_owned(), self.register_key.clone()),
            ("register_result".to_owned(), self.register_result.clone()),
            ("rebind_result".to_owned(), self.rebind_result.clone()),
            (
                "settlement_winner_equipment_cod".to_owned(),
                self.settlement_winner_equipment_cod.clone(),
            ),
            ("settlement_payment_carrier".to_owned(), self.settlement_payment_carrier.clone()),
            ("settlement_payment_reminder".to_owned(), self.settlement_payment_reminder.clone()),
            ("settlement_equipment_handoff".to_owned(), self.settlement_equipment_handoff.clone()),
            ("settlement_seller_payout".to_owned(), self.settlement_seller_payout.clone()),
            ("settlement_buyer_refund".to_owned(), self.settlement_buyer_refund.clone()),
            ("operations_alert".to_owned(), self.operations_alert.clone()),
        ])
    }
}

/// MoogleMail 拍卖装备过滤策略。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MoogleMailEquipmentFilterConfig {
    /// 是否启用品质、前后缀、分类、类型和等级过滤；默认关闭。
    pub enabled: bool,
    /// 允许的品质 token；空数组表示不限制。
    pub qualities: Vec<String>,
    /// 允许的前缀 token；空数组表示不限制。
    pub prefixes: Vec<String>,
    /// 允许的后缀 token；空数组表示不限制。
    pub suffixes: Vec<String>,
    /// 允许的装备分类 token；空数组表示不限制。
    pub categories: Vec<String>,
    /// 允许的装备类型 token；空数组表示不限制。
    pub equipment_types: Vec<String>,
    /// 最低装备等级；0 表示不启用等级下限。
    pub level_min: i32,
    /// 最高装备等级；0 表示不启用等级上限，最大允许 1000。
    pub level_max: i32,
}

/// 单个 MoogleMail 世界的入站轮询配置。
///
/// 世界配置不包含 Cookie 或其他敏感值。Worker 会为每个世界建立独立的持久化邮箱状态，
/// 但复用同一组已校验的外部凭据池。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MoogleMailWorldConfig {
    /// 游戏世界，只允许 `main` 或 `isekai`。
    pub world_id: String,
    /// 该世界对应的持久化邮箱标识；必须与其他世界不同。
    pub mailbox_key: String,
    /// 该世界允许读取详情的发件人集合。
    pub sender_allowlist: Vec<String>,
    /// 明确设为 true 才读取该世界的所有发件人详情。
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
            mail_templates: MoogleMailNotificationTemplatesConfig::default(),
            keyenv_file: None,
            endpoint: "https://hentaiverse.org".to_owned(),
            maximum_response_bytes: 8 * 1024 * 1024,
            minimum_request_interval_millis: 1_000,
            request_timeout_seconds: 15,
            batch_size: 100,
            maximum_pages: 20,
            schedule: "* * * * *".to_owned(),
            jitter_seconds: 5,
            lease_duration_seconds: 60,
            renew_interval_seconds: 10,
            fetch_attempts: 3,
            fetch_timeout_seconds: 30,
            process_attempts: 3,
            item_timeout_seconds: 30,
            retry_base_seconds: 1,
            retry_maximum_seconds: 30,
            sender_allowlist: Vec::new(),
            sender_allowlist_disabled: false,
            additional_worlds: Vec::new(),
        }
    }
}

impl MoogleMailWorkerConfig {
    /// 返回按 `main`、`isekai` 稳定排序的全部已配置世界。
    pub fn configured_worlds(&self) -> Vec<MoogleMailWorldConfig> {
        let mut worlds = Vec::with_capacity(1 + self.additional_worlds.len());
        worlds.push(MoogleMailWorldConfig {
            world_id: self.world_id.clone(),
            mailbox_key: self.mailbox_key.clone(),
            sender_allowlist: self.sender_allowlist.clone(),
            sender_allowlist_disabled: self.sender_allowlist_disabled,
        });
        worlds.extend(self.additional_worlds.iter().cloned());
        worlds.sort_by_key(|world| if world.world_id == "main" { 0 } else { 1 });
        worlds
    }
}
