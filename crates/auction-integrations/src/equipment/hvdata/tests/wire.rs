//! 以脱敏来源样本核对字段、结构预算和精度边界。

use auction_application::consignment::{
    ListingEquipmentLevel, ListingEquipmentLookupError, ListingLookupSnapshot,
    LookupPermanentReason,
};
use auction_domain::EquipmentKey;
use auction_domain::consignment::ListingEquipmentIdentity;
use auction_test_support::hvdata::{
    EQUIPMENT, FIXTURE_EID, FIXTURE_KEY, FIXTURE_OWNER, MISSING_PARAMETER, NOT_FOUND,
};
use auction_time::Timestamp;
use reqwest::StatusCode;
use serde_json::{Value, json};

use super::super::http::decode_response;

type Reason = LookupPermanentReason;
type LookupError = ListingEquipmentLookupError;

fn fixture() -> Value {
    serde_json::from_slice(EQUIPMENT).expect("脱敏样本 JSON")
}

fn as_wire(document: &Value) -> String {
    serde_json::to_string(document).expect("序列化测试样本")
}

fn decode(
    status: StatusCode,
    body: &[u8],
    world_id: &str,
    equipment_id: &str,
) -> Result<ListingLookupSnapshot, LookupError> {
    let identity = ListingEquipmentIdentity::new(world_id, equipment_id).expect("测试请求身份");
    let key = EquipmentKey::new(FIXTURE_KEY).expect("测试凭证");
    decode_response(status, body, &identity, &key, Timestamp::UNIX_EPOCH)
}

fn snapshot(body: impl AsRef<[u8]>) -> Result<ListingLookupSnapshot, LookupError> {
    decode(StatusCode::OK, body.as_ref(), "main", FIXTURE_EID)
}

fn rejects(body: impl AsRef<[u8]>, reason: Reason) {
    assert_eq!(snapshot(body).expect_err("响应应失败关闭"), reason.into());
}

#[test]
fn hvdata_sample_snapshot_projects_only_verified_fields() {
    let snapshot = snapshot(EQUIPMENT).expect("已见成功样本");
    assert_eq!(snapshot.identity().external_equipment_id(), FIXTURE_EID);
    assert_eq!(snapshot.identity().world_id(), "main");
    assert_eq!(snapshot.owner_name(), FIXTURE_OWNER);
    let details = snapshot.details();
    assert_eq!(details.observed_at(), Timestamp::UNIX_EPOCH);
    assert_eq!(details.label().name, "Peerless Tempestuous Willow Staff of Destruction");
    assert_eq!(details.label().category, "Staff");
    assert!(details.label().alt_name.is_some());
    assert_eq!(details.label().level, ListingEquipmentLevel::Soulbound);
    assert!(!details.label().tradeable);
    assert_eq!(
        details.stat_groups().iter().map(|group| group.name.as_str()).collect::<Vec<_>>(),
        ["Primary Attributes", "Proficiency", "Spell Damage", "misc"]
    );
    let rendered = format!("{snapshot:?} {details:?}");
    assert!(!rendered.contains(FIXTURE_KEY));
    assert!(!rendered.contains(FIXTURE_OWNER));
}

#[test]
fn hvdata_error_samples_only_recognize_observed_shape() {
    for (status, sample, expected, unknown) in [
        (
            StatusCode::NOT_FOUND,
            NOT_FOUND,
            Reason::EquipmentNotFound,
            br#"{"detail":"Unknown"}"#.as_slice(),
        ),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            MISSING_PARAMETER,
            Reason::SourceRequestRejected,
            br#"{"detail":[]}"#.as_slice(),
        ),
    ] {
        assert_eq!(
            decode(status, sample, "main", FIXTURE_EID).expect_err("错误样本"),
            expected.into()
        );
        assert_eq!(
            decode(status, unknown, "main", FIXTURE_EID).expect_err("未知错误样本"),
            Reason::SourceSchemaInvalid.into()
        );
    }
    let known = serde_json::from_slice::<Value>(MISSING_PARAMETER).expect("脱敏错误样本");
    for (field, malformed) in [
        ("type", json!("other")),
        ("msg", json!(42)),
        ("input", json!({"url":format!("https://source.invalid/equip?key={FIXTURE_KEY}")})),
        ("loc", json!(["body", "key"])),
        ("loc", json!(["query", "unknown"])),
        ("loc", json!(["query", "key", "extra"])),
    ] {
        let mut document = known.clone();
        document["detail"][0][field] = malformed;
        let error = decode(
            StatusCode::UNPROCESSABLE_ENTITY,
            as_wire(&document).as_bytes(),
            "main",
            FIXTURE_EID,
        )
        .expect_err("未见错误形态");
        assert_eq!(error, Reason::SourceSchemaInvalid.into());
        let text = format!("{error} {error:?}");
        assert!(!text.contains(FIXTURE_KEY));
        assert!(!text.contains("https://"));
    }
}

