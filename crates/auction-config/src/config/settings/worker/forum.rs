use auction_application::EquipmentColorRule;
use serde::Deserialize;

/// Worker 使用的 E-Hentai 论坛主题同步策略。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ForumSyncWorkerConfig {
    /// 是否启用论坛主题创建/幂等查找；默认关闭，避免配置不完整时触发远程 POST。
    pub enabled: bool,
    /// 正常发布/刷新是否额外回读 BBCode；默认关闭，回帖未知写入始终只读核对。
    pub verify_post_body: bool,
    /// active ForumCookie 凭据所属的 world scope；允许 main、isekai 或 shared。
    pub credential_world: String,
    /// 可选固定论坛 endpoint；省略时使用 `https://forums.e-hentai.org`。
    pub endpoint: Option<String>,
    /// 可选公开端 origin；配置后论坛主题会附带指向拍卖页面的绝对链接。
    pub public_base_url: Option<String>,
    /// 是否把 main/isekai 都映射到旧版测试板块 4；生产环境必须保持 false。
    pub test_board_mode: bool,
    /// 主世界拍卖帖板块 ID；默认 77（WTS）。
    pub main_board_id: i32,
    /// 异世界拍卖帖板块 ID；默认 90。
    pub isekai_board_id: i32,
    /// 单轮最多领取的主题同步任务数，范围 1-100。
    pub batch_size: u16,
    /// 论坛发布与回帖导入轮询间隔秒数；默认五分钟，独立于拍卖生命周期轮询。
    pub poll_interval_seconds: u64,
    /// 论坛轮询抖动秒数，用于错开多个 Worker 的领取时刻。
    pub jitter_seconds: u64,
    /// 单次响应最大字节数，范围 1-8 MiB。
    pub maximum_response_bytes: usize,
    /// 单次主题扫描允许读取的最大页数，范围 1-20。
    pub maximum_topic_pages: u16,
    pub minimum_request_interval_millis: u64,
    /// 单次论坛 HTTP 请求超时秒数，范围 1-300。
    pub request_timeout_seconds: u64,
    /// 单个论坛同步任务的总处理超时秒数，必须小于 Worker 租约。
    pub item_timeout_seconds: u64,
    /// 单个论坛发布 claim 的租约秒数；每个主题在实际领取时单独计算。
    pub lease_duration_seconds: u64,
    /// Admin runtime 投影的装备名称标色规则；不由 worker.toml 直接配置。
    #[serde(skip)]
    pub equipment_colors: Vec<EquipmentColorRule>,
}

impl Default for ForumSyncWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            verify_post_body: false,
            credential_world: "shared".to_owned(),
            endpoint: None,
            public_base_url: None,
            test_board_mode: false,
            main_board_id: 77,
            isekai_board_id: 90,
            batch_size: 100,
            poll_interval_seconds: 300,
            jitter_seconds: 5,
            maximum_response_bytes: 2 * 1024 * 1024,
            maximum_topic_pages: 10,
            minimum_request_interval_millis: 750,
            request_timeout_seconds: 30,
            item_timeout_seconds: 240,
            lease_duration_seconds: 300,
            equipment_colors: Vec::new(),
        }
    }
}
