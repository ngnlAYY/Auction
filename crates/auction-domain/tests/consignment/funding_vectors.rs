//! 寄售资金规则黄金向量：分账取整守恒、COD opening 份额、现金退件阻断、手续费冲回只冲本笔。
//!
//! v5 字节与金额一旦冻结不得再改；只使用公开领域 API。

// 集成测试目标只含测试代码，断言直接解包。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use auction_domain::SettlementWorld;
use auction_domain::consignment::funding::{
    BlockClearanceEvidence, BlockedLeg, CodReceiptSplit, FeeEarned, FundingMode, FundingRuleError,
    OpeningBlock, OpeningSlice, PrincipalConclusion, TransitOutput, buyer_refund_liability,
    check_opening_gate, expected_gross, fee_rows, may_clear_block, opening_included_total,
    plan_cod_funding, refund_conversion, resolve_principal_conclusion, reverse_fee,
};
use auction_domain::consignment::{FeeBps, ListingPrice, ListingQuote, quote_v1};
use uuid::Uuid;

fn price(credits: i64) -> ListingPrice {
    ListingPrice::new(credits).unwrap()
}

fn id(byte: u8) -> Uuid {
    Uuid::from_bytes([byte; 16])
}

#[test]
fn split_conserves_credits_for_every_price_and_fee() {
    for world in [SettlementWorld::Main, SettlementWorld::Isekai] {
        for credits in [1, 19, 101, 999_999_999, 1_000_000_000] {
            for bps in [0, 1, 500, 9_999] {
                let price = price(credits);
                let quote = quote_v1(price, FeeBps::new(bps).unwrap());
                let gross = expected_gross(world, credits);
                for included in [0, gross / 2, gross] {
                    let split = CodReceiptSplit::new(gross, included).unwrap();
                    assert_eq!(split.opening_included() + split.new_issue(), gross);
                    let plan =
                        plan_cod_funding(world, FundingMode::StrictCash, price, quote, split)
                            .unwrap();
                    assert_eq!(plan.opening_borrow + plan.new_issue, gross);
                    assert_eq!(plan.outputs_total(), gross);
                }
            }
        }
    }
}

#[test]
fn split_rejects_out_of_range_and_unbalanced_inputs() {
    assert_eq!(CodReceiptSplit::new(102, 103), Err(FundingRuleError::OpeningExceedsGross));
    assert_eq!(CodReceiptSplit::new(102, -1), Err(FundingRuleError::InvalidAmount));
    assert_eq!(CodReceiptSplit::new(0, 0), Err(FundingRuleError::InvalidAmount));

    let p = price(101);
    let quote = quote_v1(p, FeeBps::DEFAULT_CONSIGNED);
    // isekai 毛额必须是 P+1；只收到 P 不是同一笔经济实收。
    let short = CodReceiptSplit::new(101, 0).unwrap();
    assert_eq!(
        plan_cod_funding(SettlementWorld::Isekai, FundingMode::StrictCash, p, quote, short),
        Err(FundingRuleError::GrossMismatch)
    );
    // 被篡改的报价（S+F 不等于 P）不得进入计划。
    let forged = auction_domain::consignment::ListingQuote { fee: 5, seller_amount: 97 };
    let split = CodReceiptSplit::new(102, 0).unwrap();
    assert_eq!(
        plan_cod_funding(SettlementWorld::Isekai, FundingMode::StrictCash, p, forged, split),
        Err(FundingRuleError::QuoteMismatch)
    );
}

#[test]
fn cod_opening_share_golden_102_60_0() {
    let p = price(101);
    let quote = quote_v1(p, FeeBps::DEFAULT_CONSIGNED);
    assert_eq!((quote.fee, quote.seller_amount), (5, 96));
    assert_eq!(expected_gross(SettlementWorld::Isekai, 101), 102);
    for (included, issue) in [(102, 0), (60, 42), (0, 102)] {
        let split = CodReceiptSplit::new(102, included).unwrap();
        assert_eq!(split.new_issue(), issue);
        let plan =
            plan_cod_funding(SettlementWorld::Isekai, FundingMode::StrictCash, p, quote, split)
                .unwrap();
        assert_eq!((plan.opening_borrow, plan.new_issue), (included, issue));
        assert_eq!(
            plan.outputs,
            vec![
                TransitOutput::Seller(96),
                TransitOutput::Fee(5),
                TransitOutput::Principal { amount: 1, mode: FundingMode::StrictCash },
            ]
        );
    }
}

