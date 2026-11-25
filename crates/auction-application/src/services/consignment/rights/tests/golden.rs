//! 受限权利四个命令的 v2 规范编码黄金向量；字节流逐段手工拼出，摘要冻结。

use auction_domain::SettlementWorld;
use auction_domain::consignment::rights::{RightAction, RightLocator, locator_digest};
use uuid::Uuid;

use super::browser;
use crate::ports::consignment::GameIdentity;
use crate::services::consignment::deposit::encoding::{COMMAND_PREFIX_V2, CommandEncoding};
use crate::services::consignment::rights::challenge::canonical_challenge;
use crate::services::consignment::rights::claim::canonical_claim;
use crate::services::consignment::rights::locate::canonical_locate;
use crate::services::consignment::rights::verify::canonical_verify;

fn text(value: &str) -> Vec<u8> {
    let mut bytes = (value.len() as u64).to_be_bytes().to_vec();
    bytes.extend_from_slice(value.as_bytes());
    bytes
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn right_id() -> Uuid {
    uuid::Builder::from_random_bytes([0x33; 16]).into_uuid()
}

fn challenge_id() -> Uuid {
    uuid::Builder::from_random_bytes([0x44; 16]).into_uuid()
}

/// 浏览器三命令的头：前缀、操作、actor 类型、actor 引用、目标（均为绑定摘要十六进制）。
fn browser_head(operation: &str) -> Vec<u8> {
    let binding = hex(&[1_u8; 32]);
    let mut bytes = COMMAND_PREFIX_V2.to_vec();
    bytes.extend(text(operation));
    bytes.extend(text("browser"));
    bytes.extend(text(&binding));
    bytes.extend(text(&binding));
    bytes
}

fn assert_vector(encoding: &CommandEncoding, expected: &[u8], sha: &str) {
    assert_eq!(hex(encoding.as_bytes()), hex(expected));
    assert_eq!(hex(&encoding.request_hash().as_bytes()), sha);
}

#[test]
fn rights_commands_have_frozen_v2_golden_vectors() {
    assert!(COMMAND_PREFIX_V2.starts_with(b"auction.consignment.command.v2\0"));

    let identity = GameIdentity::parse("Alice").expect("身份");
    let mut expected = browser_head("locate_right");
    expected.extend(text("Alice"));
    expected.extend(text("main"));
    assert_vector(
        &canonical_locate(browser(1), &identity, SettlementWorld::Main),
        &expected,
        LOCATE_SHA,
    );

    let locator = locator_digest(&RightLocator::from_entropy([0x22; 32]));
    let mut expected = browser_head("challenge_right");
    expected.extend(locator.as_bytes());
    assert_vector(&canonical_challenge(browser(1), &locator), &expected, CHALLENGE_SHA);

    let mut expected = browser_head("verify_right");
    expected.extend(challenge_id().as_bytes());
    expected.extend([0x55; 32]);
    assert_vector(
        &canonical_verify(browser(1), challenge_id(), &[0x55; 32]),
        &expected,
        VERIFY_SHA,
    );

    let mut expected = COMMAND_PREFIX_V2.to_vec();
    expected.extend(text("claim_right"));
    expected.extend(text("right_holder"));
    expected.extend(right_id().as_bytes());
    expected.extend(right_id().as_bytes());
    expected.extend(7_u64.to_be_bytes());
    expected.extend(text("claim_equipment"));
    assert_vector(
        &canonical_claim(right_id(), 7, RightAction::ClaimEquipment),
        &expected,
        CLAIM_SHA,
    );
}

const LOCATE_SHA: &str = "07712096f363f4f9b86c48da16c5d3ce71bafaac266826c0f999a5b77f69c693";
const CHALLENGE_SHA: &str = "3b287b98fb61653878907bb2b8b4bd6eb81dc7d4d19cd0416d49353f8abf2d6b";
const VERIFY_SHA: &str = "e9d940a54137e802b68610f79a50c2a24e888449af71b83aac391cbb0d35ff9f";
const CLAIM_SHA: &str = "d3c33d264d9001ddee7f482c14ce818dac36080428da52f29e978019c090645b";