#[test]
fn hvdata_json_document_rejects_duplicate_keys_and_invalid_syntax() {
    let valid = as_wire(&fixture());
    assert!(snapshot(&valid).is_ok());
    let body = valid.strip_prefix('{').expect("对象样本");
    for prefix in [
        r#"{"name":"重复","#,
        r#"{"\u006eame":"转义后重复","#,
        r#"{"extension":{"marker":1,"marker":2},"#,
        r#"{"extension":[{"marker":1,"\u006darker":2}],"#,
    ] {
        rejects(format!("{prefix}{body}"), Reason::SourceSchemaInvalid);
    }
    for invalid in [
        format!("{valid} {{}}"),
        valid.replace("424242", "NaN"),
        valid.replace("424242", "Infinity"),
        r#"[]"#.to_owned(),
        format!(r#"{{"extension":"\ud800",{body}"#),
        format!(r#"{{"extension":1e999,{body}"#),
        format!(r#"{{"extension":1e-999,{body}"#),
    ] {
        rejects(invalid, Reason::SourceSchemaInvalid);
    }
    let mut invalid_utf8 = EQUIPMENT.to_vec();
    invalid_utf8.push(0xff);
    rejects(invalid_utf8, Reason::SourceSchemaInvalid);
    let mut invalid_in_string = b"{\"extension\":\"".to_vec();
    invalid_in_string.push(0xff);
    invalid_in_string.extend_from_slice(b"\",");
    invalid_in_string.extend_from_slice(body.as_bytes());
    rejects(invalid_in_string, Reason::SourceSchemaInvalid);
}

#[test]
fn hvdata_structure_budget_accepts_exact_depth_and_rejects_next_level() {
    for nested_count in [7, 8] {
        let mut value = json!(0);
        for depth in 0..nested_count {
            value = if depth % 2 == 0 { json!([value]) } else { json!({"nested":value}) };
        }
        let mut document = fixture();
        document["extension"] = value;
        if nested_count == 7 {
            assert!(snapshot(as_wire(&document)).is_ok());
        } else {
            rejects(as_wire(&document), Reason::SourceTooLarge);
        }
    }
}

#[test]
fn hvdata_structure_budget_counts_all_object_keys_and_array_items() {
    for (count, passes) in [(128, true), (129, false)] {
        let object = (0..count).map(|n| (format!("field_{n}"), json!(0))).collect();
        let mut document = fixture();
        document["unconsumed"] = Value::Object(object);
        if passes {
            assert!(snapshot(as_wire(&document)).is_ok());
        } else {
            rejects(as_wire(&document), Reason::SourceTooLarge);
        }
    }
    for (count, passes) in [(256, true), (257, false)] {
        let mut document = fixture();
        document["unconsumed"] = json!(vec![0; count]);
        if passes {
            assert!(snapshot(as_wire(&document)).is_ok());
        } else {
            rejects(as_wire(&document), Reason::SourceTooLarge);
        }
    }
}

#[test]
fn hvdata_eid_compares_integer_token_without_rounding() {
    let valid = as_wire(&fixture());
    let eid = "92233720368547758081234567890123";
    let body = valid.replace(FIXTURE_EID, eid);
    let good = decode(StatusCode::OK, body.as_bytes(), "main", eid).expect("32 位 ID 应精确接收");
    assert_eq!(good.identity().external_equipment_id(), eid);
    let adjacent = body.replace(eid, "92233720368547758081234567890124");
    assert_eq!(
        decode(StatusCode::OK, adjacent.as_bytes(), "main", eid).expect_err("邻接编号不符"),
        Reason::SourceIdentityMismatch.into()
    );
    for token in [
        r#""424242""#,
        "4.24242e5",
        "424242e0",
        "-424242",
        "424242.0",
        "0424242",
        "0",
        "123456789012345678901234567890123",
    ] {
        rejects(valid.replace(FIXTURE_EID, token), Reason::SourceSchemaInvalid);
    }
}

#[test]
fn hvdata_key_and_optional_world_echoes_must_match() {
    let mut document = fixture();
    document["key"] = json!(FIXTURE_KEY.to_ascii_uppercase());
    document["is_isekai"] = json!(true);
    document["world_id"] = json!("isekai");
    let valid = as_wire(&document);
    let read = decode(StatusCode::OK, valid.as_bytes(), "isekai", FIXTURE_EID)
        .expect("大小写凭证和两个世界回显一致");
    assert_eq!(read.identity().world_id(), "isekai");

    for (field, wrong, reason) in [
        ("is_isekai", json!(false), Reason::SourceIdentityMismatch),
        ("world_id", json!("main"), Reason::SourceIdentityMismatch),
        ("world_id", json!(null), Reason::SourceSchemaInvalid),
        ("is_isekai", json!("true"), Reason::SourceSchemaInvalid),
        ("key", json!("decaf00001"), Reason::SourceIdentityMismatch),
        ("key", json!("incorrect!"), Reason::SourceSchemaInvalid),
    ] {
        let mut mutated = document.clone();
        mutated[field] = wrong;
        assert_eq!(
            decode(StatusCode::OK, as_wire(&mutated).as_bytes(), "isekai", FIXTURE_EID)
                .expect_err("错误回显"),
            reason.into()
        );
    }
}

#[test]
fn hvdata_owner_requires_name_and_optional_uid_is_positive_integer() {
    let mut document = fixture();
    document["owner"] = json!({"name": FIXTURE_OWNER});
    let read = snapshot(as_wire(&document)).expect("owner UID 可以省略");
    assert_eq!(read.owner_name(), FIXTURE_OWNER);

    let mut absent = document.clone();
    absent.as_object_mut().expect("对象").remove("owner");
    rejects(as_wire(&absent), Reason::OwnerMissing);
    for (owner, reason) in [
        (json!(null), Reason::OwnerMissing),
        (json!({ "uid":1 }), Reason::OwnerMissing),
        (json!([]), Reason::SourceSchemaInvalid),
        (json!({ "name":null }), Reason::SourceSchemaInvalid),
        (json!({ "name":45 }), Reason::SourceSchemaInvalid),
        (json!({ "name":FIXTURE_OWNER, "uid":"42" }), Reason::SourceSchemaInvalid),
        (json!({ "name":FIXTURE_OWNER, "uid":0 }), Reason::SourceSchemaInvalid),
        (json!({ "name":FIXTURE_OWNER, "uid":-1 }), Reason::SourceSchemaInvalid),
        (json!({ "name":FIXTURE_OWNER, "uid":10.5 }), Reason::SourceSchemaInvalid),
    ] {
        document["owner"] = owner;
        rejects(as_wire(&document), reason);
    }
    let valid = as_wire(&fixture());
    rejects(valid.replace("\"uid\":42424", "\"uid\":4e1"), Reason::SourceSchemaInvalid);
}

#[test]
fn hvdata_label_owner_and_optional_alias_use_byte_limits() {
    for (field, maximum) in [("name", 512), ("alt_name", 512), ("category", 128), ("owner", 256)] {
        let mut document = fixture();
        for length in [maximum, maximum + 1] {
            let text = "厚".repeat(length / 3) + &"a".repeat(length % 3);
            if field == "owner" {
                document[field]["name"] = json!(text);
            } else {
                document[field] = json!(text);
            }
            if length == maximum {
                assert!(snapshot(as_wire(&document)).is_ok(), "{field}");
            } else {
                rejects(as_wire(&document), Reason::SourceSchemaInvalid);
            }
        }
        for invalid in ["", " leading", "trailing ", "line\nfeed", "line\u{2028}separator"] {
            if field == "owner" {
                document[field]["name"] = json!(invalid);
            } else {
                document[field] = json!(invalid);
            }
            rejects(as_wire(&document), Reason::SourceSchemaInvalid);
        }
    }
}

#[test]
fn hvdata_rejects_secret_text_even_in_whitelisted_fields() {
    let mut document = fixture();
    document["owner"]["name"] = json!(format!("Given {}", FIXTURE_KEY.to_ascii_uppercase()));
    rejects(as_wire(&document), Reason::SourceSchemaInvalid);
    document["owner"]["name"] = json!(FIXTURE_OWNER);
    document["stats"] = json!({"bonus": {"read key=decaf00000": {"value":12}}});
    rejects(as_wire(&document), Reason::SourceSchemaInvalid);
    document["stats"] = json!({"bonus": {"read": {"value":12}}});
    document["name"] = json!("https://example.invalid/equip/42/0000decafe");
    rejects(as_wire(&document), Reason::SourceSchemaInvalid);
}

#[test]
fn hvdata_level_only_allows_confirmed_soulbound_pair() {
    let mut document = fixture();
    assert!(snapshot(as_wire(&document)).is_ok());
    for level in [json!(42), json!("42"), json!("Unassigned"), Value::Null, json!({})] {
        for tradeable in [false, true] {
            document["level"] = level.clone();
            document["is_tradeable"] = json!(tradeable);
            rejects(as_wire(&document), Reason::SourceShapeUnconfirmed);
        }
    }
    document["level"] = json!("Soulbound");
    for tradeable in [json!(true), json!("false"), json!(0), Value::Null] {
        document["is_tradeable"] = tradeable;
        rejects(as_wire(&document), Reason::SourceSchemaInvalid);
    }
    document.as_object_mut().expect("对象").remove("is_tradeable");
    rejects(as_wire(&document), Reason::SourceSchemaInvalid);
    document["is_tradeable"] = json!(false);
    document.as_object_mut().expect("对象").remove("level");
    rejects(as_wire(&document), Reason::SourceSchemaInvalid);
}

#[test]
fn hvdata_optional_name_parts_only_allows_seen_nulls() {
    let mut document = fixture();
    document.as_object_mut().expect("对象").remove("name_parts");
    document.as_object_mut().expect("对象").remove("alt_name");
    let without_optionals = snapshot(as_wire(&document)).expect("可选字段可以省略");
    assert!(without_optionals.details().label().alt_name.is_none());
    document["alt_name"] = Value::Null;
    rejects(as_wire(&document), Reason::SourceSchemaInvalid);
    document.as_object_mut().expect("对象").remove("alt_name");
    for valid in [
        json!({"armor_type":null, "slot_type":null, "weapon_type":"Fixture"}),
        json!({"prefix":"Fixture", "unknown":[1,2,3]}),
    ] {
        document["name_parts"] = valid;
        assert!(snapshot(as_wire(&document)).is_ok());
    }
    for invalid in [
        json!([]),
        Value::Null,
        json!({"tier":null}),
        json!({"weapon_type":null}),
        json!({"suffix":true}),
        json!({"prefix":" bad "}),
    ] {
        document["name_parts"] = invalid;
        rejects(as_wire(&document), Reason::SourceSchemaInvalid);
    }
}

#[test]
fn hvdata_stats_total_and_names_are_bounded_and_sorted() {
    let mut document = fixture();
    let group: serde_json::Map<String, Value> =
        (0..128).map(|n| (format!("stat_{n:03}"), json!({"value":1}))).collect();
    document["stats"] = json!({"组":group,"A":group});
    let read = snapshot(as_wire(&document)).expect("256 条属性在预算内");
    let groups = read.details().stat_groups();
    assert_eq!(groups.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(), ["A", "组"]);
    assert_eq!(groups.iter().map(|entry| entry.stats.len()).sum::<usize>(), 256);
    assert_eq!(groups[0].stats[0].name, "stat_000");
    assert_eq!(groups[0].stats[127].name, "stat_127");
    document["stats"]["third"] = json!({"extra":{"value":1}});
    rejects(as_wire(&document), Reason::SourceTooLarge);

    let boundary = "界".repeat(42) + "aa";
    document["stats"] = json!({(&boundary): {(&boundary): {"value":1}}});
    assert!(snapshot(as_wire(&document)).is_ok());
    for invalid in ["", " trailing ", "new\nline", &format!("{boundary}a")] {
        document["stats"] = json!({(invalid): {"valid": {"value":1}}});
        rejects(as_wire(&document), Reason::SourceSchemaInvalid);
        document["stats"] = json!({"valid": {(invalid): {"value":1}}});
        rejects(as_wire(&document), Reason::SourceSchemaInvalid);
    }
}

#[test]
fn hvdata_stat_shapes_and_decimal_limit_fail_closed() {
    let mut document = fixture();
    document["stats"] =
        json!({"test":{"a":{"value":6.5},"b":{"value":-1000000000,"base":1000000000}}});
    let read = snapshot(as_wire(&document)).expect("base 省略和恰好上限合法");
    let stats = &read.details().stat_groups()[0].stats;
    assert_eq!((stats[0].value, stats[0].base), (6.5, None));
    assert_eq!((stats[1].value, stats[1].base), (-1e9, Some(1e9)));
    let valid = as_wire(&document);
    for token in ["1000000000.00000000001", "1.00000000000000001e9", "1000000001", "1e309"] {
        rejects(valid.replace("1000000000", token), Reason::SourceSchemaInvalid);
    }
    for token in ["1e9", "0.001000e12", "999999999.99999999999"] {
        assert!(snapshot(valid.replace("1000000000", token)).is_ok());
    }
    for invalid in [
        json!([]),
        json!({"test":null}),
        json!({"test":{"a":2}}),
        json!({"test":{"a":{}}}),
        json!({"test":{"a":{"base":1}}}),
        json!({"test":{"a":{"value":"1"}}}),
        json!({"test":{"a":{"value":1,"base":null}}}),
        json!({"test":{"a":{"value":1,"base":"1"}}}),
    ] {
        document["stats"] = invalid;
        rejects(as_wire(&document), Reason::SourceSchemaInvalid);
    }
}
