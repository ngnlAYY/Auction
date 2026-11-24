//! 寄售观察的摘要、冻结清单与领域投影；未判标记未清时不得冻结或授予处分权。

use std::collections::HashSet;

use auction_application::settlement::{
    parse_returned_settlement_body, system_returned_settlement_obligation_id,
};
use auction_domain::consignment::{
    EnvelopeItemKind, EnvelopeItemObservation, EnvelopeObservation, LegacyAuctionOwner,
    LegacyReturnEvidence, SenderIdentity, is_consign_subject,
};
use sha2::{Digest, Sha256};

use super::ConsignmentDetailObservation;
use super::ConsignmentListRow;
use super::FrozenMailAttachment;
use super::FrozenMailManifest;
use super::parse::unavailable_observation;
use crate::clients::mooglemail::MoogleMailWorld;

impl ConsignmentDetailObservation {
    /// 已认证但清单无法读取时的受控观察；保留原定位并明确标记未知。
    pub fn unavailable(world: MoogleMailWorld, row: &ConsignmentListRow) -> Self {
        unavailable_observation(world, row)
    }

    /// 页面主题是否走寄售协议；旧拍卖主题不改变。
    pub fn subject_is_consign(&self) -> bool {
        is_consign_subject(&self.subject)
    }

    /// 是否疑似系统 RTS；真正的 owner 归属由应用层先运行的拍卖路由决定。
    pub fn system_rts(&self) -> bool {
        self.sender == "MoogleMail"
            && self.subject.get(..6).is_some_and(|prefix| prefix.eq_ignore_ascii_case("[RTS] "))
    }

    /// 系统 RTS 上按旧退件路径同一口径拆出的义务、原收件人、正文摘要与派发引用。
    ///
    /// 不变量 7：正文只保留 canonical 摘要与末行引用，不把正文或秘密写入派生事实。
    pub fn legacy_return_evidence(&self) -> LegacyReturnEvidence {
        if !self.system_rts() {
            return LegacyReturnEvidence::default();
        }
        let obligation_id = system_returned_settlement_obligation_id(&self.sender, &self.subject);
        let Some(body) = parse_returned_settlement_body(&self.body) else {
            return LegacyReturnEvidence {
                obligation_id,
                subject: obligation_id.map(|_| self.subject.clone()),
                ..LegacyReturnEvidence::default()
            };
        };
        LegacyReturnEvidence {
            obligation_id,
            subject: obligation_id.map(|_| self.subject.clone()),
            returned_from: Some(body.returned_from),
            body_digest: Some(body.body_digest),
            dispatch_reference: body.dispatch_reference,
        }
    }

    /// 清单摘要；所有未判标记都参与，便于同观察重放与矛盾留案。
    pub fn manifest_digest(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(b"auction/consignment/mail-observation-manifest/v1\0");
        write_text(&mut digest, self.world.as_str());
        write_text(&mut digest, &self.external_mail_id);
        write_text(&mut digest, &self.received_minute_raw);
        write_text(&mut digest, &self.sender);
        write_text(&mut digest, &self.recipient);
        write_text(&mut digest, &self.subject);
        digest.update(self.subject_digest);
        digest.update(self.body_digest);
        write_optional_u64(&mut digest, self.reported_attachment_count.map(u64::from));
        write_optional_u64(&mut digest, self.credits);
        write_optional_u64(&mut digest, self.cod);
        match self.mail_state {
            Some(state) => {
                digest.update([1, state]);
            }
            None => digest.update([0]),
        }
        digest.update([
            u8::from(self.returnable),
            u8::from(self.claimable),
            u8::from(self.actions_proven),
            u8::from(self.list_detail_consistent),
            u8::from(self.duplicate_raw_eid),
            u8::from(self.canonical_alias_ambiguous),
            u8::from(self.unknown_attachment),
            u8::from(self.asset_count_unknown),
        ]);
        digest.update(u64::try_from(self.items.len()).unwrap_or(u64::MAX).to_be_bytes());
        for item in &self.items {
            digest.update(u64::from(item.ordinal).to_be_bytes());
            write_text(&mut digest, item.kind.as_str());
            write_optional_text(&mut digest, item.raw_eid.as_deref());
            write_optional_text(&mut digest, item.canonical_eid.as_deref());
            write_optional_u64(&mut digest, item.amount);
            write_text(&mut digest, &item.name);
        }
        digest.finalize().into()
    }

