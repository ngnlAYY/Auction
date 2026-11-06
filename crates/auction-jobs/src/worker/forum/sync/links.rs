//! 公开地址门禁：基址 origin 校验与相对引用解析。

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use url::Url;

use crate::worker::diagnostics;

pub(crate) fn valid_public_base_url(value: &str) -> bool {
    let Ok(parsed) = Url::parse(value) else {
        return false;
    };
    (parsed.scheme() == "https"
        || (parsed.scheme() == "http"
            && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && (parsed.path().is_empty() || parsed.path() == "/")
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && value.trim() == value
}

pub(crate) fn resolve_public_url(
    base_url: Option<&str>,
    reference: Option<&str>,
) -> Result<Option<String>, ApplicationError> {
    let Some(base_url) = base_url else {
        return Ok(None);
    };
    let Some(reference) = reference.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let base = Url::parse(base_url).map_err(|error| {
        diagnostics::warn(
            "forum_sync",
            "parse_public_base_url",
            error.diagnostic_kind(),
            DiagnosticContext::default(),
        );

        ApplicationError::Repository(auction_application::core::RepositoryError::Failed(
            "论坛公开地址配置无效".to_owned(),
        ))
    })?;
    if !reference.starts_with('/')
        || reference.starts_with("//")
        || reference.contains('#')
        || reference.chars().any(char::is_control)
    {
        return Err(ApplicationError::Repository(
            auction_application::core::RepositoryError::Failed("论坛公开地址引用无效".to_owned()),
        ));
    }
    let resolved = base.join(reference).map_err(|error| {
        diagnostics::warn(
            "forum_sync",
            "join_public_url",
            error.diagnostic_kind(),
            DiagnosticContext::default(),
        );

        ApplicationError::Repository(auction_application::core::RepositoryError::Failed(
            "论坛公开地址引用无效".to_owned(),
        ))
    })?;
    if resolved.scheme() != base.scheme() || resolved.host() != base.host() {
        return Err(ApplicationError::Repository(
            auction_application::core::RepositoryError::Failed("论坛公开地址引用无效".to_owned()),
        ));
    }
    Ok(Some(resolved.to_string()))
}
