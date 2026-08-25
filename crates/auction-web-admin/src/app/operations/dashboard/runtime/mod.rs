//! Admin Dashboard 运行时安全详情的模块出口。

mod details;
mod helpers;

pub(super) use details::DashboardRuntimeDetails;
pub(super) use helpers::{
    DashboardDetailCard, credential_scope_status, credential_scope_tone, runtime_bool,
    runtime_status, runtime_tone, section_unavailable,
};
