// Rust `delivery/tests.rs` 的玩家支持 URL 与契约样本回归。

import { expect, test } from "bun:test";
import { AdminPlayerSupportBundleResponseSchema } from "@auction/contracts/dto/admin/delivery";
import { decode } from "@auction/contracts/wire";

import { strictDeliveryEnvelopeSchema } from "./decode";
import {
  DELIVERY_ID,
  parseFixture,
  readAdminFixture,
  requireDecoded,
  requireElement,
} from "./delivery-fixture.test";
import { decodePlayerSupport, playerSupportUrl } from "./player-support";

test("player_support_url_requests_the_settlement_returns_projection", () => {
  expect(playerSupportUrl("user", "Sample Name")).toBe(
    "/api/v1/admin/player-support?user=Sample+Name&include=settlement_returns",
  );
  expect(playerSupportUrl("reference", DELIVERY_ID)).toBe(
    `/api/v1/admin/player-support?reference=${DELIVERY_ID}&include=settlement_returns`,
  );
});

test("player_support_bids_decode_backend_contract_fixtures", () => {
  for (const [name, count, mechanism] of [
    ["player-support", 1, "ordinary"],
    ["player-support-settlement-returns", 2, "proxy"],
  ] as const) {
    const payload = requireDecoded(
      decode(
        strictDeliveryEnvelopeSchema(AdminPlayerSupportBundleResponseSchema),
        parseFixture(readAdminFixture(name)),
      ),
      name,
    );
    const support = requireDecoded(decodePlayerSupport(payload), name);
    expect(support.deliveries).toHaveLength(count);
    expect(requireElement(support.bids, 0, "首条出价").mechanism).toBe(mechanism);
  }
});
