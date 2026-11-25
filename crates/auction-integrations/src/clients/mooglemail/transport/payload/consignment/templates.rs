//! 七个寄售发送模板的冻结正文。
//!
//! 字节一经发布不可改动：投递请求摘要与系统退件的正文摘要都基于这些字节，黄金向量见
//! `vectors_test.rs`。末尾固定为合同行、义务引用行与独立的轮引用行。

use auction_application::settlement::ConsignmentSubjectKind;
use auction_domain::DISPATCH_REFERENCE_LINE_PREFIX;

use super::Variables;

/// 渲染正文；变量已由调用方校验，该种类需要的金额与期限必然存在。
pub(super) fn render_body(kind: ConsignmentSubjectKind, variables: &Variables<'_>) -> String {
    let item = variables.item_name;
    let amount = variables.amount.as_deref().unwrap_or_default();
    let deadline = variables.payment_deadline.unwrap_or_default();
    let lead = match kind {
        ConsignmentSubjectKind::EquipmentCod => format!(
            concat!(
                "Your consignment purchase is ready to collect.\n",
                "\n",
                "Item: {item}\n",
                "\n",
                "Amount due (CoD): {amount} Credits\n",
                "Payment deadline: {deadline}\n",
                "\n",
                "Collect the attached equipment to pay the CoD\n",
                "and receive your item.",
            ),
            item = item,
            amount = amount,
            deadline = deadline,
        ),
        ConsignmentSubjectKind::PaymentCarrier => format!(
            concat!(
                "This mail collects payment for your Isekai consignment purchase.\n",
                "\n",
                "Item: {item}\n",
                "CoD: {amount} Credits\n",
                "Payment deadline: {deadline}\n",
                "\n",
                "Collect the attached Credits to pay the CoD.\n",
                "Your equipment will be sent separately in Isekai\n",
                "after payment is confirmed.",
            ),
            item = item,
            amount = amount,
            deadline = deadline,
        ),
        ConsignmentSubjectKind::EquipmentHandoff => format!(
            concat!(
                "The following consignment equipment is attached.\n",
                "\n",
                "Item: {item}\n",
                "\n",
                "Please collect the attachment.",
            ),
            item = item,
        ),
        ConsignmentSubjectKind::SellerPayout => format!(
            concat!(
                "Your consignment payout is attached.\n",
                "\n",
                "Item: {item}\n",
                "Amount: {amount} Credits\n",
                "\n",
                "Please collect the attached Credits.",
            ),
            item = item,
            amount = amount,
        ),
        ConsignmentSubjectKind::BuyerRefund => format!(
            concat!(
                "Your consignment refund is attached.\n",
                "\n",
                "Item: {item}\n",
                "Amount: {amount} Credits\n",
                "\n",
                "Please collect the attached Credits.",
            ),
            item = item,
            amount = amount,
        ),
        ConsignmentSubjectKind::EquipmentWithdrawal => format!(
            concat!(
                "Your withdrawn consignment equipment is attached.\n",
                "\n",
                "Item: {item}\n",
                "\n",
                "Please collect the attachment.",
            ),
            item = item,
        ),
        ConsignmentSubjectKind::PaymentReminder => format!(
            concat!(
                "Payment for this consignment purchase is overdue.\n",
                "\n",
                "Item: {item}\n",
                "Amount due (CoD): {amount} Credits\n",
                "Payment deadline: {deadline}\n",
                "\n",
                "Please check your original consignment payment mail.\n",
                "If you have already paid, contact the auction\n",
                "operator with the references below.",
            ),
            item = item,
            amount = amount,
            deadline = deadline,
        ),
    };
    format!(
        "{lead}\n\nContract: {contract}\nReference: {reference}\n\n{prefix}{dispatch_ref}",
        contract = variables.contract,
        reference = variables.reference,
        prefix = DISPATCH_REFERENCE_LINE_PREFIX,
        dispatch_ref = variables.dispatch_ref,
    )
}
