//! 权利通知的类型：类别、处置与专用 outbox 请求。
//!
//! 权利通知不进分类开关，类别 settlement（裁定 J10）；挑战码类走专用密文 outbox，
//! 不进 `deliveries`。投递执行属于 7D-5，本文件只定接口两端共同的值。

use std::fmt;

use uuid::Uuid;

use super::crypto::SealedChallengeSecret;

/// 通知种类，对应 `deliver_right_notice` 的工作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RightNoticeKind {
    /// 定位符指引（`locate`）：只含有界的定位符指引，不含挑战码。
    LocatorGuidance,
    /// 挑战码（`challenge`）：专用密文，传输边界解封。
    Challenge,
    /// 损失回执。
    LossReceipt,
    /// 原权利通知。
    RightNotice,
}

impl RightNoticeKind {
    /// 全部种类；存储层的 `c7_notice_kind_ck` 必须全部接受。
    pub const ALL: [Self; 4] =
        [Self::LocatorGuidance, Self::Challenge, Self::LossReceipt, Self::RightNotice];

    /// 稳定码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocatorGuidance => "locator_guide",
            Self::Challenge => "challenge",
            Self::LossReceipt => "loss_receipt",
            Self::RightNotice => "right_notice",
        }
    }
}

/// `deliver_right_notice` 的处置闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RightNoticeDisposition {
    /// 已发送。
    Sent,
    /// 发送结果未知；绝不自动重发，转人工对账。
    Unknown,
    /// 已抑制（身份变更、预算或关闭）。
    Suppressed,
}

impl RightNoticeDisposition {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sent => "notice_sent",
            Self::Unknown => "notice_unknown",
            Self::Suppressed => "notice_suppressed",
        }
    }
}

/// 写入专用 outbox 的通知请求；只向冻结身份派发。
#[derive(Clone, PartialEq, Eq)]
pub struct RightNoticeRequest {
    /// 唯一工作 ID。
    pub work_id: Uuid,
    /// 种类。
    pub kind: RightNoticeKind,
    /// 冻结身份摘要；传输边界据此取回原冻结身份，改名后不发给新名字。
    pub identity_digest: [u8; 32],
    /// 关联挑战。
    pub challenge_id: Option<Uuid>,
    /// 关联权利。
    pub right_id: Option<Uuid>,
    /// 挑战码密文；定位符指引没有。
    pub sealed: Option<SealedChallengeSecret>,
}

impl fmt::Debug for RightNoticeRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RightNoticeRequest")
            .field("work_id", &self.work_id)
            .field("kind", &self.kind)
            .field("challenge_id", &self.challenge_id)
            .field("right_id", &self.right_id)
            .field("sealed", &self.sealed.as_ref().map(|_| "[REDACTED]"))
            .finish_non_exhaustive()
    }
}
