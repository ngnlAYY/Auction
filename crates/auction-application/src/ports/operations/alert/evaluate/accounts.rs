//! 外部账号与通知面的阈值评估。

use super::super::OperationsAlertError;
use super::super::model::{
    OperationsAlertCredentialSnapshot, OperationsAlertForumSnapshot, OperationsAlertMailSnapshot,
    OperationsAlertNotificationSnapshot, OperationsAlertSourceObservation,
};
use super::super::signals::{
    OPERATIONS_ALERT_MAIL_POLL_STALL_AGE, OperationsAlertKind, OperationsAlertSignal,
    OperationsAlertSource, reached,
};

pub(super) fn evaluate_mail(
    snapshot: Option<OperationsAlertMailSnapshot>,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Mail);
    };
    if snapshot.last_poll_age_seconds < 0
        || snapshot.consecutive_failures < 0
        || snapshot.sent_pages_scanned.is_some_and(|pages| pages < 0)
        || snapshot.sent_page_limit.is_some_and(|limit| !(1..=1_000).contains(&limit))
        || snapshot.sent_pages_scanned.is_some() != snapshot.sent_page_limit.is_some()
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    // 显式暂停的邮箱已在读取层排除；这里只判断失败计数与水位超限。
    if snapshot.consecutive_failures > 0
        || reached(snapshot.last_poll_age_seconds, OPERATIONS_ALERT_MAIL_POLL_STALL_AGE)
    {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::MailPollStalled,
            "mail",
            OperationsAlertSource::Mail,
            snapshot.consecutive_failures,
            snapshot.last_poll_age_seconds,
        )?);
    }
    if let (Some(pages), Some(limit)) = (snapshot.sent_pages_scanned, snapshot.sent_page_limit)
        && pages > 0
        && pages.saturating_mul(100) >= limit.saturating_mul(80)
    {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::MailSentPageLimitApproaching,
            "mail",
            OperationsAlertSource::Mail,
            pages,
            0,
        )?);
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Mail, signals)
}

pub(super) fn evaluate_credential(
    snapshot: Option<OperationsAlertCredentialSnapshot>,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Credential);
    };
    if snapshot.invalid_mail_cookie < 0 || snapshot.invalid_external_api < 0 {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    for (scope, count) in [
        ("mail_cookie", snapshot.invalid_mail_cookie),
        ("external_api", snapshot.invalid_external_api),
    ] {
        if count > 0 {
            signals.push(OperationsAlertSignal::new(
                OperationsAlertKind::CredentialInvalid,
                scope,
                OperationsAlertSource::Credential,
                count,
                0,
            )?);
        }
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Credential, signals)
}

pub(super) fn evaluate_forum(
    snapshot: Option<OperationsAlertForumSnapshot>,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Forum);
    };
    if snapshot.sync_failed_count < 0
        || snapshot.oldest_sync_failed_age_seconds < 0
        || snapshot.message_failed_count < 0
        || snapshot.oldest_message_failed_age_seconds < 0
        || (snapshot.sync_failed_count == 0 && snapshot.oldest_sync_failed_age_seconds != 0)
        || (snapshot.message_failed_count == 0 && snapshot.oldest_message_failed_age_seconds != 0)
        || snapshot.remote_page_count.is_some_and(|pages| pages < 0)
        || snapshot.remote_page_limit.is_some_and(|limit| !(1..=1_000).contains(&limit))
        || snapshot.remote_page_count.is_some() != snapshot.remote_page_limit.is_some()
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    if snapshot.sync_failed_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::ForumSyncFailed,
            "forum",
            OperationsAlertSource::Forum,
            snapshot.sync_failed_count,
            snapshot.oldest_sync_failed_age_seconds,
        )?);
    }
    if snapshot.message_failed_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::ForumMessageFailed,
            "forum",
            OperationsAlertSource::Forum,
            snapshot.message_failed_count,
            snapshot.oldest_message_failed_age_seconds,
        )?);
    }
    if let (Some(pages), Some(limit)) = (snapshot.remote_page_count, snapshot.remote_page_limit)
        && pages > 0
        && pages.saturating_mul(100) >= limit.saturating_mul(80)
    {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::ForumTopicPageLimitApproaching,
            "forum",
            OperationsAlertSource::Forum,
            pages,
            0,
        )?);
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Forum, signals)
}

pub(super) fn evaluate_notification(
    snapshot: Option<OperationsAlertNotificationSnapshot>,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Notification);
    };
    if snapshot.failed_count < 0
        || snapshot.oldest_failed_age_seconds < 0
        || snapshot.postage_insufficient_count < 0
        || snapshot.oldest_postage_age_seconds < 0
        || snapshot.postage_insufficient_count > snapshot.failed_count
        || (snapshot.failed_count == 0 && snapshot.oldest_failed_age_seconds != 0)
        || (snapshot.postage_insufficient_count == 0 && snapshot.oldest_postage_age_seconds != 0)
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    // 邮资不足是失败的一个子集，仍计入总失败水位，不互相替代。
    if snapshot.postage_insufficient_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::NotificationPostageInsufficient,
            "notification",
            OperationsAlertSource::Notification,
            snapshot.postage_insufficient_count,
            snapshot.oldest_postage_age_seconds,
        )?);
    }
    if snapshot.failed_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::NotificationDeliveryFailed,
            "notification",
            OperationsAlertSource::Notification,
            snapshot.failed_count,
            snapshot.oldest_failed_age_seconds,
        )?);
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Notification, signals)
}
