//! Admin dashboard section 的安全降级结果。

use auction_contracts::AdminDashboardSectionResponse;
use auction_observability::DashboardStatus;

pub(crate) fn unknown_section() -> AdminDashboardSectionResponse {
    unknown_section_with_code("source_unavailable")
}

pub(crate) fn unknown_section_with_code(error_code: &'static str) -> AdminDashboardSectionResponse {
    AdminDashboardSectionResponse {
        status: DashboardStatus::Unknown.as_str().to_owned(),
        observed_at: None,
        error_code: Some(error_code.to_owned()),
        data: None,
    }
}