#[test]
fn strict_cash_balance_chain_10_9_111_10() {
    let p = price(101);
    let quote = quote_v1(p, FeeBps::DEFAULT_CONSIGNED);
    let plan = plan_cod_funding(
        SettlementWorld::Isekai,
        FundingMode::StrictCash,
        p,
        quote,
        CodReceiptSplit::new(102, 0).unwrap(),
    )
    .unwrap();
    let own = 10;
    let carrier = match plan.outputs[2] {
        TransitOutput::Principal { amount, .. } => amount,
        other => panic!("缺少本金项: {other:?}"),
    };
    let own_after_carrier = own - carrier;
    assert_eq!(own_after_carrier, 9);
    // 买家实付 102 后自有 = 9 + 102 = 10 + 96 + 5。
    assert_eq!(own_after_carrier + plan.new_issue, 111);
    assert_eq!(111, own + 96 + 5);
    // 未付载体 RTS 精确 Take 1 后恢复 10。
    assert_eq!(own_after_carrier + carrier, 10);
}

#[test]
fn zero_fee_has_no_fee_output_or_rows() {
    let p = price(19);
    let quote = quote_v1(p, FeeBps::new(0).unwrap());
    let plan = plan_cod_funding(
        SettlementWorld::Main,
        FundingMode::RecordOnly,
        p,
        quote,
        CodReceiptSplit::new(19, 0).unwrap(),
    )
    .unwrap();
    assert_eq!(plan.outputs, vec![TransitOutput::Seller(19)]);
    assert!(!fee_rows(quote).earned && !fee_rows(quote).reverse_on_full_refund);
    let conversion = refund_conversion(p, quote).unwrap();
    assert_eq!((conversion.from_seller, conversion.from_fee, conversion.to_refund), (19, 0, 19));
}

#[test]
fn refund_conversion_equals_price_and_liability_is_price_not_sum() {
    let p = price(101);
    let quote = quote_v1(p, FeeBps::DEFAULT_CONSIGNED);
    let conversion = refund_conversion(p, quote).unwrap();
    assert_eq!(conversion.from_seller + conversion.from_fee, conversion.to_refund);
    assert_eq!(conversion.to_refund, 101);
    assert_eq!(buyer_refund_liability(p), 101);
}

#[test]
fn opening_slices_must_not_overlap_within_an_entry() {
    let entry = id(1);
    let slices = [
        OpeningSlice { entry_id: entry, start: 0, end: 40 },
        OpeningSlice { entry_id: entry, start: 40, end: 60 },
        OpeningSlice { entry_id: id(2), start: 0, end: 10 },
    ];
    assert_eq!(opening_included_total(&slices), Ok(70));
    let overlap = [slices[0], OpeningSlice { entry_id: entry, start: 39, end: 60 }];
    assert_eq!(opening_included_total(&overlap), Err(FundingRuleError::SliceOverlap));
    let empty = [OpeningSlice { entry_id: entry, start: 5, end: 5 }];
    assert_eq!(opening_included_total(&empty), Err(FundingRuleError::InvalidAmount));
}

#[test]
fn principal_conclusions_are_mutually_exclusive() {
    use PrincipalConclusion::{PaymentPrincipal, RtsPrincipal};
    assert_eq!(resolve_principal_conclusion(None, PaymentPrincipal), Ok(true));
    assert_eq!(resolve_principal_conclusion(Some(RtsPrincipal), RtsPrincipal), Ok(false));
    assert_eq!(
        resolve_principal_conclusion(Some(PaymentPrincipal), RtsPrincipal),
        Err(FundingRuleError::PrincipalConflict)
    );
}

fn block(uid: &str, leg: BlockedLeg) -> OpeningBlock {
    OpeningBlock::new(uid.to_owned(), id(7), leg, id(8), 1, FundingMode::StrictCash).unwrap()
}

#[test]
fn cash_return_blocks_opening_of_same_uid_only() {
    for leg in [BlockedLeg::Refund, BlockedLeg::Payout, BlockedLeg::CarrierPrincipal] {
        let blocks = [block("uid-a", leg)];
        assert_eq!(check_opening_gate("uid-a", &blocks), Err(FundingRuleError::OpeningBlocked));
        // A 的阻断只归 A，B 的 opening 不受它影响。
        assert_eq!(check_opening_gate("uid-b", &blocks), Ok(()));
    }
    assert_eq!(check_opening_gate("uid-a", &[]), Ok(()));
    assert_eq!(
        OpeningBlock::new(
            String::new(),
            id(7),
            BlockedLeg::Refund,
            id(8),
            1,
            FundingMode::StrictCash
        ),
        Err(FundingRuleError::InvalidBlock)
    );
}