    /// 只有完全可证明且未判标记全清的观察才可冻结为写前清单。
    pub fn frozen_manifest(&self) -> Option<FrozenMailManifest> {
        if self.asset_count_unknown
            || self.unknown_attachment
            || self.duplicate_raw_eid
            || self.canonical_alias_ambiguous
            || !self.list_detail_consistent
            || !self.actions_proven
        {
            return None;
        }
        let credits = self.credits?;
        let cod = self.cod?;
        let mut seen = HashSet::new();
        let mut attachments = Vec::with_capacity(self.items.len());
        for item in &self.items {
            if item.kind != EnvelopeItemKind::Equipment {
                return None;
            }
            let canonical_eid = item.canonical_eid.clone()?;
            if !seen.insert(canonical_eid.clone()) || item.name.is_empty() {
                return None;
            }
            attachments.push(FrozenMailAttachment {
                ordinal: item.ordinal,
                name: item.name.clone(),
                canonical_eid: Some(canonical_eid),
            });
        }
        Some(FrozenMailManifest {
            world: self.world,
            external_mail_id: self.external_mail_id.clone(),
            sender: self.sender.clone(),
            recipient: self.recipient.clone(),
            subject: self.subject.clone(),
            body_digest: self.body_digest,
            credits,
            cod,
            received_minute_raw: self.received_minute_raw.clone(),
            received_at: self.received_at,
            attachments,
            manifest_digest: self.manifest_digest(),
        })
    }

    /// 转成 4A 的领域观察；卖家、可交易性、历史与受保护凭证引用由应用层逐项补证。
    pub fn to_envelope_observation(
        &self,
        observation_id: uuid::Uuid,
        session_binding_digest: [u8; 32],
        sender_identity: SenderIdentity,
        system_rts: bool,
        legacy_auction_owner: Option<LegacyAuctionOwner>,
    ) -> EnvelopeObservation {
        let asset_count = (!self.asset_count_unknown)
            .then_some(self.items.len())
            .and_then(|count| u32::try_from(count).ok());
        let has_assets =
            !self.items.is_empty() || self.credits.unwrap_or(0) > 0 || self.cod.unwrap_or(0) > 0;
        let takeable = (self.actions_proven
            && !self.unknown_attachment
            && self.claimable == has_assets
            && self.mail_state == Some(1))
        .then_some(self.claimable);
        EnvelopeObservation {
            observation_id,
            session_binding_digest,
            manifest_digest: self.manifest_digest(),
            parser_version: self.parser_version.to_owned(),
            subject_is_consign: self.subject_is_consign(),
            sender_identity,
            list_detail_consistent: self.list_detail_consistent,
            asset_count,
            duplicate_raw_eid: self.duplicate_raw_eid,
            canonical_alias_ambiguous: self.canonical_alias_ambiguous,
            unknown_attachment: self.unknown_attachment,
            credits: self.credits.and_then(|value| i64::try_from(value).ok()),
            cod: self.cod.and_then(|value| i64::try_from(value).ok()),
            returnable: self.returnable,
            takeable,
            system_rts,
            legacy_auction_owner,
            items: self
                .items
                .iter()
                .map(|item| EnvelopeItemObservation {
                    ordinal: item.ordinal,
                    kind: item.kind,
                    display_name: Some(item.name.clone()),
                    raw_eid: item.raw_eid.clone(),
                    canonical_eid: item.canonical_eid.clone(),
                    amount: item.amount,
                    world: Some(self.world.into()),
                    seller_user_id: None,
                    tradeable: false,
                    history_ok: false,
                    protected_ref: None,
                })
                .collect(),
        }
    }
}

fn write_text(digest: &mut Sha256, value: &str) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    digest.update(value.as_bytes());
}

fn write_optional_text(digest: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            digest.update([1]);
            write_text(digest, value);
        }
        None => digest.update([0]),
    }
}

