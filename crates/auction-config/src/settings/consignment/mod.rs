//! 三角色共用的寄售配置、范围校验与制品启用判定。

use std::ops::RangeInclusive;

use auction_domain::consignment::{
    ConsignmentSalePolicy, DEFAULT_DEPOSIT_TTL_DAYS, DEFAULT_DISPLAY_TTL_DAYS, FeeBps,
    ListingPublicationTtls,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::settings::errors::ConfigError;

mod activation;
#[cfg(test)]
mod activation_tests;
#[cfg(test)]
mod tests;
mod web_stack;
#[cfg(test)]
mod web_stack_tests;

pub use activation::process_consignment_activation;
pub use web_stack::{
    WebStackBinary, WebStackFacts, consignment_activation_with_web_stack, validate_web_stack,
};

/// 第 8 期正式制品具备玩家与 Admin 页面并通过双世界端到端验收后，由代码变更翻转。
pub const CONSIGNMENT_COMPLETE: bool = false;

/// 制品对已加载的寄售配置作出的启用决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsignmentActivation {
    /// 配置要求关闭寄售。
    Disabled,
    /// 当前制品允许按配置开启寄售。
    Enabled,
}

/// 按四个显式事实判定寄售能否开启，拒绝时返回配置错误。
///
/// | `enabled` | `complete` | `preview` | `debug` | 结果 |
/// | --- | --- | --- | --- | --- |
/// | false | 任意 | 任意 | 任意 | 关闭 |
/// | true | true | 任意 | 任意 | 允许 |
/// | true | false | true | true | 允许 |
/// | true | false | true | false | 拒绝 |
/// | true | false | false | true | 拒绝 |
/// | true | false | false | false | 拒绝 |
pub fn consignment_activation(
    enabled: bool,
    complete: bool,
    preview: bool,
    debug: bool,
) -> Result<ConsignmentActivation, ConfigError> {
    if !enabled {
        return Ok(ConsignmentActivation::Disabled);
    }
    if complete || (preview && debug) {
        return Ok(ConsignmentActivation::Enabled);
    }
    Err(ConfigError::ConsignmentPreviewRequired)
}

/// 已校验的寄售启动配置，由三角色从同一个公共文件读取。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsignmentConfig {
    enabled: bool,
    fee_bps: FeeBps,
    publication_ttls: ListingPublicationTtls,
    reservation_ttl_hours: i64,
    isekai_preclose_days: u8,
    max_open_per_user: i64,
    registrations_per_hour: i64,
    lookup_timeout_seconds: u64,
    lookup_max_response_bytes: usize,
    lookup_min_interval_ms: u64,
    sale_policy: ConsignmentSalePolicy,
    auto_relist_enabled: bool,
    section_digest: [u8; 32],
}

/// 配置段规范字节的版本前缀；改变编码必须换前缀，旧摘要随之不可比。
const SECTION_DIGEST_PREFIX: &[u8] = b"auction.consignment.config.v1\0";

impl ConsignmentConfig {
    /// 返回配置请求的开关值，启动时还需通过制品门。
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// 新寄售登记适用的手续费率；非寄售费率由领域固定为零。
    pub const fn fee_bps(&self) -> FeeBps {
        self.fee_bps
    }

    /// 核验通过时供领域冻结的展示期和待寄期。
    pub const fn publication_ttls(&self) -> ListingPublicationTtls {
        self.publication_ttls
    }

    /// 新寄件预约的最长有效期；签发时还会再受待寄截止与季末约束。
    pub const fn reservation_ttl_hours(&self) -> i64 {
        self.reservation_ttl_hours
    }

    /// isekai 换季前停止新预约与购买的提前本地日历日数。
    pub const fn isekai_preclose_days(&self) -> u8 {
        self.isekai_preclose_days
    }

    /// 成交时冻结进合同的恢复策略；范围已在加载时校验，固定常量由领域写入。
    pub const fn sale_policy(&self) -> ConsignmentSalePolicy {
        self.sale_policy
    }

