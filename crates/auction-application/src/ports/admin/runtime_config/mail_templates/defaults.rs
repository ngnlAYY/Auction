//! 出站邮件模板的默认英文正文。

use super::AdminRuntimeMailTemplatesConfig;

impl Default for AdminRuntimeMailTemplatesConfig {
    fn default() -> Self {
        Self {
            auction_assets_rejected: concat!(
                "Your auction submission was not accepted.\n",
                "\n",
                "Auction submissions must contain equipment only.\n",
                "Please remove Credits, CoD and other item types\n",
                "before submitting again.",
            )
            .to_owned(),
            auction_no_attachments: concat!(
                "No equipment attachments were found in your message.\n",
                "\n",
                "Attach the equipment you want to submit,\n",
                "then send a new message with the subject \"auction\".",
            )
            .to_owned(),
            auction_equipment_rejected: concat!(
                "Your auction submission was not accepted.\n",
                "\n",
                "Reason:\n",
                "{reason_text}\n",
                "\n",
                "Equipment return:\n",
                "A return has been requested for all equipment\n",
                "in your original message.\n",
                "\n",
                "If you submitted multiple items together,\n",
                "the entire submission will be returned.\n",
                "\n",
                "What to do next:\n",
                "{next_steps}\n",
                "\n",
                "Original mail reference: {mail_reference}",
            )
            .to_owned(),
            user_assets_rejected: concat!(
                "Your account request was not accepted.\n",
                "\n",
                "Account requests must not include equipment,\n",
                "Credits, other items or CoD.\n",
                "\n",
                "Please resend your request without these assets.",
            )
            .to_owned(),
            register_key: concat!(
                "Your HV-Auction login key is ready.\n",
                "\n",
                "Player: {username}\n",
                "\n",
                "Login key:\n",
                "{login_key}\n",
                "\n",
                "Use this key to sign in to HV-Auction.\n",
                "Keep it private.",
            )
            .to_owned(),
            register_result: concat!(
                "Your registration could not be completed.\n",
                "\n",
                "Player: {username}\n",
                "Result code: {result_code}\n",
                "\n",
                "Contact the auction operator if you need help\n",
                "resolving this issue.",
            )
            .to_owned(),
            rebind_result: concat!(
                "Your account rebind request has been processed.\n",
                "\n",
                "Player: {username}\n",
                "Result code: {result_code}\n",
                "\n",
                "Contact the auction operator if you need help\n",
                "understanding the result.",
            )
            .to_owned(),
            rekey_key: concat!(
                "Your HV-Auction login key has been rotated.\n",
                "\n",
                "Player: {username}\n",
                "\n",
                "New login key:\n",
                "{login_key}\n",
                "\n",
                "All previous sessions have been signed out.\n",
                "Keep this key private.",
            )
            .to_owned(),
            rekey_result: concat!(
                "Your login key rotation request could not be completed.\n",
                "\n",
                "Player: {username}\n",
                "Result code: {result_code}\n",
                "\n",
                "Send a message with the subject \"register\" first\n",
                "if you have not registered yet.\n",
                "Contact the auction operator if you need help\n",
                "understanding the result.",
            )
            .to_owned(),
            settlement_winner_equipment_cod: concat!(
                "Your auction item is ready to collect.\n",
                "\n",
                "Item: {item_name}\n",
                "\n",
                "{price_breakdown}Amount due (CoD): {amount} Credits\n",
                "Payment deadline: {payment_deadline}\n",
                "\n",
                "Collect the attached equipment to pay the CoD\n",
                "and receive your item.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            settlement_payment_carrier: concat!(
                "This mail collects payment for your Isekai auction item.\n",
                "\n",
                "Item: {item_name}\n",
                "CoD: {amount} Credits\n",
                "Payment deadline: {payment_deadline}\n",
                "\n",
                "Collect the attached Credits to pay the CoD.\n",
                "Your equipment will be sent separately in Isekai\n",
                "after payment is confirmed.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            settlement_payment_reminder: concat!(
                "Payment for this auction transaction is overdue.\n",
                "\n",
                "Please check your original auction payment mail.\n",
                "If you have already paid, contact the auction\n",
                "operator with the reference below.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            settlement_equipment_handoff: concat!(
                "The following equipment is attached.\n",
                "\n",
                "Item: {item_name}\n",
                "\n",
                "Please collect the attachment.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            settlement_seller_payout: concat!(
                "Your auction payout is attached.\n",
                "\n",
                "Item: {item_name}\n",
                "Amount: {amount} Credits\n",
                "\n",
                "Please collect the attached Credits.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            settlement_buyer_refund: concat!(
                "Your auction refund is attached.\n",
                "\n",
                "Item: {item_name}\n",
                "Amount: {amount} Credits\n",
                "\n",
                "Please collect the attached Credits.\n",
                "\n",
                "Reference: {reference}",
            )
            .to_owned(),
            // 默认正文与既有冻结正文逐字一致：{deadline_line}、{lot_url_line} 缺失时展开为空行。
            lot_watch: concat!(
                "Your leading bid has been outbid.\n",
                "\n",
                "Item: {item_name}\n",
                "Lot: {lot_code}\n",
                "Accepted bid: {amount} Credits\n",
                "{deadline_line}{lot_url_line}",
                "\n",
                "Check the auction website for the latest price\n",
                "and place another bid if you wish to continue.",
            )
            .to_owned(),
            lot_watch_pm: concat!(
                "Your leading bid has been outbid.\n",
                "\n",
                "Item: {item_name}\n",
                "Lot: {lot_code}\n",
                "Accepted bid: {amount} Credits\n",
                "{deadline_line}{lot_url_line}",
                "\n",
                "Notification: {notification_id}.",
            )
            .to_owned(),
            operations_alert: concat!(
                "An auction service issue needs attention.\n",
                "\n",
                "{summary}\n",
                "\n",
                "Please review the Admin dashboard for details.",
            )
            .to_owned(),
        }
    }
}
