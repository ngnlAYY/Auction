//! 寄售资金端口的进程内假实现：用领域规则在内存里复现幂等、份额互斥、手续费与阻断语义。

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};

use auction_domain::SettlementWorld;
use auction_domain::consignment::funding::CodReceiptSplit;
use auction_domain::consignment::funding::{
    FeeEarned, FundingRuleError, OpeningBlock, OpeningSlice, check_opening_gate,
    opening_included_total, plan_cod_funding, refund_conversion, reverse_fee,
};
use auction_domain::consignment::{ListingPrice, ListingQuote};
use uuid::Uuid;

use super::{
    AttributionReceipt, CashReceipt, CertifyCod, CertifyReceipt, ConsignmentFundingPreflight,
    ConsignmentFundingStore, ConvertRefund, FundingPreflightFact, ReceiptKey, RefundReceipt,
    RegisterOpeningAttribution, SettleCash,
};
use crate::ports::settlement::{FundingError, FundingFuture, FundingMutation};

type Ledger<T> = HashMap<Uuid, ([u8; 32], T)>;

struct Sealed {
    attribution_id: Uuid,
    receipt: ReceiptKey,
    slices: Vec<OpeningSlice>,
}

struct Certified {
    payment_obligation_id: Uuid,
    original_delivery_id: Uuid,
    price: ListingPrice,
    quote: ListingQuote,
    receipt: CertifyReceipt,
}

#[derive(Default)]
struct State {
    attributions: Ledger<AttributionReceipt>,
    certifies: Ledger<CertifyReceipt>,
    cash: Ledger<()>,
    refunds: Ledger<RefundReceipt>,
    sealed: Vec<Sealed>,
    certified: HashMap<ReceiptKey, Certified>,
    total_issued: i64,
    earned: HashMap<Uuid, FeeEarned>,
    reversed: HashSet<Uuid>,
    blocks: Vec<OpeningBlock>,
    preflight: Option<FundingPreflightFact>,
}

/// 内存资金假实现；方法内无 await 点，future 立即就绪。
#[derive(Default)]
pub(crate) struct FakeFunding(Mutex<State>);

fn lookup<T: Clone>(
    ledger: &Ledger<T>,
    mutation: &FundingMutation,
) -> Result<Option<T>, FundingError> {
    match ledger.get(&mutation.command_id) {
        None => Ok(None),
        Some((digest, value)) if *digest == mutation.request_digest => Ok(Some(value.clone())),
        // 不变量 1：同 ID 不同请求冲突。
        Some(_) => Err(FundingError::Conflict),
    }
}

fn rule(error: FundingRuleError) -> FundingError {
    match error {
        FundingRuleError::OpeningBlocked => FundingError::Unavailable,
        FundingRuleError::FeeAlreadyReversed | FundingRuleError::PrincipalConflict => {
            FundingError::Conflict
        }
        _ => FundingError::Invalid,
    }
}

impl FakeFunding {
    fn state(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 预置预检结论。
    pub(crate) fn set_preflight(&self, fact: FundingPreflightFact) {
        self.state().preflight = Some(fact);
    }

    /// 预置一条未核清阻断；真实阻断由存储事务内原语建立（R5 第 3.8 节），这里只验证 opening 门。
    pub(crate) fn seed_block(&self, block: OpeningBlock) {
        self.state().blocks.push(block);
    }

    /// 累计新发行额。
    pub(crate) fn total_issued(&self) -> i64 {
        self.state().total_issued
    }

    /// 已写入的 earned / reversed 行数。
    pub(crate) fn fee_rows(&self) -> (usize, usize) {
        let state = self.state();
        (state.earned.len(), state.reversed.len())
    }
}

impl ConsignmentFundingPreflight for FakeFunding {
    fn preflight(&self, _world: SettlementWorld) -> FundingFuture<'_, FundingPreflightFact> {
        Box::pin(async move { self.state().preflight.clone().ok_or(FundingError::Unproven) })
    }
}

