//! Delivery 页面组件之间共享的本地状态模型。

use crate::api::{Delivery, DeliveryAction};
use dioxus_i18n::t;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct DeliveryDialogState {
    pub(super) delivery: Delivery,
    pub(super) kind: DeliveryDialogKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeliveryDialogKind {
    Release,
    Retry,
    Reconcile,
}

impl DeliveryDialogKind {
    pub(super) const fn action(self) -> DeliveryAction {
        match self {
            Self::Release => DeliveryAction::Release,
            Self::Retry => DeliveryAction::Retry,
            Self::Reconcile => DeliveryAction::ReconcileRetry,
        }
    }

    pub(super) fn action_label(self) -> String {
        match self {
            Self::Release => t!("delivery-release"),
            Self::Retry => t!("delivery-retry"),
            Self::Reconcile => t!("delivery-reconcile"),
        }
    }

    pub(super) fn title(self) -> String {
        match self {
            Self::Release => t!("delivery-confirm-release-title"),
            Self::Retry => t!("delivery-confirm-retry-title"),
            Self::Reconcile => t!("delivery-reconcile-title"),
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
