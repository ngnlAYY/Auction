//! 权利通知的装配边界：解封挑战码并向冻结身份发信。
//!
//! 不变量 4、7：解封只发生在发信前的这一处，明文码只存在于本次调用的栈与待发正文里，不进日志、
//! 错误或回执；收件人只取领取时带出的冻结身份（`ClaimedRightNotice::recipient`），本文件不读取
//! 任何“当前名字”。发信失败只返回结论，不重试、不重发；是否未知由发信传输按“写入是否已开始”判定。

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use auction_application::consignment::{ClaimedRightNotice, RightChallengeCrypto};
use auction_domain::SettlementWorld;
use auction_domain::consignment::rights::format_challenge_code;
use auction_integrations::mooglemail::{
    MoogleMailDeliveryTransport, MoogleMailWorld, PlainNoticeMail, PlainNoticeOutcome,
};
use auction_jobs::consignment::{NoticeDelivery, RightNoticeDeliverer};

/// 一封待发的权利通知邮件；`Debug` 不输出收件人与正文。
pub struct RightNoticeMail {
    /// 发信世界。
    pub world: SettlementWorld,
    /// 收件人：冻结身份。
    pub recipient: String,
    /// 主题。
    pub subject: String,
    /// 正文，含挑战码。
    pub body: String,
}

impl fmt::Debug for RightNoticeMail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RightNoticeMail")
            .field("world", &self.world)
            .finish_non_exhaustive()
    }
}

/// 发信传输：把一封纯文本邮件写给指定收件人，返回是否发出。
///
/// 实现须在“写入已开始但结果无法证明”时返回 `Unknown`，且自身不得重试。
pub trait RightNoticeMailTransport: Send + Sync {
    /// 发送一封邮件。
    fn send<'a>(
        &'a self,
        mail: RightNoticeMail,
    ) -> Pin<Box<dyn Future<Output = NoticeDelivery> + Send + 'a>>;
}

/// 经 MoogleMail delivery 同一路径（独占会话、写前 Sent 基线、发后证明）发送纯文本通知。
///
/// 与 delivery 共用同一 transport 实例，凭据独占锁与发送器状态只有一份；结论原样投影，
/// “写入已开始但无法证明”由传输判为 `Unknown`，这里不重试。
pub struct MoogleMailRightNoticeTransport {
    transport: Arc<MoogleMailDeliveryTransport>,
}

impl MoogleMailRightNoticeTransport {
    /// 以 Worker 已装配的 delivery transport 建立。
    pub fn new(transport: Arc<MoogleMailDeliveryTransport>) -> Self {
        Self { transport }
    }
}

impl RightNoticeMailTransport for MoogleMailRightNoticeTransport {
    fn send<'a>(
        &'a self,
        mail: RightNoticeMail,
    ) -> Pin<Box<dyn Future<Output = NoticeDelivery> + Send + 'a>> {
        Box::pin(async move {
            let outcome = self
                .transport
                .send_plain_notice(PlainNoticeMail {
                    world: MoogleMailWorld::from(mail.world),
                    recipient: mail.recipient,
                    subject: mail.subject,
                    body: mail.body,
                })
                .await;
            match outcome {
                PlainNoticeOutcome::Sent => NoticeDelivery::Sent,
                PlainNoticeOutcome::Unknown => NoticeDelivery::Unknown,
                PlainNoticeOutcome::NotSent { retryable } => NoticeDelivery::NotSent { retryable },
            }
        })
    }
}

/// 解封并发信的投递器。
pub struct SealedNoticeDeliverer {
    crypto: Arc<dyn RightChallengeCrypto>,
    transport: Arc<dyn RightNoticeMailTransport>,
}

impl SealedNoticeDeliverer {
    /// 以挑战码密码端口与发信传输建立。
    pub fn new(
        crypto: Arc<dyn RightChallengeCrypto>,
        transport: Arc<dyn RightNoticeMailTransport>,
    ) -> Self {
        Self { crypto, transport }
    }
}

