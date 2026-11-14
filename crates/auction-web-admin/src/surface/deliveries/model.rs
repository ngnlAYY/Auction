//! Delivery 页面组件之间共享的本地状态模型。

use auction_web_shared::t;

use super::labels::is_mail_disposition_conflict;
use crate::api::{Delivery, DeliveryAction};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct DeliveryDialogState {
    pub(super) delivery: Delivery,
    pub(super) kind: DeliveryDialogKind,
}

/// 页面 owner 持有的人工对账草稿；同一 delivery 重开时保留，不同 delivery 切换时替换。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeliveryReconcileDraft {
    pub(super) delivery_id: String,
    pub(super) action: String,
    pub(super) external_reference: String,
    pub(super) failure_code: String,
}

impl DeliveryReconcileDraft {
    pub(super) fn for_delivery(delivery: &Delivery) -> Self {
        Self {
            delivery_id: delivery.id.clone(),
            action: if is_mail_disposition_conflict(delivery) { "failed" } else { "retry" }
                .to_owned(),
            external_reference: String::new(),
            failure_code: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeliveryDialogKind {
    Release,
    Retry,
    Reconcile,
    ReconcileRetry,
}

impl DeliveryDialogKind {
    pub(super) const fn action(self) -> DeliveryAction {
        match self {
            Self::Release => DeliveryAction::Release,
            Self::Retry => DeliveryAction::Retry,
            Self::Reconcile | Self::ReconcileRetry => DeliveryAction::ReconcileRetry,
        }
    }

    pub(super) fn action_label(self) -> String {
        match self {
            Self::Release => t!("delivery-release"),
            Self::Retry => t!("delivery-retry"),
            Self::Reconcile => t!("delivery-reconcile"),
            Self::ReconcileRetry => t!("delivery-retry"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeliveryActionRequest {
    pub(super) delivery_id: String,
    pub(super) action: DeliveryAction,
    pub(super) expected_fencing_version: i64,
    pub(super) external_reference: Option<String>,
    pub(super) failure_code: Option<String>,
}
