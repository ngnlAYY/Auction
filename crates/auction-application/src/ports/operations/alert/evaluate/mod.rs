//! operations alert 各 Worker 来源的阈值 evaluator。

use super::OperationsAlertError;
use super::model::{
    OperationsAlertPolicy, OperationsAlertSnapshot, OperationsAlertSourceObservation,
};

/// 从全部来源快照生成完整 signal 集合；来源闭集见 `OperationsAlertSource::WORKER`。
pub fn evaluate_snapshot(
    snapshot: &OperationsAlertSnapshot,
    policy: &OperationsAlertPolicy,
) -> Result<Vec<OperationsAlertSourceObservation>, OperationsAlertError> {
    policy.validate()?;
    let result = vec![
        evaluate_process(snapshot.process.as_ref())?,
        evaluate_delivery(snapshot.delivery, policy)?,
        evaluate_fulfillment(snapshot.fulfillment, policy)?,
        evaluate_recovery(snapshot.recovery.as_ref(), policy)?,
        evaluate_mail(snapshot.mail)?,
        evaluate_credential(snapshot.credential)?,
        evaluate_lifecycle(snapshot.lifecycle)?,
        evaluate_forum(snapshot.forum)?,
        evaluate_notification(snapshot.notification)?,
        evaluate_reconciliation(snapshot.reconciliation.as_ref())?,
    ];
    let total = result.iter().map(|observation| observation.signals.len()).sum::<usize>();
    if total > policy.max_signals_per_scan {
        return Err(OperationsAlertError::Invalid);
    }
    Ok(result)
}

mod accounts;
mod operations;
mod settlement;

use self::accounts::{evaluate_credential, evaluate_forum, evaluate_mail, evaluate_notification};
use self::operations::{evaluate_lifecycle, evaluate_process, evaluate_reconciliation};
use self::settlement::{evaluate_delivery, evaluate_fulfillment, evaluate_recovery};