    /// 单个用户跨世界的有效活动挂单上限。
    pub const fn max_open_per_user(&self) -> i64 {
        self.max_open_per_user
    }

    /// 单个用户滚动一小时内可接受的登记数上限。
    pub const fn registrations_per_hour(&self) -> i64 {
        self.registrations_per_hour
    }

    /// 一次解析尝试的端到端秒数预算。
    pub const fn lookup_timeout_seconds(&self) -> u64 {
        self.lookup_timeout_seconds
    }

    /// 解析响应解压后允许读取的字节数上限。
    pub const fn lookup_max_response_bytes(&self) -> usize {
        self.lookup_max_response_bytes
    }

    /// 全服务两次解析起始许可之间的最小毫秒数。
    pub const fn lookup_min_interval_ms(&self) -> u64 {
        self.lookup_min_interval_ms
    }

    /// 退回处置决策时是否允许自动重列；只在启动时读取，改动须重启。
    pub const fn auto_relist_enabled(&self) -> bool {
        self.auto_relist_enabled
    }

    /// 寄售配置段规范字节的 SHA-256，供装配根与开关一起冻结进自动重列决定（J12）。
    ///
    /// 摘要取校验后的有效值：省略字段与显式写出默认值得到同一摘要，任一字段的值变化都会改变摘要。
    pub const fn section_digest(&self) -> [u8; 32] {
        self.section_digest
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ConsignmentFileConfig {
    enabled: bool,
    fee_bps: i64,
    display_ttl_days: i64,
    deposit_ttl_days: i64,
    reservation_ttl_hours: i64,
    isekai_preclose_days: i64,
    max_open_per_user: i64,
    registrations_per_hour: i64,
    lookup_timeout_seconds: i64,
    lookup_max_response_bytes: i64,
    lookup_min_interval_ms: i64,
    payment_deadline_days: i64,
    cod_return_limit: i64,
    isekai_refusal_limit: i64,
    auto_relist_enabled: bool,
}

impl Default for ConsignmentFileConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            fee_bps: i64::from(FeeBps::DEFAULT_CONSIGNED.value()),
            display_ttl_days: DEFAULT_DISPLAY_TTL_DAYS,
            deposit_ttl_days: DEFAULT_DEPOSIT_TTL_DAYS,
            reservation_ttl_hours: 24,
            isekai_preclose_days: 3,
            max_open_per_user: 20,
            registrations_per_hour: 10,
            lookup_timeout_seconds: 10,
            lookup_max_response_bytes: 65_536,
            lookup_min_interval_ms: 1_000,
            payment_deadline_days: 7,
            cod_return_limit: 3,
            isekai_refusal_limit: 3,
            auto_relist_enabled: false,
        }
    }
}