#[test]
fn block_clears_only_with_full_evidence() {
    let full = BlockClearanceEvidence {
        exact_take_proven: true,
        funding_consumed: true,
        returned_delivery_recorded: true,
        purpose_restored: true,
        principal_settled: true,
        take_disproved_and_permit_sealed: false,
    };
    assert!(may_clear_block(BlockedLeg::CarrierPrincipal, &full));
    // 只有 Take 证明不足。
    let take_only = BlockClearanceEvidence { exact_take_proven: true, ..Default::default() };
    assert!(!may_clear_block(BlockedLeg::Refund, &take_only));
    // 其余证据齐全但没有精确 Take 证明，仍不得解除。
    let no_take = BlockClearanceEvidence { exact_take_proven: false, ..full };
    assert!(!may_clear_block(BlockedLeg::Refund, &no_take));
    // 资金未消费、未记 returned_delivery、未恢复原用途，各自缺一都不得解除。
    for broken in [
        BlockClearanceEvidence { funding_consumed: false, ..full },
        BlockClearanceEvidence { returned_delivery_recorded: false, ..full },
        BlockClearanceEvidence { purpose_restored: false, ..full },
    ] {
        assert!(!may_clear_block(BlockedLeg::Refund, &broken));
        assert!(!may_clear_block(BlockedLeg::CarrierPrincipal, &broken));
    }
    // 载体本金腿缺本金核销不得解除，其他腿不要求。
    let no_principal = BlockClearanceEvidence { principal_settled: false, ..full };
    assert!(!may_clear_block(BlockedLeg::CarrierPrincipal, &no_principal));
    assert!(may_clear_block(BlockedLeg::Payout, &no_principal));
    // 确定未 Take 但许可未封闭仍不解除。
    assert!(!may_clear_block(BlockedLeg::Refund, &BlockClearanceEvidence::default()));
    let disproved =
        BlockClearanceEvidence { take_disproved_and_permit_sealed: true, ..Default::default() };
    assert!(may_clear_block(BlockedLeg::Refund, &disproved));
}

#[test]
fn fee_reversal_reverses_only_this_contract_once() {
    let p = price(101);
    let quote = quote_v1(p, FeeBps::DEFAULT_CONSIGNED);
    let (contract, other, receipt, refund) = (id(1), id(2), id(3), id(4));
    let earned = FeeEarned { contract_id: contract, receipt_source_id: receipt, amount: 5 };

    let reversal = reverse_fee(contract, quote, Some(earned), false, refund).unwrap().unwrap();
    assert_eq!(
        (
            reversal.contract_id,
            reversal.receipt_source_id,
            reversal.refund_obligation_id,
            reversal.amount
        ),
        (contract, receipt, refund, 5)
    );
    // 同额、同价位的另一合同 earned 不能被借用。
    let foreign = FeeEarned { contract_id: other, ..earned };
    assert_eq!(
        reverse_fee(contract, quote, Some(foreign), false, refund),
        Err(FundingRuleError::FeeBinding)
    );
    // 金额与冻结 F 不符不得冲回。
    let wrong_amount = FeeEarned { amount: 4, ..earned };
    assert_eq!(
        reverse_fee(contract, quote, Some(wrong_amount), false, refund),
        Err(FundingRuleError::FeeBinding)
    );
    // 已冲回不得二次冲回；缺 earned 不得凭空冲回。
    assert_eq!(
        reverse_fee(contract, quote, Some(earned), true, refund),
        Err(FundingRuleError::FeeAlreadyReversed)
    );
    assert_eq!(
        reverse_fee(contract, quote, None, false, refund),
        Err(FundingRuleError::FeeBinding)
    );
    // F=0：无 earned、无 reversed；有残留 earned 属账实不符。
    let free = quote_v1(price(19), FeeBps::new(0).unwrap());
    assert_eq!(reverse_fee(contract, free, None, false, refund), Ok(None));
    assert_eq!(
        reverse_fee(contract, free, Some(earned), false, refund),
        Err(FundingRuleError::FeeBinding)
    );
}

#[test]
fn refund_conversion_rejects_unbalanced_quote() {
    let p = price(101);
    let good = quote_v1(p, FeeBps::DEFAULT_CONSIGNED);
    for bad in [
        ListingQuote { fee: 5, seller_amount: 97 },
        ListingQuote { fee: good.fee, seller_amount: good.seller_amount - 1 },
        ListingQuote { fee: -1, seller_amount: 102 },
        ListingQuote { fee: 101, seller_amount: 0 },
    ] {
        assert_eq!(refund_conversion(p, bad), Err(FundingRuleError::QuoteMismatch));
    }
}

#[test]
fn split_new_issue_always_equals_gross_minus_opening() {
    for (gross, included) in [(1, 0), (1, 1), (102, 0), (102, 60), (102, 102)] {
        let split = CodReceiptSplit::new(gross, included).unwrap();
        assert_eq!(split.new_issue(), gross - included);
        assert_eq!(split.opening_included() + split.new_issue(), split.gross());
    }
}
