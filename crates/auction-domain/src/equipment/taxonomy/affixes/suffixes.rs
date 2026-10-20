//! 按具体种类关联后缀；护甲包含材质专属词缀和 0.91 恢复的元素减伤后缀。

use super::Recommendations;

const ELEMENTAL_MITIGATION: &[&str] =
    &["Fire-eater", "Frost-born", "Thunder-child", "Wind-waker", "Thrice-blessed", "Spirit-ward"];
const PROFICIENCIES: &[&str] =
    &["Elementalist", "Heaven-Sent", "Demon-Fiend", "Earth-walker", "Curse-weaver"];
const SPECIFIC_MITIGATION: &[&str] = &["Dampening", "Deflection", "Stoneskin"];

pub(super) fn for_kind(kind: &str) -> Recommendations {
    let weapon_or_shield: &[&str] = match kind {
        "Axe" => &[
            "Slaughter",
            "Balance",
            "Battlecaster",
            "Vampire",
            "Illithid",
            "Banshee",
            "Ox",
            "Raccoon",
        ],
        "Club" => &[
            "Slaughter",
            "Balance",
            "Nimble",
            "Swiftness",
            "Battlecaster",
            "Focus",
            "Vampire",
            "Illithid",
            "Banshee",
            "Raccoon",
            "Cheetah",
        ],
        "Dagger" => &[
            "Slaughter",
            "Balance",
            "Nimble",
            "Swiftness",
            "Battlecaster",
            "Focus",
            "Raccoon",
            "Cheetah",
        ],
        "Rapier" => &[
            "Slaughter",
            "Balance",
            "Nimble",
            "Swiftness",
            "Battlecaster",
            "Vampire",
            "Illithid",
            "Banshee",
            "Raccoon",
            "Cheetah",
        ],
        "Shortsword" => &[
            "Slaughter",
            "Balance",
            "Swiftness",
            "Battlecaster",
            "Focus",
            "Vampire",
            "Illithid",
            "Banshee",
            "Ox",
            "Raccoon",
        ],
        "Wakizashi" => {
            &["Slaughter", "Balance", "Nimble", "Swiftness", "Battlecaster", "Raccoon", "Cheetah"]
        }
        "Estoc" | "Longsword" | "Mace" | "Great Mace" => {
            &["Slaughter", "Balance", "Battlecaster", "Vampire", "Illithid", "Banshee", "Ox"]
        }
        "Katana" | "Scythe" => &["Slaughter", "Balance", "Battlecaster", "Ox"],
        // Wiki 总表与分项的明确差异继续取兼容并集，不把候选用作解析白名单。
        "Swordchucks" => &["Slaughter", "Balance", "Battlecaster", "Ox", "Raccoon"],
        "Ebony Staff" => &[
            "Destruction",
            "Focus",
            "Surtr",
            "Niflheim",
            "Mjolnir",
            "Freyr",
            "Heimdall",
            "Fenrir",
            "Elementalist",
            "Heaven-Sent",
            "Demon-Fiend",
            "Fox",
            "Owl",
        ],
        "Katalox Staff" => &[
            "Destruction",
            "Focus",
            "Heimdall",
            "Fenrir",
            "Heaven-Sent",
            "Demon-Fiend",
            "Earth-walker",
            "Curse-weaver",
            "Fox",
            "Owl",
        ],
        "Oak Staff" => &["Destruction", "Focus", "Heimdall", "Earth-walker", "Fox", "Owl"],
        "Redwood Staff" => &[
            "Destruction",
            "Focus",
            "Surtr",
            "Niflheim",
            "Mjolnir",
            "Freyr",
            "Elementalist",
            "Fox",
            "Owl",
        ],
        "Willow Staff" => &["Destruction", "Focus", "Fenrir", "Curse-weaver", "Fox", "Owl"],
        "Buckler" => {
            &["Barrier", "Nimble", "Battlecaster", "Fleet", "Protection", "Warding", "Turtle"]
        }
        "Kite Shield" => &[
            "Barrier",
            "Battlecaster",
            "Protection",
            "Warding",
            "Dampening",
            "Deflection",
            "Stoneskin",
            "Turtle",
        ],
        "Tower Shield" => {
            &["Barrier", "Protection", "Warding", "Dampening", "Deflection", "Stoneskin", "Turtle"]
        }
        "Force Shield" => {
            &["Protection", "Warding", "Dampening", "Deflection", "Stoneskin", "Turtle"]
        }
        _ => &[],
    };
    if !weapon_or_shield.is_empty() {
        return Recommendations::new([weapon_or_shield, &[], &[]]);
    }
    let material = kind.split_whitespace().next().unwrap_or_default();
    match material {
        "Cotton" => {
            Recommendations::new([&["Protection", "Warding"], PROFICIENCIES, ELEMENTAL_MITIGATION])
        }
        "Gossamer" => Recommendations::new([PROFICIENCIES, &["Fox", "Owl"], &[]]),
        "Ironsilk" => Recommendations::new([
            &["Protection", "Warding", "Fleet", "Raccoon", "Cheetah"],
            &[],
            &[],
        ]),
        "Phase" => Recommendations::new([
            PROFICIENCIES,
            &[
                "Raccoon", "Cheetah", "Fox", "Owl", "Surtr", "Niflheim", "Mjolnir", "Freyr",
                "Heimdall", "Fenrir",
            ],
            &[],
        ]),
        "Shade" => {
            Recommendations::new([&["Shadowdancer", "Arcanist", "Fleet", "Negation"], &[], &[]])
        }
        "Power" => {
            Recommendations::new([&["Slaughter", "Balance", "Protection", "Warding"], &[], &[]])
        }
        "Leather" | "Drakehide" | "Kevlar" => Recommendations::new([
            if material == "Leather" {
                &["Protection", "Warding", "Negation"]
            } else {
                &["Protection", "Warding", "Negation", "Battlecaster"]
            },
            SPECIFIC_MITIGATION,
            ELEMENTAL_MITIGATION,
        ]),
        "Plate" | "Chain" | "Reactive" => Recommendations::new([
            if material == "Reactive" {
                &["Protection", "Warding", "Barrier"]
            } else {
                &["Protection", "Warding"]
            },
            SPECIFIC_MITIGATION,
            ELEMENTAL_MITIGATION,
        ]),
        _ => Recommendations::new([&[], &[], &[]]),
    }
}