impl ConsignmentFundingStore for FakeFunding {
    fn register_opening_attribution(
        &self,
        command: RegisterOpeningAttribution,
    ) -> FundingFuture<'_, AttributionReceipt> {
        Box::pin(async move {
            let mut state = self.state();
            if let Some(done) = lookup(&state.attributions, &command.mutation)? {
                return Ok(AttributionReceipt { replayed: true, ..done });
            }
            // opening 最终写门：本 UID 有未核清退件即拒绝。
            check_opening_gate(&command.receipt.uid, &state.blocks).map_err(rule)?;
            let included = opening_included_total(&command.slices).map_err(rule)?;
            for sealed in &state.sealed {
                let overlaps = sealed.slices.iter().any(|old| {
                    command.slices.iter().any(|new| {
                        old.entry_id == new.entry_id && new.start < old.end && old.start < new.end
                    })
                });
                if sealed.receipt == command.receipt {
                    if sealed.slices == command.slices {
                        return Ok(AttributionReceipt {
                            attribution_id: sealed.attribution_id,
                            included,
                            replayed: true,
                        });
                    }
                    return Err(FundingError::Conflict);
                }
                // 同一份额只能绑定一个实收业务键。
                if overlaps {
                    return Err(FundingError::Conflict);
                }
            }
            let attribution_id =
                Uuid::new_v5(&Uuid::NAMESPACE_OID, &command.mutation.command_id.into_bytes());
            let receipt = AttributionReceipt { attribution_id, included, replayed: false };
            state.sealed.push(Sealed {
                attribution_id,
                receipt: command.receipt,
                slices: command.slices,
            });
            state
                .attributions
                .insert(command.mutation.command_id, (command.mutation.request_digest, receipt));
            Ok(receipt)
        })
    }

    fn certify_cod(&self, command: CertifyCod) -> FundingFuture<'_, CertifyReceipt> {
        Box::pin(async move {
            command.check_leg()?;
            command.check_cash()?;
            let mut state = self.state();
            if let Some(done) = lookup(&state.certifies, &command.mutation)? {
                return Ok(CertifyReceipt { replayed: true, ..done });
            }
            // 已封存的 opening 份额必须由本实收业务键显式声明；省略会把已含金额当未含而重复发行。
            let sealed = state.sealed.iter().find(|sealed| sealed.receipt == command.receipt);
            let included = match (command.attribution_id, sealed) {
                (None, None) => 0,
                (None, Some(_)) => return Err(FundingError::Conflict),
                (Some(id), Some(sealed)) if sealed.attribution_id == id => {
                    opening_included_total(&sealed.slices).map_err(rule)?
                }
                _ => return Err(FundingError::Invalid),
            };
            // 同一经济实收换命令 ID 复见：相容者读原结果，异合同、异额或异 delivery 冲突，零新增
            // 发行。查找闭集与存储一致：收据键、付款义务或原 delivery 任一命中都只能有一份结果。
            let existing = state.certified.iter().find(|(key, old)| {
                **key == command.receipt
                    || old.payment_obligation_id == command.payment_obligation_id
                    || old.original_delivery_id == command.original_delivery_id
            });
            if let Some((_, old)) = existing {
                let same = old.payment_obligation_id == command.payment_obligation_id
                    && old.original_delivery_id == command.original_delivery_id
                    && old.price == command.price
                    && old.quote == command.quote
                    && old.receipt.contract_id == command.contract_id
                    && old.receipt.split.gross() == command.gross
                    && old.receipt.split.opening_included() == included;
                return if same {
                    Ok(CertifyReceipt { replayed: true, ..old.receipt.clone() })
                } else {
                    Err(FundingError::Conflict)
                };
            }
            let split = CodReceiptSplit::new(command.gross, included).map_err(rule)?;
            let plan =
                plan_cod_funding(command.world, command.mode, command.price, command.quote, split)
                    .map_err(rule)?;
            let receipt = CertifyReceipt {
                contract_id: command.contract_id,
                receipt_source_id: Uuid::new_v5(
                    &Uuid::NAMESPACE_OID,
                    &command.payment_obligation_id.into_bytes(),
                ),
                split,
                plan,
                replayed: false,
            };
            state.total_issued += split.new_issue();
            if command.quote.fee > 0 {
                state.earned.insert(
                    command.contract_id,
                    FeeEarned {
                        contract_id: command.contract_id,
                        receipt_source_id: receipt.receipt_source_id,
                        amount: command.quote.fee,
                    },
                );
            }
            state.certified.insert(
                command.receipt,
                Certified {
                    payment_obligation_id: command.payment_obligation_id,
                    original_delivery_id: command.original_delivery_id,
                    price: command.price,
                    quote: command.quote,
                    receipt: receipt.clone(),
                },
            );
            state.certifies.insert(
                command.mutation.command_id,
                (command.mutation.request_digest, receipt.clone()),
            );
            Ok(receipt)
        })
    }

    fn settle_cash(&self, command: SettleCash) -> FundingFuture<'_, CashReceipt> {
        Box::pin(async move {
            let mut state = self.state();
            if lookup(&state.cash, &command.mutation)?.is_some() {
                return Ok(CashReceipt { replayed: true });
            }
            if command.amount <= 0 {
                return Err(FundingError::Invalid);
            }
            state.cash.insert(command.mutation.command_id, (command.mutation.request_digest, ()));
            Ok(CashReceipt { replayed: false })
        })
    }

    fn convert_refund(&self, command: ConvertRefund) -> FundingFuture<'_, RefundReceipt> {
        Box::pin(async move {
            let mut state = self.state();
            if let Some(done) = lookup(&state.refunds, &command.mutation)? {
                return Ok(RefundReceipt { replayed: true, ..done });
            }
            // 先校验报价守恒，失败时不留下任何冲回痕迹。
            let conversion = refund_conversion(command.price, command.quote).map_err(rule)?;
            let earned = state.earned.get(&command.contract_id).copied();
            let already = state.reversed.contains(&command.contract_id);
            let fee_reversal = reverse_fee(
                command.contract_id,
                command.quote,
                earned,
                already,
                command.refund_obligation_id,
            )
            .map_err(rule)?;
            if fee_reversal.is_some() {
                state.reversed.insert(command.contract_id);
            }
            let receipt = RefundReceipt { conversion, fee_reversal, replayed: false };
            state.refunds.insert(
                command.mutation.command_id,
                (command.mutation.request_digest, receipt.clone()),
            );
            Ok(receipt)
        })
    }
}