impl RightNoticeDeliverer for SealedNoticeDeliverer {
    fn deliver<'a>(
        &'a self,
        notice: &'a ClaimedRightNotice,
    ) -> Pin<Box<dyn Future<Output = NoticeDelivery> + Send + 'a>> {
        Box::pin(async move {
            // 解封失败（密钥已移除或 AAD 不符）永远不会好转：明确未发出，不重试。
            let Ok(code) = self.crypto.open(&notice.secret.sealed, notice.secret.aad) else {
                tracing::warn!(kind = "right_notice", "权利通知挑战码无法解封，不发送");
                return NoticeDelivery::NotSent { retryable: false };
            };
            let shown = format_challenge_code(&code).display();
            // 暂定文案：正文措辞与多语言由 7E 与文档定稿（见 P7D5 报告“与合同的出入”）。
            let mail = RightNoticeMail {
                world: notice.world,
                recipient: notice.recipient.clone(),
                subject: "HV Auction consignment verification code".to_owned(),
                body: format!(
                    "Your consignment verification code is {shown}. It expires in 10 minutes. \
                     If you did not request it, ignore this message."
                ),
            };
            self.transport.send(mail).await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use auction_application::consignment::{
        ChallengeAad, ClaimedRightNotice, RightChallengeCrypto, RightCryptoError, RightNoticeKind,
        RightNoticeLease, RightNoticeSecret, SealedChallengeSecret,
    };
    use auction_domain::SettlementWorld;
    use auction_domain::consignment::rights::{ChallengeCode, format_challenge_code};
    use auction_jobs::consignment::{NoticeDelivery, RightNoticeDeliverer};
    use uuid::Uuid;

    use super::{
        Arc, Future, Pin, RightNoticeMail, RightNoticeMailTransport, SealedNoticeDeliverer,
    };

    /// 假密码端口：密钥标识为 `good` 时解出固定码，否则失败。
    struct FakeCrypto;

    impl RightChallengeCrypto for FakeCrypto {
        fn seal(
            &self,
            _: &ChallengeCode,
            _: ChallengeAad,
        ) -> Result<SealedChallengeSecret, RightCryptoError> {
            Err(RightCryptoError)
        }

        fn open(
            &self,
            sealed: &SealedChallengeSecret,
            _: ChallengeAad,
        ) -> Result<ChallengeCode, RightCryptoError> {
            if sealed.encryption_key_id() == "good" {
                Ok(ChallengeCode::from_entropy([7; 16]))
            } else {
                Err(RightCryptoError)
            }
        }

        fn mac(&self, _: ChallengeAad, _: &ChallengeCode) -> Result<[u8; 32], RightCryptoError> {
            Err(RightCryptoError)
        }
    }

    #[derive(Default)]
    struct RecordingTransport {
        sent: Mutex<Vec<(String, String)>>,
    }

    impl RightNoticeMailTransport for RecordingTransport {
        fn send<'a>(
            &'a self,
            mail: RightNoticeMail,
        ) -> Pin<Box<dyn Future<Output = NoticeDelivery> + Send + 'a>> {
            self.sent.lock().expect("锁").push((mail.recipient, mail.body));
            Box::pin(async { NoticeDelivery::Sent })
        }
    }

    fn notice(key_id: &str) -> ClaimedRightNotice {
        let sealed =
            SealedChallengeSecret::new(vec![1], [0; 12], key_id.to_owned(), "0".repeat(64))
                .expect("夹具密文形状有效");
        ClaimedRightNotice {
            lease: RightNoticeLease {
                work_id: Uuid::nil(),
                claim_token: Uuid::nil(),
                claim_fence: 1,
            },
            kind: RightNoticeKind::Challenge,
            world: SettlementWorld::Main,
            recipient: "FrozenFakeName".to_owned(),
            secret: RightNoticeSecret {
                aad: ChallengeAad {
                    challenge_id: Uuid::nil(),
                    right_id: None,
                    identity_digest: [0; 32],
                    right_version: 1,
                },
                sealed,
            },
            reclaimed: false,
        }
    }

    #[tokio::test]
    async fn opened_code_is_sent_only_to_the_frozen_recipient() {
        let transport = Arc::new(RecordingTransport::default());
        let deliverer = SealedNoticeDeliverer::new(Arc::new(FakeCrypto), transport.clone());
        let outcome = deliverer.deliver(&notice("good")).await;
        assert_eq!(outcome, NoticeDelivery::Sent);
        let sent = transport.sent.lock().expect("锁");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "FrozenFakeName");
        let shown = format_challenge_code(&ChallengeCode::from_entropy([7; 16])).display();
        assert!(sent[0].1.contains(&shown));
    }

    #[tokio::test]
    async fn unopenable_secret_sends_nothing() {
        let transport = Arc::new(RecordingTransport::default());
        let deliverer = SealedNoticeDeliverer::new(Arc::new(FakeCrypto), transport.clone());
        let outcome = deliverer.deliver(&notice("rotated-away")).await;
        assert_eq!(outcome, NoticeDelivery::NotSent { retryable: false });
        assert!(transport.sent.lock().expect("锁").is_empty());
    }
}
