//! 在命令底座的 Serializable 事务内终结超期或耗尽的解析。

use auction_application::consignment::{
    AutoCloseCandidate, AutoCloseEvidence, ListingOperation, LookupFence,
};
use auction_application::core::RequestHash;
use auction_domain::consignment::{
    ForcedTerminationFacts, ListingChange, ListingReason, ListingTransition, ListingTrigger,
    transition_listing,
};

use super::admission::db_now;
use super::control::lock_control;
use super::reclaim::{
    ReclaimMode, TerminalEvidence, WorkCompletion, finish_work, lock_reclaimable, read_resolution,
    terminal_evidence,
};
use super::store::ReapCommands;
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::{WriteError, invalid_projection};
use crate::database::consignment::commands::facts::{ChangedOperation, Decision};
use crate::database::consignment::commands::ledger::{Actor, Arbitration, RequestProof, Target};
use crate::database::consignment::commands::session::{
    CommandTransaction, LockKeys, with_command_transaction,
};
use crate::database::consignment::eligibility::accounts::lock_listing_seller;
use crate::database::consignment::listings::projection::{
    ListingLockIdentity, lock_listings, pre_read_listing_identity,
};

struct Terminal {
    fence: LookupFence,
    mode: ReclaimMode,
    evidence: TerminalEvidence,
    seller: ListingLockIdentity,
    request_hash: RequestHash,
}

enum Outcome {
    Lost,
    Applied,
    RecheckDeadline(auction_time::Timestamp),
}

pub(super) async fn finish(
    store: &DatabaseStore,
    commands: &ReapCommands,
    fence: LookupFence,
    mode: ReclaimMode,
    evidence: TerminalEvidence,
) -> Result<bool, WriteError> {
    let Some(identity) = pre_read_listing_identity(store, fence.listing_id()).await? else {
        return Ok(false);
    };
    let sellers = [identity.seller_user_id];
    let equipment = [identity.equipment.clone()];
    let mut evidence = evidence;
    loop {
        let (command_id, request_hash) = match evidence {
            TerminalEvidence::Deadline(deadline) => {
                let candidate = AutoCloseCandidate {
                    listing_id: fence.listing_id(),
                    evidence: AutoCloseEvidence::LookupDeadline(deadline),
                };
                let command = (commands.deadline)(candidate);
                if command.candidate != candidate {
                    return Err(WriteError::invariant("consignment_lookup_deadline_command"));
                }
                (command.command_id, command.request_hash)
            }
            TerminalEvidence::Exhausted => (commands.exhausted)(fence),
        };
        let keys =
            LockKeys { command_id: command_id.as_uuid(), sellers: &sellers, equipment: &equipment };
        let input = Terminal { fence, mode, evidence, seller: identity.clone(), request_hash };
        match with_command_transaction(store, keys, &input, |input, tx| Box::pin(write(tx, input)))
            .await?
        {
            Outcome::Lost => return Ok(false),
            Outcome::Applied => return Ok(true),
            Outcome::RecheckDeadline(deadline) => evidence = TerminalEvidence::Deadline(deadline),
        }
    }
}

async fn write(tx: &mut CommandTransaction<'_>, input: &Terminal) -> Result<Outcome, WriteError> {
    let listing_id = input.fence.listing_id();
    let (actor, operation, change_operation, trigger) = match input.evidence {
        TerminalEvidence::Deadline(_) => (
            Actor::System,
            ListingOperation::AutoClose,
            ChangedOperation::AutoClose,
            ListingTrigger::AutoClose,
        ),
        TerminalEvidence::Exhausted => (
            // 现有账本约束只允许 worker/apply_lookup 写解析拒绝。
            // 使用应用层独立的回收身份，避免将回收记作任何迟到来源观察。
            Actor::Worker,
            ListingOperation::ApplyLookup,
            ChangedOperation::ApplyLookup,
            ListingTrigger::LookupRejected(ListingReason::LookupExhausted),
        ),
    };
    let Arbitration::First(mut writer) = tx
        .arbitrate(
            actor,
            operation,
            Target::Listing(listing_id),
            RequestProof::Digest(input.request_hash),
        )
        .await?
    else {
        // 不变量 1、5：竞争者已提交终结，当前调用没有回收到新的工作。
        return Ok(Outcome::Lost);
    };
    let conn = writer.connection();
    let name = input.seller.seller_game_name.as_deref();
    if lock_listing_seller(conn, input.seller.seller_user_id, name).await?.is_none() {
        return Err(WriteError::invariant("consignment_lookup_missing_seller"));
    }
    let Some(current) = lock_listings(conn, &[listing_id]).await?.pop() else {
        return Ok(Outcome::Lost);
    };
    let control = lock_control(conn).await?;
    let Some(work) = lock_reclaimable(conn, input.fence, input.mode).await? else {
        return Ok(Outcome::Lost);
    };
    let at = db_now(conn).await?;
    let (forced, expires_at) = read_resolution(conn, input.fence, at).await?;
    match terminal_evidence(work.attempt, at, forced.as_deref(), expires_at) {
        Some(evidence) if evidence == input.evidence => {}
        Some(TerminalEvidence::Deadline(deadline))
            if input.evidence == TerminalEvidence::Exhausted =>
        {
            // 等锁时跨过冻结期限，重新取得到期命令锁，避免持业务锁嵌套取得第二个命令锁。
            return Ok(Outcome::RecheckDeadline(deadline));
        }
        _ => return Ok(Outcome::Lost),
    }
    let ListingTransition::Changed { listing, change } =
        transition_listing(Some(current.snapshot), trigger, at, ForcedTerminationFacts::default())
            .map_err(invalid_projection)?
    else {
        return Err(WriteError::invariant("consignment_lookup_terminal_transition"));
    };
    // 不变量 1、2、3、5、7、8：工作、信封退休、挂单事实、回执和审计同提交。
    finish_work(conn, &control, input.fence, input.mode, work, at, WorkCompletion::Done).await?;
    sqlx::query(
        "UPDATE public.consignment_registration_secrets
         SET ciphertext = NULL, nonce = NULL, encryption_key_id = NULL, fingerprint = NULL,
             updated_at = $2, retired_at = $2 WHERE listing_id = $1 AND retired_at IS NULL",
    )
    .bind(listing_id)
    .bind(at)
    .execute(conn)
    .await?;
    match change {
        ListingChange::Expired => {
            writer.stage_system_closure(&current, listing, change, at).await?;
        }
        ListingChange::Rejected { keep_snapshot: false } => {
            writer.stage_change(&current, listing, None, change, at).await?;
        }
        _ => return Err(WriteError::invariant("consignment_lookup_terminal_change")),
    }
    // 系统回收没有 HTTP request_id；底座为该列写 NULL。
    writer.finish(Decision::Changed { listing_id, operation: change_operation }, at).await?;
    Ok(Outcome::Applied)
}
