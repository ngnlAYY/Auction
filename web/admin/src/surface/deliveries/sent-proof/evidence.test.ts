// 对应 Rust `sent_proof/tests.rs::decodes_lowercase_hex_digest_only`。

import { describe, expect, test } from "bun:test";

import { adminDeliveryDigest32FromHex } from "@auction/contracts/dto/admin/delivery";

describe("Sent 证据解码（对应 Rust sent_proof/tests.rs）", () => {
  test("decodes_lowercase_hex_digest_only", () => {
    const digest = adminDeliveryDigest32FromHex("ab".repeat(32));
    if (digest === null) throw new Error("小写摘要应解码成功");
    expect([...digest]).toEqual(Array.from({ length: 32 }, () => 0xab));
    expect(adminDeliveryDigest32FromHex("AB".repeat(32))).toBeNull();
    expect(adminDeliveryDigest32FromHex("ab")).toBeNull();
    expect(adminDeliveryDigest32FromHex("zz".repeat(32))).toBeNull();
  });
});
