// 从真实 Solid 列表树检查筛选、分页和无快照的私有行。

import { expect, test } from "bun:test";
import { compileRoutes, resolveRoute } from "@auction/web-shared/router/pattern";

import { loadRenderModule } from "../../solid-render-fixture.test";
import { ADMIN_ROUTES, consignmentListingPage, consignmentPage } from "../router/route";
import { requireElement, withAdminTree } from "../settlement/settlement-tree-fixture.test";
import { settle } from "../solid-dom-fixture";
import {
  adminListing,
  CONSIGNED_QUOTE,
  DETAIL_PAGE,
  LISTING_ID,
  LISTINGS,
  SELLER_ID,
  success,
} from "./fixtures";

function loadList() {
  return loadRenderModule<typeof import("./list")>(`${import.meta.dir}/list.tsx`, "page");
}

test("世界、模式、有效状态和精确卖家写入 URL；无效卖家草稿不导航", async () => {
  const { page } = await loadList();
  await withAdminTree("/consignment?page=3&page_size=20", async (tree) => {
    const empty = success({ items: [], total: 0, page: 1, page_size: 20 });
    const first = success({ items: [], total: 100, page: 3, page_size: 20 });
    const requests = tree.requests.installFetch([first, empty, empty, empty, empty, empty]);
    tree.mount(page);
    await settle();
    for (const [key, choice] of [
      ["world", "isekai"],
      ["mode", "consigned"],
      ["state", "closed"],
    ]) {
      if (key === undefined || choice === undefined) throw new Error("筛选夹具缺字段");
      const input = requireElement(
        tree.find(
          (node) =>
            node.tag === "select" &&
            node.querySelectorAll("option").some((opt) => opt.value === choice),
        ),
        key,
      );
      input.change(choice);
      await settle();
      expect(tree.fake.location.searchParams.get(key)).toBe(choice);
      expect(tree.fake.location.searchParams.has("page")).toBe(false);
      expect(requests.at(-1)?.url).toContain(`${key}=${choice}`);
    }
    const seller = requireElement(
      tree.find((node) => node.id === "consignment-seller-id"),
      "卖家输入",
    );
    const form = requireElement(
      tree.find(
        (node) =>
          node.tag === "form" && node.getAttribute("class") === "admin-consignment-filters__seller",
      ),
      "卖家筛选表单",
    );
    seller.input("partial");
    form.submit();
    await settle();
    expect(requests).toHaveLength(4);
    expect(tree.fake.location.searchParams.has("seller_user_id")).toBe(false);
    expect(seller.getAttribute("aria-invalid")).toBe("true");
    seller.input(SELLER_ID);
    form.submit();
    await settle();
    expect(tree.fake.location.searchParams.get("seller_user_id")).toBe(SELLER_ID);
    expect(requests.at(-1)?.url).toBe(
      `${LISTINGS}?page=1&page_size=20&world=isekai&mode=consigned&state=closed&seller_user_id=${SELLER_ID}`,
    );
    seller.input("");
    form.submit();
    await settle();
    expect(tree.fake.location.searchParams.has("seller_user_id")).toBe(false);
    expect(requests.at(-1)?.url).not.toContain("seller_user_id");
  });
});

