//! 出站通知模板 DTO；数据库修订事实在 HTTP 边界显式映射。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 一个模板键的当前状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailTemplateDto {
    /// 闭集模板键。
    pub key: String,
    /// 编辑框里的正文：运营保存过的正文，没有或已恢复默认时是内置默认正文。
    pub body: String,
    /// 程序内置的默认正文，供对照与恢复。
    pub default_body: String,
    /// 最新修订是否为运营保存的正文；`false` 表示正在使用内置默认正文。
    pub customized: bool,
    /// 运营保存的正文是否满足现行占位符规则；不满足时实际发信使用内置默认正文。
    pub usable: bool,
    /// 最新数据库版本；从未修订过为零，保存与恢复默认都要带上它。
    pub revision: i64,
    /// 最新修订的操作者；从未修订过为空。
    pub updated_by: Option<String>,
    /// 最新修订的数据库接收时间；从未修订过为空。
    pub updated_at: Option<DateTime<Utc>>,
    /// 正文必须包含的占位符（不含花括号）。
    pub required_placeholders: Vec<String>,
    /// 正文允许使用的全部占位符（不含花括号）。
    pub allowed_placeholders: Vec<String>,
}

/// 全部模板，顺序固定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailTemplateListResponse {
    /// 每个闭集模板键一项。
    pub templates: Vec<AdminMailTemplateDto>,
}

/// 保存或恢复默认一个模板的稳定命令。
///
/// 结果未知时必须连同 `command_id`、`expected_revision` 与 `body` 原样重试。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailTemplateUpdateRequest {
    /// 稳定命令 ID（UUID）。
    pub command_id: String,
    /// 读取到的最新版本；从未修订过为零。
    pub expected_revision: i64,
    /// 新正文；`null` 表示恢复内置默认正文。
    pub body: Option<String>,
}

/// 数据库已接受的修订；各角色约 5 秒内刷新生效。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailTemplateUpdateResponse {
    /// 修订后的模板状态。
    pub template: AdminMailTemplateDto,
}
