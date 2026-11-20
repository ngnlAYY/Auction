// 对照 Rust 拍品列表窄屏排序的选项、方向及桌面表头 ARIA 语义。

import { expect, test } from "bun:test";
import { AdminAuctionResponseSchema } from "@auction/contracts/dto/admin/settlement";
import { decode } from "@auction/contracts/wire";
import { createCell } from "@auction/web-shared/reactive/cell";
import { success } from "../../api/auction-contract-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { auctionWire } from "../auctions/actions/auction-actions-fixture.test";
import { type EditorTest, present, withEditor } from "../auctions/editor-fixture.test";
import { createComponent, visit } from "../solid-dom-fixture";
import type { LotEdit } from "./edit";
import type { LotUpdateNotice } from "./state";

async function withSort(sort: string, run: (page: EditorTest) => void): Promise<void> {
  const [table, paging] = await Promise.all([
    loadRenderModule<typeof import("./table")>(`${import.meta.dir}/table.tsx`, "AdminLotTable"),
    loadRenderModule<typeof import("../pagination")>(
      `${import.meta.dir}/../pagination/index.tsx`,
      "useAdminPagination",
    ),
  ]);
  const auction = success(decode(AdminAuctionResponseSchema, auctionWire()));
  await withEditor(
    ({ reload }) =>
      createComponent(table.AdminLotTable, {
        page: {
          auction,
          lots: [],
          pagination: { page: 1, page_size: 50, total: 0 },
          capabilities: { manualIntake: false },
        },
        pagination: paging.useAdminPagination("lots", ["page_size"], 100, 100_000),
        sort,
        direction: "desc",
        editsReady: true,
        onPageChange() {},
        onSortChange() {},
        reloadGeneration: reload,
        draft: createCell<LotEdit | null>(null),
        notice: createCell<LotUpdateNotice | null>(null),
      }),
    run,
  );
}

function mobile(page: EditorTest) {
  const root = present(page.find((node) => node.matches(".admin-mobile-sort"))[0], "移动端排序区");
  const selector = present(root.querySelector("select"), "排序选择器");
  const direction = present(root.querySelector("button"), "排序方向按钮");
  return { root, selector, direction };
}

function ariaSortNodes(page: EditorTest) {
  return page.find((node) => node.hasAttribute("aria-sort"));
}

test("mobile_sort_exposes_the_four_columns_and_active_direction", () =>
  withSort("current_price", (page) => {
    const { root, selector, direction } = mobile(page);
    expect(typeof selector.attributes.onChange).toBe("function");
    const options = selector.querySelectorAll("option");
    expect(
      new Set(options.filter((option) => option.value !== "").map((option) => option.value)),
    ).toEqual(new Set(["code", "status", "start_price", "current_price"]));
    expect(selector.value).toBe("current_price");
    expect(direction.getAttribute("aria-label")).toContain("当前为降序");
    const sorted = ariaSortNodes(page);
    expect(sorted.map((node) => node.getAttribute("aria-sort"))).toEqual(["descending"]);
    const [currentPrice] = page.find(
      (node) => node.tag === "th" && node.matches(".admin-auction-lot-current"),
    );
    expect(sorted[0]).toBe(present(currentPrice, "当前价表头"));
    const headers = page.find((node) => node.tag === "thead");
    expect(headers).toHaveLength(1);
    visit([root, ...headers], (node) => {
      expect(node.hasAttribute("disabled")).toBe(false);
    });
  }));

test("default_mobile_order_has_no_enabled_direction", () =>
  withSort("", (page) => {
    const { selector, direction } = mobile(page);
    expect(typeof direction.attributes.onClick).toBe("function");
    expect(direction.hasAttribute("disabled")).toBe(true);
    expect(
      present(
        selector.querySelectorAll("option").find((option) => option.value === ""),
        "默认顺序选项",
      ).textContent,
    ).toBe("默认顺序");
    const label = direction.getAttribute("aria-label");
    expect(present(label, "默认方向按钮的 aria-label")).not.toMatch(/升序|降序/u);
    expect(ariaSortNodes(page)).toEqual([]);
  }));
