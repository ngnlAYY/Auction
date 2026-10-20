//! 按武器、盾牌及护甲材质关联前缀；部位不改变可用前缀集合。

use super::Recommendations;

const ELEMENTAL_MITIGATION: &[&str] = &["Ruby", "Cobalt", "Amber", "Jade", "Zircon", "Onyx"];
const WEAPON_PREFIXES: &[&str] =
    &["Ethereal", "Fiery", "Arctic", "Shocking", "Tempestuous", "Hallowed", "Demonic"];

pub(super) fn for_kind(kind: &str) -> Recommendations {
    match kind {
        "Buckler" | "Kite Shield" => {
            return Recommendations::new([&["Reinforced", "Agile", "Mithril"], &[], &[]]);
        }
        "Tower Shield" => {
            return Recommendations::new([&["Reinforced", "Mithril"], &[], &[]]);
        }
        "Force Shield" => return Recommendations::new([ELEMENTAL_MITIGATION, &[], &[]]),
        "Axe" | "Club" | "Dagger" | "Rapier" | "Shortsword" | "Wakizashi" | "Estoc" | "Katana"
        | "Longsword" | "Mace" | "Great Mace" | "Scythe" | "Swordchucks" | "Ebony Staff"
        | "Katalox Staff" | "Oak Staff" | "Redwood Staff" | "Willow Staff" => {
            return Recommendations::new([WEAPON_PREFIXES, &[], &[]]);
        }
        _ => {}
    }
    let material = kind.split_whitespace().next().unwrap_or_default();
    let special: &[&str] = match material {
        "Cotton" => &["Charged", "Frugal"],
        "Phase" => &["Charged", "Frugal", "Radiant", "Mystic"],
        "Ironsilk" | "Kevlar" => &["Reinforced"],
        "Leather" => &["Reinforced", "Agile"],
        "Shade" => &["Savage", "Agile"],
        "Plate" => &["Mithril", "Shielding"],
        "Chain" => &["Mithril", "Reinforced"],
        "Reactive" => &["Mithril"],
        "Power" => &["Mithril", "Savage"],
        _ => &[],
    };
    Recommendations::new([special, ELEMENTAL_MITIGATION, &[]])
}