fn write_optional_u64(digest: &mut Sha256, value: Option<u64>) {
    match value {
        Some(value) => {
            digest.update([1]);
            digest.update(value.to_be_bytes());
        }
        None => digest.update([0]),
    }
}

#[cfg(test)]
mod tests {
    use auction_domain::consignment::EnvelopeItemKind;
    use auction_domain::consignment::deposit::{LegacyReturnEvidence, SenderIdentity};
    use auction_time::Timestamp;

    use super::super::ConsignmentAttachmentObservation;
    use super::super::ConsignmentListRow;
    use super::super::parse::unavailable_observation;
    use super::ConsignmentDetailObservation;
    use crate::clients::mooglemail::MoogleMailWorld;

    fn observation(sender: &str, subject: &str) -> ConsignmentDetailObservation {
        let row = ConsignmentListRow {
            external_mail_id: "100".to_owned(),
            world: MoogleMailWorld::Main,
            world_marker: None,
            sender: sender.to_owned(),
            subject: subject.to_owned(),
            received_minute_raw: "2026-11-21 10:00".to_owned(),
            received_at: Timestamp::UNIX_EPOCH,
        };
        let mut observation = unavailable_observation(MoogleMailWorld::Main, &row);
        observation.sender = sender.to_owned();
        observation.subject = subject.to_owned();
        observation
    }

    #[test]
    fn system_rts_requires_mooglemail_and_accepts_ascii_case_variants() {
        assert!(observation("MoogleMail", "[RTS] Auction buyer refund [x]").system_rts());
        assert!(observation("MoogleMail", "[rts] Auction buyer refund [x]").system_rts());
        assert!(observation("MoogleMail", "[RtS] Auction buyer refund [x]").system_rts());
        assert!(!observation("Seller", "[RTS] Auction buyer refund [x]").system_rts());
        assert!(!observation("MoogleMail", "[RTS]Auction buyer refund [x]").system_rts());
        assert!(!observation("MoogleMail", "Auction buyer refund [x]").system_rts());
    }

    #[test]
    fn envelope_observation_keeps_the_page_display_name_for_marker_pairing() {
        let mut detail = observation("Seller", "consign");
        detail.items = vec![ConsignmentAttachmentObservation {
            ordinal: 1,
            kind: EnvelopeItemKind::Equipment,
            raw_eid: Some("42".to_owned()),
            canonical_eid: Some("42".to_owned()),
            amount: None,
            name: "Fake Sword".to_owned(),
            key: None,
        }];
        let envelope = detail.to_envelope_observation(
            uuid::Uuid::new_v4(),
            [0; 32],
            SenderIdentity::Verified,
            false,
            None,
        );
        assert_eq!(envelope.items[0].display_name.as_deref(), Some("Fake Sword"));
    }

    #[test]
    fn system_rts_carries_the_same_legacy_return_evidence_as_the_old_path() {
        let obligation = "5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a";
        let original = "Auction buyer refund body\n\nDispatch reference: v1/2/0d6c1d0c-63c4-5d0b-9a4f-1d7d2f8e9a01";
        let mut detail =
            observation("MoogleMail", &format!("[RTS] Auction buyer refund [{obligation}]"));
        detail.body = format!("{original}\n\nThis message was returned from Fake Buyer, kupo!");
        let evidence = detail.legacy_return_evidence();
        assert_eq!(
            evidence.obligation_id.map(|value| value.to_string()).as_deref(),
            Some(obligation)
        );
        assert_eq!(evidence.returned_from.as_deref(), Some("Fake Buyer"));
        assert_eq!(
            evidence.body_digest,
            Some(auction_application::settlement::returned_mail_body_digest(original))
        );
        assert_eq!(
            evidence.dispatch_reference.as_deref(),
            Some("v1/2/0d6c1d0c-63c4-5d0b-9a4f-1d7d2f8e9a01")
        );
        // 非系统发件人即使伪造相同的主题与正文后缀也不产生旧退件证据。
        let mut forged =
            observation("FakeSeller", &format!("[RTS] Auction buyer refund [{obligation}]"));
        forged.body = detail.body;
        assert_eq!(forged.legacy_return_evidence(), LegacyReturnEvidence::default());
    }
}
