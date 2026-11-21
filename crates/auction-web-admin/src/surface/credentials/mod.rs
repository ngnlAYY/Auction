//! Admin 凭据管理页面的稳定组件出口。
//! 页面按加载编排、安全录入、列表操作、确认对话框和展示辅助拆分；凭据 secret 仍只在
//! 创建表单的短生命周期状态中存在，不进入列表或持久化状态。

mod check;
mod dialogs;
mod field_validation;
mod form;
mod labels;
mod page;
mod table;

pub(crate) use check::CredentialCheckAction;
pub(crate) use dialogs::CredentialDeleteDialog;
pub(crate) use form::CredentialCreateForm;
pub(crate) use labels::{
    credential_kind_label, credential_status_class, credential_status_label, credential_world_label,
};
pub(crate) use page::page;
pub(crate) use table::CredentialTable;
