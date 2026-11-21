// 公开详情的可见投影、路由身份切换和可取消重读测试。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createComponent, flush } from "solid-js";

import {
  detail,
  failure,
  fakeClock,
  fakeFetch,
  LISTING_A,
  LISTING_B,
  LISTINGS_PATH,
  mountConsignment,
  response,
  STATUS_PATH,
  settle,
  status,
} from "./read-fixture.test";
import { loadDetail } from "./render-fixture.test";

test("详情只显示公开安全投影，刷新收到 404 后清空装备与卖家", async () => {
  const { PublicDetail } = await loadDetail();
  let removed = false;
  const transport = fakeFetch(({ url }) =>
    url.pathname === STATUS_PATH
      ? response(status(true))
      : removed
        ? failure(404, "listing_not_found")
        : response(detail()),
  );
  const page = await mountConsignment(
    `/consignment/listings/${LISTING_A}`,
    () => createComponent(PublicDetail, { params: { listing_id: LISTING_A }, query: {} }),
    true,
  );
  try {
    await settle();
    expect(page.text()).toContain("Fake Oak Staff");
    expect(page.text()).toContain("Fake Seller");
    expect(page.text()).toContain("1,234");
    expect(page.text()).toContain("3.5");
    expect(page.text()).toContain("-2.25(0)");
    expect(page.text()).toContain("Contact the seller in game");
    expect(transport.requests.map(({ url }) => url.pathname)).toEqual([
      STATUS_PATH,
      `${LISTINGS_PATH}/${LISTING_A}`,
    ]);
    expect(transport.requests.every(({ init }) => init?.credentials === "omit")).toBe(true);

    removed = true;
    page.focus();
    await settle();
    expect(page.text()).toContain("This listing does not exist or is no longer displayed");
    expect(page.text()).not.toContain("Fake Oak Staff");
    expect(page.text()).not.toContain("Fake Seller");
    expect(page.find((node) => node.tag === "article")).toBeNull();
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("先后切换详情时取消旧请求并丢弃迟到结果", async () => {
  const { PublicDetail } = await loadDetail();
  const first = Promise.withResolvers<Response>();
  const second = Promise.withResolvers<Response>();
  const transport = fakeFetch(({ url }) =>
    url.pathname === STATUS_PATH
      ? response(status(true))
      : url.pathname.endsWith(LISTING_A)
        ? first.promise
        : second.promise,
  );
  const listingId = createCell(LISTING_A);
  const page = await mountConsignment(`/consignment/listings/${LISTING_A}`, () =>
    createComponent(PublicDetail, {
      get params() {
        return { listing_id: listingId() };
      },
      query: {},
    }),
  );
  try {
    await settle();
    expect(transport.requests[1]?.init?.signal?.aborted).toBe(false);
    listingId.set(LISTING_B);
    flush();
    await settle();
    expect(transport.requests[1]?.init?.signal?.aborted).toBe(true);
    expect(transport.requests[2]?.url.pathname).toBe(`${LISTINGS_PATH}/${LISTING_B}`);
    second.resolve(response(detail(LISTING_B)));
    await settle();
    expect(page.text()).toContain("Fake Iron Shield");
    expect(page.text()).toContain("does not yet hold the equipment");
    expect(page.text()).not.toContain("Contact the seller in game");

    first.resolve(response(detail(LISTING_A)));
    await settle();
    expect(page.text()).toContain("Fake Iron Shield");
    expect(page.text()).not.toContain("Fake Oak Staff");
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("详情每三十秒重读，隐藏停止定时器，离页取消在途请求", async () => {
  const { PublicDetail } = await loadDetail();
  const late = Promise.withResolvers<Response>();
  let reads = 0;
  const transport = fakeFetch(({ url }) => {
    if (url.pathname === STATUS_PATH) return response(status(true));
    reads += 1;
    return reads <= 2 ? response(detail()) : late.promise;
  });
  const clock = fakeClock();
  const page = await mountConsignment(`/consignment/listings/${LISTING_A}`, () =>
    createComponent(PublicDetail, { params: { listing_id: LISTING_A }, query: {} }),
  );
  try {
    await settle();
    expect(reads).toBe(1);
    await clock.advance(29_999);
    expect(reads).toBe(1);
    await clock.advance(1);
    expect(reads).toBe(2);

    page.setHidden(true);
    page.focus();
    await clock.advance(60_000);
    expect(reads).toBe(2);
    page.setHidden(false);
    await settle();
    expect(reads).toBe(3);
    const pending = transport.requests.at(-1);
    expect(pending?.init?.signal?.aborted).toBe(false);
    page.dispose();
    expect(pending?.init?.signal?.aborted).toBe(true);
    late.resolve(response(detail()));
    await clock.advance(60_000);
    expect(reads).toBe(3);
    expect(clock.activeIn(30_000)).toBe(0);
  } finally {
    page.dispose();
    clock.restore();
    transport.restore();
  }
});

test("等级判别与属性适配保留缺省、零基础值及安全等级文案", async () => {
  const { EquipmentPanel, consignmentStatLabel } = await loadDetail();
  const equipment = createCell(detail().equipment);
  const page = await mountConsignment("/consignment", () =>
    createComponent(EquipmentPanel, {
      get equipment() {
        return equipment();
      },
    }),
  );
  try {
    expect(page.text()).toContain("Level42");
    equipment.set({ ...detail().equipment, level: { kind: "unassigned" } });
    flush();
    expect(page.text()).toContain("Level—");
    equipment.set({ ...detail().equipment, level: { kind: "soulbound" }, tradeable: false });
    flush();
    expect(page.text()).toContain("LevelThis equipment cannot be traded");
    expect(consignmentStatLabel({ name: "Fake", value: 0.125, base: null })).toBe("0.125");
    expect(consignmentStatLabel({ name: "Fake", value: 1, base: 0 })).toBe("1(0)");
    expect(consignmentStatLabel({ name: "Fake", value: 1.25, base: 0.001 })).toBe("1.25(0.001)");
  } finally {
    page.dispose();
  }
});

test("已展示的详情在路由改用另一挂单后立即移除，直到新目标完成", async () => {
  const { PublicDetail } = await loadDetail();
  const pending = Promise.withResolvers<Response>();
  const transport = fakeFetch(({ url }) => {
    if (url.pathname === STATUS_PATH) return response(status(true));
    return url.pathname.endsWith(LISTING_A) ? response(detail()) : pending.promise;
  });
  const listingId = createCell(LISTING_A);
  const page = await mountConsignment(`/consignment/listings/${LISTING_A}`, () =>
    createComponent(PublicDetail, {
      get params() {
        return { listing_id: listingId() };
      },
      query: {},
    }),
  );
  try {
    await settle();
    expect(page.text()).toContain("Fake Oak Staff");
    listingId.set(LISTING_B);
    await settle();
    expect(page.text()).not.toContain("Fake Oak Staff");
    expect(page.text()).toContain("Loading");
    pending.resolve(response(detail(LISTING_B)));
    await settle();
    expect(page.text()).toContain("Fake Iron Shield");
  } finally {
    page.dispose();
    transport.restore();
  }
});