test("翻页与自填 37 保留 URL 过滤；无快照行显示长装备号、卖家、标价及解析进度", async () => {
  const { page } = await loadList();
  await withAdminTree("/consignment?mode=direct&page_size=20", async (tree) => {
    const items = [adminListing()];
    const requests = tree.requests.installFetch([
      success({ items, total: 100, page: 1, page_size: 20 }),
      success({ items, total: 100, page: 2, page_size: 20 }),
      success({ items, total: 100, page: 1, page_size: 37 }),
    ]);
    tree.mount(page);
    await settle();
    const row = requireElement(
      tree.find((node) => node.tag === "tr" && node.querySelector("a") !== null),
      "无快照行",
    );
    expect(row.textContent).toContain("Fixture seller");
    expect(row.textContent).toContain("1,001");
    expect(row.textContent).toContain("解析中");
    expect(row.textContent).toContain("2/6");
    expect(row.textContent).toContain("外部读取已暂停");
    expect(row.textContent).toContain("读取装备超时");
    expect(row.textContent).not.toContain("Fixture sword");
    const link = requireElement(row.querySelector("a"), "装备号链接");
    expect(link.textContent).toBe("987654321098765432109876543210");
    expect(link.getAttribute("href")).toBe(DETAIL_PAGE);
    expect(row.querySelector("time")?.getAttribute("datetime")).toBe("2026-11-18T01:00:00Z");
    expect(row.querySelectorAll("time").at(-1)?.getAttribute("datetime")).toBe(
      "2026-11-18T00:15:00Z",
    );

    requireElement(
      tree.find((node) => node.tag === "button" && node.getAttribute("aria-label") === "下一页"),
      "下一页",
    ).click();
    await settle();
    expect(tree.fake.location.searchParams.get("page")).toBe("2");
    const size = requireElement(
      tree.find(
        (node) =>
          node.tag === "form" && node.getAttribute("class")?.includes("ui-pager__custom") === true,
      ),
      "自定义页长",
    );
    const input = requireElement(size.querySelector("input"), "页长输入");
    input.input("201");
    size.submit();
    await settle();
    expect(requests).toHaveLength(2);
    input.input("37");
    size.submit();
    await settle();
    expect(tree.fake.location.searchParams.get("page_size")).toBe("37");
    expect(tree.fake.location.searchParams.has("page")).toBe(false);
    expect(tree.fake.location.searchParams.get("mode")).toBe("direct");
    expect(requests.map((request) => request.url)).toEqual([
      `${LISTINGS}?page=1&page_size=20&mode=direct`,
      `${LISTINGS}?page=2&page_size=20&mode=direct`,
      `${LISTINGS}?page=1&page_size=37&mode=direct`,
    ]);
  });
});

test("重复筛选或无效的卖家书签失败关闭", async () => {
  const { page } = await loadList();
  for (const query of ["state=closed&state=resolving", "seller_user_id=partial"]) {
    await withAdminTree(`/consignment?${query}&page_size=20`, async (tree) => {
      const requests = tree.requests.installFetch([]);
      tree.mount(page);
      await settle();
      expect(
        tree.find((node) => node.textContent === "请求参数无效。请检查输入内容后重试。"),
      ).not.toBeNull();
      expect(requests).toEqual([]);
    });
  }
});

test("模式列逐行区分非寄售与寄售", async () => {
  const { page } = await loadList();
  await withAdminTree("/consignment?page_size=20", async (tree) => {
    const items = [
      adminListing({ external_equipment_id: "42" }),
      adminListing({
        ...CONSIGNED_QUOTE,
        listing_id: "10000000-0000-4000-8000-000000000004",
        external_equipment_id: "43",
      }),
    ];
    tree.requests.installFetch([success({ items, total: 2, page: 1, page_size: 20 })]);
    tree.mount(page);
    await settle();
    const rows = tree.elements((node) => node.tag === "tr" && node.querySelector("a") !== null);
    expect(
      rows.map((row) => [
        row.querySelector("a")?.textContent,
        row.querySelectorAll("td").find((cell) => cell.getAttribute("data-label") === "挂单模式")
          ?.textContent,
      ]),
    ).toEqual([
      ["42", "非寄售"],
      ["43", "寄售"],
    ]);
  });
});

test("列表与详情可直接匹配，认证后的详情高亮一口价入口", async () => {
  const routes = compileRoutes(ADMIN_ROUTES);
  const index = resolveRoute(routes, "/consignment?state=closed");
  const detail = resolveRoute(routes, DETAIL_PAGE);
  expect(index.match).toMatchObject({ kind: "matched", name: "Consignment" });
  expect(detail.match).toMatchObject({
    kind: "matched",
    name: "ConsignmentListing",
    params: { listing_id: LISTING_ID },
  });
  expect(resolveRoute(routes, `${DETAIL_PAGE}/unknown`).match.kind).toBe("not-found");
  expect(consignmentPage.kind).toBe("lazy");
  expect(consignmentListingPage.kind).toBe("lazy");

  const { NavigationLinks } = await loadRenderModule<typeof import("../auth/navigation")>(
    new URL("../auth/navigation/index.tsx", import.meta.url).pathname,
    "NavigationLinks",
  );
  await withAdminTree(DETAIL_PAGE, (tree) => {
    tree.mount(() => NavigationLinks({ currentRoute: detail.match }));
    const link = requireElement(
      tree.find((node) => node.tag === "a" && node.getAttribute("href") === "/consignment"),
      "一口价入口",
    );
    expect(link.textContent).toBe("一口价挂单管理");
    expect(link.getAttribute("aria-current")).toBe("page");
  });
});
