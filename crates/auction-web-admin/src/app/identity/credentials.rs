//! Admin 凭据管理页面的稳定组件出口。
//! 页面按加载编排、安全录入、列表操作、确认对话框和展示辅助拆分；凭据 secret 仍只在
//! 创建表单的短生命周期状态中存在，不进入列表或持久化状态。

#[path = "credentials/check.rs"]
mod check;
#[path = "credentials/dialogs.rs"]
mod dialogs;
#[path = "credentials/form.rs"]
mod form;
#[path = "credentials/helpers.rs"]
mod helpers;
#[path = "credentials/page.rs"]
mod page;
#[path = "credentials/table.rs"]
mod table;

pub(super) use check::CredentialCheckAction;
pub(super) use dialogs::CredentialDeleteDialog;
pub(super) use form::CredentialCreateForm;
pub(super) use helpers::{
    credential_kind_label, credential_status_class, credential_status_label, credential_world_label,
};
pub(super) use page::page;
pub(super) use table::{CredentialPager, CredentialTable};
