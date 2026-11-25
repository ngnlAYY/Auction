//! 寄售合同产生的邮件意图：七个发送模板的变量与规范主题。

mod templates;

pub use templates::{
    ConsignmentSendFacts, ConsignmentSendIntent, ConsignmentTemplateError,
    build_consignment_send_intent, consignment_send_template_kind, consignment_send_template_name,
};
