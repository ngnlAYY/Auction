//! 派发用例的核心行为：许可只发一次、命令 ID 与摘要的稳定性。

use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

use auction_domain::CommandId;
use auction_domain::consignment::dispatch::AttemptKey;
use uuid::Uuid;

use super::DispatchService;
use super::encoding;
use crate::ports::consignment::{
    AttemptReceipt, AuthorizeDispatch, ConsignmentError, ConsignmentFuture, DispatchStore,
    EvidenceReceipt, MarkWriteStarted, RecordEvidence, RoundReceipt, StartAttempt, StartReceipt,
    WriteStart, WriteStartOutcome,
};
use crate::ports::delivery::DeliverySendCredentialBinding;

fn run<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..4 {
        if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
            return result;
        }
    }
    panic!("测试存储未在预期轮数内完成")
}

/// 按脚本依次返回起写结果的存储；其余端口方法不会被这些测试调用。
struct ScriptedStore {
    outcomes: Mutex<Vec<WriteStartOutcome>>,
}

impl DispatchStore for ScriptedStore {
    fn authorize_dispatch(&self, _: AuthorizeDispatch) -> ConsignmentFuture<'_, RoundReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn start_attempt(&self, _: StartAttempt) -> ConsignmentFuture<'_, AttemptReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn mark_write_started(&self, _: MarkWriteStarted) -> ConsignmentFuture<'_, WriteStartOutcome> {
        Box::pin(async move {
            let Ok(mut outcomes) = self.outcomes.lock() else {
                return Err(ConsignmentError::Unavailable);
            };
            if outcomes.is_empty() {
                return Err(ConsignmentError::Unavailable);
            }
            Ok(outcomes.remove(0))
        })
    }

    fn record_evidence(&self, _: RecordEvidence) -> ConsignmentFuture<'_, EvidenceReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }
}

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn receipt(delivery: Uuid, fence: u64) -> StartReceipt {
    StartReceipt::started(CommandId::from_uuid(id(1)), id(2), delivery, id(3), fence)
}

fn service(outcomes: Vec<WriteStartOutcome>) -> DispatchService {
    DispatchService::new(Arc::new(ScriptedStore { outcomes: Mutex::new(outcomes) }))
}

#[test]
fn only_the_first_committed_write_start_carries_a_post_permit() {
    let attempt = AttemptKey { delivery_id: id(50), fence: 1 };
    let service = service(vec![
        WriteStartOutcome::FirstCommitted(receipt(id(50), 1)),
        WriteStartOutcome::AlreadyCommitted(receipt(id(50), 1)),
    ]);
    // 正例：首次提交携带绑定该投递与 fence 的许可。
    let first = run(service.mark_write_started(attempt, id(60), None)).expect("首次起写");
    let WriteStart::FirstCommitted { permit, .. } = first else {
        panic!("首次提交必须带许可");
    };
    assert_eq!((permit.delivery_id(), permit.fence()), (id(50), 1));
    // 反例：已提交的重放与新 fence 都只得到“已提交”，没有许可可取。
    let again = run(service.mark_write_started(attempt, id(60), None)).expect("重放起写");
    assert!(matches!(again, WriteStart::AlreadyCommitted(_)));
}

#[test]
fn a_first_commit_receipt_for_another_delivery_or_fence_yields_no_permit() {
    let attempt = AttemptKey { delivery_id: id(50), fence: 2 };
    for wrong in [receipt(id(51), 2), receipt(id(50), 1)] {
        let service = service(vec![WriteStartOutcome::FirstCommitted(wrong)]);
        assert_eq!(
            run(service.mark_write_started(attempt, id(60), None)).expect_err("回执对不上请求"),
            ConsignmentError::Unavailable
        );
    }
}

#[test]
fn write_start_command_identity_follows_the_fence_and_the_digest_follows_the_claim() {
    let first = AttemptKey { delivery_id: id(50), fence: 1 };
    let second = AttemptKey { delivery_id: id(50), fence: 2 };
    let (first_id, first_hash) = encoding::mark_write_started(&first, id(60), None);
    let (second_id, _) = encoding::mark_write_started(&second, id(60), None);
    // 新 fence 是新的命令 ID；同 fence 换 claim token 保持命令 ID、改变摘要（账本判冲突）。
    assert_ne!(first_id, second_id);
    let (same_id, other_hash) = encoding::mark_write_started(&first, id(61), None);
    assert_eq!(same_id, first_id);
    assert_ne!(other_hash, first_hash);
    // 黄金向量：命令 ID 的字节一经发布不得再改。
    assert_eq!(first_id.as_uuid().to_string(), "90ed1540-3c29-5ce6-ab8e-89b49b844420");
}

#[test]
fn write_start_digest_binds_the_credential_but_keeps_the_command_identity() {
    let attempt = AttemptKey { delivery_id: id(50), fence: 1 };
    let binding = |version: i64| DeliverySendCredentialBinding {
        credential_id: id(70),
        credential_version: version,
        runtime_generation: 1,
    };
    let (none_id, none_hash) = encoding::mark_write_started(&attempt, id(60), None);
    let (some_id, some_hash) = encoding::mark_write_started(&attempt, id(60), Some(&binding(1)));
    let (_, other_hash) = encoding::mark_write_started(&attempt, id(60), Some(&binding(2)));
    // 同一尝试同一命令 ID；换凭据或版本改变摘要，账本判冲突，不会悄悄换凭据重放。
    assert_eq!(none_id, some_id);
    assert_ne!(none_hash, some_hash);
    assert_ne!(some_hash, other_hash);
}
