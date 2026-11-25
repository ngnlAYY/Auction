// 验证 Admin v2 挂单列表、详情、有效止售与运营动作的线协议。

import { expect, test } from "bun:test";

import { decode } from "../../../wire";
import { readResponse } from "../../public/consignment/consignment-fixture";
import {
  AdminListingDetailV2Schema,
  AdminListingPageV2Schema,
  AdminListingV2Schema,
} from "./listing-v2";

const PAGE = "admin/consignment-listings-v2.json";
const DETAIL = "admin/consignment-listing-v2-detail.json";

test("Admin v2 夹具带有效止售、position 与运营动作", async () => {
  const { data: page } = await readResponse(PAGE, AdminListingPageV2Schema);
  const { data: detail } = await readResponse(DETAIL, AdminListingDetailV2Schema);
  expect(page.items.map(({ admin_actions }) => admin_actions)).toEqual([
    ["release_hold"],
    ["delist"],
  ]);
  expect(page.items.map(({ listing }) => listing.allowed_actions)).toEqual([[], []]);
  expect(detail.record.hold?.hold_id).toBe(page.items[0]?.hold?.hold_id ?? "");
  expect(detail.record.position).toMatchObject({ state: "in_custody", generation: 1 });
});

test("Admin 记录拒绝卖家动作、止售与动作不一致、处分代不一致", async () => {
  const { data: page } = await readResponse(PAGE, AdminListingPageV2Schema);
  const [held, free] = page.items;
  if (!held || !free) throw new Error("夹具须有止售项与可下架项");
  expect(decode(AdminListingV2Schema, held).ok).toBe(true);
  for (const invalid of [
    { ...held, listing: { ...held.listing, allowed_actions: ["withdraw"] } },
    { ...held, hold: null },
    // 止售只能出现在在库下架态：可下架的在库件带上止售且不宣告动作，只有本规则能拒绝。
    { ...free, hold: held.hold, admin_actions: [] },
    // 已取得（position 非 awaiting_take）的在库下架件不得没有处分代。
    {
      ...held,
      listing: { ...held.listing, generation: null },
      position: { ...held.position, generation: null },
    },
    { ...held, admin_actions: ["delist", "release_hold"] },
    { ...held, admin_actions: ["delist"] },
    { ...held, admin_actions: ["release_hold", "release_hold"] },
    { ...held, position: { ...held.position, generation: 2 } },
    { ...free, admin_actions: ["release_hold"] },
    { ...held, seller_user_id: 42 },
  ]) {
    expect(decode(AdminListingV2Schema, invalid).ok).toBe(false);
  }
});

test("Admin 详情碰撞上限与截断标记一致", async () => {
  const { data: detail } = await readResponse(DETAIL, AdminListingDetailV2Schema);
  expect(decode(AdminListingDetailV2Schema, { ...detail, conflicts_truncated: true }).ok).toBe(
    false,
  );
});
