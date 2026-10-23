use std::{collections::BTreeMap, path::PathBuf};

use auction_application::default_accepted_qualities;
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
    /// MoogleMail 业务通知模板；省略时使用旧版默认英文模板。
    pub mail_templates: MoogleMailNotificationTemplatesConfig,
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
    /// 每个 world 共用的标准五字段 cron；支持旧版描述符和时区前缀。
    pub schedule: String,
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

/// MoogleMail 业务通知模板配置。
/// 模板键与旧版 runtime 的闭集保持一致；字段采用结构化配置而不是自由键值表，避免
/// 拼写错误或把未审计的占位符带入远端邮件。模板正文只允许 `{name}` 形式的已知占位符。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MoogleMailNotificationTemplatesConfig {
    pub auction_assets_rejected: String,
    pub auction_no_attachments: String,
    /// 装备拒收正文；原因至少保留 `{reason_code}` 或 `{reason_text}`，另支持
    /// `{next_steps}` 与 `{mail_reference}`，旧自定义正文继续有效。
    pub auction_equipment_rejected: String,
    pub user_assets_rejected: String,
    /// 注册成功或恢复成功时发送登录 key；支持 `{login_key}`、`{username}`。
    pub register_key: String,
    /// 注册结果通知；支持 `{result_code}`、`{username}`。
    pub register_result: String,
    /// 重绑结果通知；支持 `{result_code}`、`{username}`。
    pub rebind_result: String,
    /// 主世界竞得装备的货到付款正文；要求 `{item_name}`、`{amount}`、`{reference}`，
    /// 可选 `{price_breakdown}`，旧载荷不具备冻结明细时该块为空。
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
            auction_assets_rejected: concat!(
                "Your auction submission was not accepted.\n",
                "\n",
                "Auction submissions must contain equipment only.\n",
                "Please remove Credits, CoD and other item types\n",
                "before submitting again.",
            )
            .to_owned(),
            auction_no_attachments: concat!(
                "No equipment attachments were found in your message.\n",
                "\n",
                "Attach the equipment you want to submit,\n",
                "then send a new message with the subject \"auction\".",
            )
            .to_owned(),
            auction_equipment_rejected: concat!(
                "Your auction submission was not accepted.\n",
                "\n",
                "Reason:\n",
                "{reason_text}\n",
                "\n",
                "Equipment return:\n",
                "A return has been requested for all equipment\n",
                "in your original message.\n",
                "\n",
                "If you submitted multiple items together,\n",
                "the entire submission will be returned.\n",
                "\n",
                "What to do next:\n",
                "{next_steps}\n",
                "\n",
                "Original mail reference: {mail_reference}",
            )
            .to_owned(),
            user_assets_rejected: concat!(
                "Your account request was not accepted.\n",
                "\n",
                "Account requests must not include equipment,\n",
                "Credits, other items or CoD.\n",
                "\n",
                "Please resend your request without these assets.",
            )
            .to_owned(),
            register_key: concat!(
                "Your HV-Auction login key is ready.\n",
                "\n",
                "Player: {username}\n",
                "\n",
                "Login key:\n",
                "{login_key}\n",
                "\n",
                "Use this key to sign in to HV-Auction.\n",
                "Keep it private.",
            )
            .to_owned(),
            register_result: concat!(
                "Your registration could not be completed.\n",
                "\n",
                "Player: {username}\n",
                "Result code: {result_code}\n",
                "\n",
                "Contact the auction operator if you need help\n",
                "resolving this issue.",
            )
            .to_owned(),
            rebind_result: concat!(
                "Your account rebind request has been processed.\n",
                "\n",
                "Player: {username}\n",
                "Result code: {result_code}\n",
                "\n",
                "Contact the auction operator if you need help\n",
                "understanding the result.",
            )
            .to_owned(),
            settlement_winner_equipment_cod: concat!(
                "Your auction item is ready to collect.\n",
                "\n",
                "Item: {item_name}\n",
                "\n",
                "{price_breakdown}Amount due (CoD): {amount} Credits\n",
                "\n",
                "Collect the attached equipment to pay the CoD\n",
                "and receive your item.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            settlement_payment_carrier: concat!(
                "This mail collects payment for your Isekai auction item.\n",
                "\n",
                "CoD: {amount} Credits\n",
                "\n",
                "Collect the attached Credits to pay the CoD.\n",
                "Your equipment will be sent separately in Isekai\n",
                "after payment is confirmed.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            settlement_payment_reminder: concat!(
                "Payment for this auction transaction is overdue.\n",
                "\n",
                "Please check your original auction payment mail.\n",
                "If you have already paid, contact the auction\n",
                "operator with the reference below.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            settlement_equipment_handoff: concat!(
                "The following equipment is attached.\n",
                "\n",
                "Item: {item_name}\n",
                "\n",
                "Please collect the attachment.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            settlement_seller_payout: concat!(
                "Your auction payout is attached.\n",
                "\n",
                "Amount: {amount} Credits\n",
                "\n",
                "Please collect the attached Credits.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            settlement_buyer_refund: concat!(
                "Your auction refund is attached.\n",
                "\n",
                "Amount: {amount} Credits\n",
                "\n",
                "Please collect the attached Credits.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            operations_alert: concat!(
                "An auction service issue needs attention.\n",
                "\n",
                "{summary}\n",
                "\n",
                "Please review the Admin dashboard for details.",
            )
            .to_owned(),
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
            mail_templates: MoogleMailNotificationTemplatesConfig::default(),
            diagnostics: MoogleMailDiagnosticsConfig::default(),
            keyenv_file: None,
            endpoint: "https://hentaiverse.org".to_owned(),
            maximum_response_bytes: 8 * 1024 * 1024,
            minimum_request_interval_millis: 1_000,
            send_intervals: auction_application::DeliverySendIntervals::default(),
            request_timeout_seconds: 15,
            batch_size: 100,
            maximum_pages: 20,
            schedule: "*/5 * * * *".to_owned(),
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
