// 结算页无效链接分支、目录定位与报告到达顺序。

import { expect, spyOn, test } from "bun:test";
import { AdminSettlementReportResponseSchema } from "@auction/contracts/dto/admin/settlement";
import { decode } from "@auction/contracts/wire";
import { requireOk } from "../../api/settlement-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { settle } from "../solid-dom-fixture";
import { reportForSelection, resolveCatalogSelection } from "./page-model";
import { withAdminTree } from "./settlement-tree-fixture.test";

const AUCTION_ID = "00000000-0000-0000-0000-000000000001";

function catalog() {
  const entry = {
    auctionId: AUCTION_ID,
    worldId: "main",
    sequence: 7,
    settledAt: "2026-11-14T09:05:00Z",
  };
  return { entry, page: { auctions: [entry], page: 1, pageSize: 10, total: 1 } };
}

test("duplicate_page_size_mounts_only_the_invalid_link_view", async () => {
  const { page } = await loadRenderModule<Pick<typeof import("./page"), "page">>(
    `${import.meta.dir}/page.tsx`,
    "page",
  );
  const pagination = await loadRenderModule<
    Pick<typeof import("../pagination"), "useAdminPagination">
  >(new URL("../pagination/index.tsx", import.meta.url).pathname, "useAdminPagination");
  const query = await import("./query");
  await withAdminTree("/settlement?sequence=7&page_size=10&page_size=20", async (tree) => {
    const normal = spyOn(pagination, "useAdminPagination");
    try {
      const read = spyOn(query, "readSettlementQuery");
      try {
        const requests = tree.requests.installFetch([]);
        const baseline = [...tree.fake.writes];
        tree.mount(page);
        await settle();
        const assertInvalidView = () => {
          expect(
            tree.elements((node) => node.tag === "h1").map((node) => node.textContent),
          ).toEqual(["结算报表"]);
          expect(
            tree
              .elements((node) => node.getAttribute("role") === "alert")
              .map((node) => node.textContent),
          ).toEqual(["此结算链接的参数无效，请核对地址后重试。"]);
        };
        assertInvalidView();
        expect(read).toHaveBeenCalledTimes(1);
        tree.fake.dispatchEvent(new Event("popstate"));
        await settle();
        expect(read).toHaveBeenCalledTimes(1);
        assertInvalidView();
        expect(
          tree.elements((node) => Object.keys(node.attributes).some((key) => key.startsWith("on"))),
        ).toEqual([]);
        expect(
          tree.elements((node) => ["button", "select", "table", "form"].includes(node.tag)),
        ).toEqual([]);
        expect(normal).not.toHaveBeenCalled();
        expect(requests).toEqual([]);
        expect(tree.fake.writes).toEqual(baseline);
        expect(tree.fake.location.search).toBe("?sequence=7&page_size=10&page_size=20");
      } finally {
        read.mockRestore();
      }
    } finally {
      normal.mockRestore();
    }
  });
});

test("deep_link_requires_both_fields_to_match", () => {
  const { page, entry } = catalog();
  expect(resolveCatalogSelection(page, null, AUCTION_ID, 7)).toEqual({ kind: "matched", entry });
  expect(resolveCatalogSelection(page, null, AUCTION_ID, 8)).toEqual({ kind: "contradictory" });
});

test("a_matching_lookup_resolves_only_after_the_directory_loads", () => {
  const { page: lookup, entry } = catalog();
  const page = {
    ...lookup,
    auctions: [
      {
        ...entry,
        auctionId: "00000000-0000-0000-0000-000000000002",
        worldId: "isekai",
      },
    ],
  };
  expect(resolveCatalogSelection(null, lookup, AUCTION_ID, 7)).toEqual({ kind: "resolving" });
  expect(resolveCatalogSelection(page, null, AUCTION_ID, 7)).toEqual({ kind: "resolving" });
  expect(resolveCatalogSelection(page, lookup, AUCTION_ID, 7)).toEqual({ kind: "matched", entry });
  expect(resolveCatalogSelection(page, page, AUCTION_ID, 7)).toEqual({ kind: "contradictory" });
});

test("a_report_arriving_first_can_wait_without_an_invalid_response", () => {
  const { entry } = catalog();
  const report = requireOk(
    decode(AdminSettlementReportResponseSchema, {
      auction: {
        id: AUCTION_ID,
        world_id: "main",
        sequence: 7,
        status: "settled",
        state_version: 2,
        preview_scheduled_at: "2026-11-13T09:00:00Z",
        scheduled_open_at: "2026-11-14T09:00:00Z",
        preview_manual_open: false,
        preview_revision: 0,
        created_at: "2026-11-13T08:00:00Z",
        updated_at: "2026-11-14T09:05:00Z",
      },
      settlement: null,
      lots: [],
    }),
  );
  expect(reportForSelection({ kind: "resolving" }, report)).toEqual({ ok: true, value: null });
  expect(reportForSelection({ kind: "matched", entry }, report)).toEqual({
    ok: true,
    value: report,
  });
  expect(
    reportForSelection(
      { kind: "matched", entry },
      { ...report, auction: { ...report.auction, sequence: 8 } },
    ),
  ).toEqual({
    ok: false,
    error: "invalid_response",
  });
});