impl ConsignmentFileConfig {
    pub(super) fn resolve(self) -> Result<ConsignmentConfig, ConfigError> {
        // 不变量 6、7：包括关闭时也必须拒绝非法数值，诊断只含键名和允许范围。
        let fee_bps =
            FeeBps::new(self.fee_bps).map_err(|_error| range_error("fee_bps", 0..=9_999))?;
        let publication_ttls =
            ListingPublicationTtls::new(self.display_ttl_days, self.deposit_ttl_days).map_err(
                |_error| {
                    // 领域构造器校验两种期限；这里只在失败后定位要修正的键。
                    if !(1..=90).contains(&self.display_ttl_days) {
                        range_error("display_ttl_days", 1..=90)
                    } else {
                        range_error("deposit_ttl_days", 1..=30)
                    }
                },
            )?;
        let sale_policy = ConsignmentSalePolicy::new(
            narrow(
                "payment_deadline_days",
                self.payment_deadline_days,
                ConsignmentSalePolicy::PAYMENT_DEADLINE_DAYS,
            )?,
            narrow(
                "cod_return_limit",
                self.cod_return_limit,
                ConsignmentSalePolicy::COD_RETURN_LIMIT,
            )?,
            narrow(
                "isekai_refusal_limit",
                self.isekai_refusal_limit,
                ConsignmentSalePolicy::ISEKAI_REFUSAL_LIMIT,
            )?,
        )
        .map_err(|_error| range_error("payment_deadline_days", 1..=90))?;
        Ok(ConsignmentConfig {
            enabled: self.enabled,
            fee_bps,
            publication_ttls,
            reservation_ttl_hours: in_range(
                "reservation_ttl_hours",
                self.reservation_ttl_hours,
                1..=168,
            )?,
            isekai_preclose_days: u8::try_from(in_range(
                "isekai_preclose_days",
                self.isekai_preclose_days,
                1..=i64::from(auction_domain::consignment::MAX_PRECLOSE_DAYS),
            )?)
            .map_err(|_error| {
                range_error(
                    "isekai_preclose_days",
                    1..=i64::from(auction_domain::consignment::MAX_PRECLOSE_DAYS),
                )
            })?,
            max_open_per_user: in_range("max_open_per_user", self.max_open_per_user, 1..=100)?,
            registrations_per_hour: in_range(
                "registrations_per_hour",
                self.registrations_per_hour,
                1..=100,
            )?,
            lookup_timeout_seconds: in_range(
                "lookup_timeout_seconds",
                self.lookup_timeout_seconds,
                1..=30,
            )? as u64,
            lookup_max_response_bytes: in_range(
                "lookup_max_response_bytes",
                self.lookup_max_response_bytes,
                4_096..=262_144,
            )? as usize,
            lookup_min_interval_ms: in_range(
                "lookup_min_interval_ms",
                self.lookup_min_interval_ms,
                250..=60_000,
            )? as u64,
            sale_policy,
            auto_relist_enabled: self.auto_relist_enabled,
            section_digest: self.section_digest(),
        })
    }

    /// 按固定键序编码整节的有效值并取摘要；只在全部字段通过校验后调用。
    fn section_digest(&self) -> [u8; 32] {
        let flags = [("enabled", self.enabled), ("auto_relist_enabled", self.auto_relist_enabled)];
        let numbers = [
            ("fee_bps", self.fee_bps),
            ("display_ttl_days", self.display_ttl_days),
            ("deposit_ttl_days", self.deposit_ttl_days),
            ("reservation_ttl_hours", self.reservation_ttl_hours),
            ("isekai_preclose_days", self.isekai_preclose_days),
            ("max_open_per_user", self.max_open_per_user),
            ("registrations_per_hour", self.registrations_per_hour),
            ("lookup_timeout_seconds", self.lookup_timeout_seconds),
            ("lookup_max_response_bytes", self.lookup_max_response_bytes),
            ("lookup_min_interval_ms", self.lookup_min_interval_ms),
            ("payment_deadline_days", self.payment_deadline_days),
            ("cod_return_limit", self.cod_return_limit),
            ("isekai_refusal_limit", self.isekai_refusal_limit),
        ];
        let mut hasher = Sha256::new();
        hasher.update(SECTION_DIGEST_PREFIX);
        for (key, value) in flags {
            hasher.update(format!("{key}={value}\n").as_bytes());
        }
        for (key, value) in numbers {
            hasher.update(format!("{key}={value}\n").as_bytes());
        }
        hasher.finalize().into()
    }
}

/// 按领域公布的范围校验并收窄到策略字段的整数类型，越界时报告键名与范围。
fn narrow<T: TryFrom<i64> + Into<i64> + Copy>(
    field: &'static str,
    value: i64,
    range: RangeInclusive<T>,
) -> Result<T, ConfigError> {
    let (min, max) = (((*range.start()).into()), ((*range.end()).into()));
    let in_bounds = in_range(field, value, min..=max)?;
    T::try_from(in_bounds).map_err(|_error| range_error(field, min..=max))
}

fn in_range(
    field: &'static str,
    value: i64,
    range: RangeInclusive<i64>,
) -> Result<i64, ConfigError> {
    if range.contains(&value) { Ok(value) } else { Err(range_error(field, range)) }
}

fn range_error(field: &'static str, range: RangeInclusive<i64>) -> ConfigError {
    ConfigError::InvalidConsignmentRange { field, min: *range.start(), max: *range.end() }
}
