//! 寄售登记的本地装备 URL 校验，沿用游戏装备链接准入规则。

use auction_application::consignment::{
    ConsignmentError, ListingEquipmentReference, ListingEquipmentUrlValidator,
};
use auction_domain::EquipmentKey;
use auction_domain::consignment::ListingEquipmentIdentity;
use url::Url;

use super::config::EquipmentSourcePolicy;
use super::parser::parse_equipment_url;

const GAME_ORIGIN: &str = "https://hentaiverse.org";

/// 将游戏链接转换成寄售端口使用的规范身份和脱敏凭证。
#[derive(Debug, Default)]
pub struct HentaiVerseListingUrlValidator;

impl ListingEquipmentUrlValidator for HentaiVerseListingUrlValidator {
    fn validate(&self, equipment_url: &str) -> Result<ListingEquipmentReference, ConsignmentError> {
        // 不变量 6、7：复用旧入口的 URL 准入，再验证新登记的严格身份；错误只返回闭集码。
        let invalid = ConsignmentError::InvalidEquipmentUrl;
        let Ok(endpoint) = Url::parse(GAME_ORIGIN) else {
            return Err(invalid);
        };
        let policy = EquipmentSourcePolicy::default();
        for world_id in ["main", "isekai"] {
            if let Ok((_target, external_equipment_id, raw_key)) =
                parse_equipment_url(&endpoint, &policy, world_id, equipment_url)
            {
                let identity = ListingEquipmentIdentity::new(world_id, &external_equipment_id)?;
                let Ok(key) = EquipmentKey::new(&raw_key) else {
                    return Err(invalid);
                };
                return Ok(ListingEquipmentReference { identity, key });
            }
        }
        Err(invalid)
    }
}

#[cfg(test)]
mod tests {
    use auction_application::consignment::{ConsignmentError, ListingEquipmentUrlValidator};

    use super::HentaiVerseListingUrlValidator;

    #[test]
    fn consignment_url_accepts_both_worlds_and_normalizes_key() {
        for (url, world_id, equipment_id) in [
            ("<https://hentaiverse.org/equip/42/DeCaF00000>。", "main", "42"),
            (
                "https://hentaiverse.org/isekai/equip/12345678901234567890123456789012/decaf00000!",
                "isekai",
                "12345678901234567890123456789012",
            ),
        ] {
            let reference = HentaiVerseListingUrlValidator.validate(url).expect("夹具链接应合法");
            assert_eq!(reference.identity.world_id(), world_id);
            assert_eq!(reference.identity.external_equipment_id(), equipment_id);
            assert!(reference.key.with_bytes(|bytes| bytes == b"decaf00000").unwrap_or_default());
            assert!(!format!("{reference:?}").contains("decaf00000"));
        }
    }

    #[test]
    fn consignment_url_rejects_noncanonical_ids_and_keys_without_echoing_input() {
        for equipment_id in ["042", "0", "00", "１２", "4z", "123456789012345678901234567890123"]
        {
            let url = format!("https://hentaiverse.org/equip/{equipment_id}/decaf00000");
            assert_invalid(&url);
        }
        for key in ["decaf0000", "decaf000000", "decafzzzzz", "decaf00000/extra"] {
            let url = format!("https://hentaiverse.org/equip/42/{key}");
            assert_invalid(&url);
        }
    }

    #[test]
    fn consignment_url_rejects_origin_and_path_deviations() {
        for url in [
            "http://hentaiverse.org/equip/42/decaf00000",
            "https://other.invalid/equip/42/decaf00000",
            "https://hentaiverse.org:444/equip/42/decaf00000",
            "https://fixture@hentaiverse.org/equip/42/decaf00000",
            "https://hentaiverse.org/equip/42/decaf00000?extra=1",
            "https://hentaiverse.org/equip/42/decaf00000#extra",
            "https://hentaiverse.org/a/../equip/42/decaf00000",
            "https://hentaiverse.org/equip/42/decaf00000 ",
            " https://hentaiverse.org/equip/42/decaf00000",
            "https://hentaiverse.org/equip/4\t2/decaf00000",
            "https://hentaiverse.org/equip/%34%32/decaf00000",
            "https://hentaiverse.org/equip/42/decaf00000>",
        ] {
            assert_invalid(url);
        }
    }

    fn assert_invalid(url: &str) {
        let error = HentaiVerseListingUrlValidator.validate(url).expect_err("夹具链接应被拒绝");
        assert_eq!(error, ConsignmentError::InvalidEquipmentUrl);
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains(url));
        assert!(!rendered.contains("decaf00000"));
    }
}
