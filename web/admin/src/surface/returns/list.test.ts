// 经由真实列表页的下拉框与表格验证筛选闭集和状态徽标。

import { expect, test } from "bun:test";
import { returnPage, returnSummary } from "../../api/returns-fixture";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { requireElement, withAdminTree } from "../settlement/settlement-tree-fixture.test";
import { settle } from "../solid-dom-fixture";

const LIST_PATH = "/api/v1/admin/settlement-returns";
const CLOSED_AT = "2026-11-16T02:00:00Z";

function loadList() {
  return loadRenderModule<typeof import("./list")>(`${import.meta.dir}/list.tsx`, "page");
}

function reply(data: ReturnType<typeof returnPage>) {
  return { body: JSON.stringify({ code: "ok", message: "", data }) };
}

test("filter_choices_stay_in_the_closed_set", async () => {
  const { page } = await loadList();
  await withAdminTree("/settlement-returns?scope=all&world=main&page_size=20", async (tree) => {
    const empty = reply(returnPage([], 0));
    const requests = tree.requests.installFetch([empty, empty, empty, empty]);
    tree.mount(page);
    await settle();

    function filter(option: string) {
      return requireElement(
        tree.find(
          (node) =>
            node.tag === "select" &&
            node.querySelectorAll("option").some((choice) => choice.value === option),
        ),
        option,
      );
    }

    const status = filter("held");
    const world = filter("isekai");
    expect(typeof status.attributes.onChange).toBe("function");
    expect(typeof world.attributes.onChange).toBe("function");
    status.change("held");
    await settle();
    expect(tree.fake.location.searchParams.get("status")).toBe("held");
    expect(filter("held")).toBe(status);
    expect(status.value).toBe("held");

    status.change("open");
    await settle();
    expect(tree.fake.location.searchParams.has("status")).toBe(false);
    expect(filter("held")).toBe(status);
    expect(status.value).toBe("");

    expect(world.value).toBe("main");
    world.change("");
    await settle();
    expect(tree.fake.location.searchParams.has("world")).toBe(false);
    expect(filter("isekai")).toBe(world);
    expect(world.value).toBe("");
    expect(requests).toEqual([
      { method: "GET", url: `${LIST_PATH}?page=1&page_size=20&world=main`, body: "" },
      { method: "GET", url: `${LIST_PATH}?page=1&page_size=20&status=held&world=main`, body: "" },
      { method: "GET", url: `${LIST_PATH}?page=1&page_size=20&world=main`, body: "" },
      { method: "GET", url: `${LIST_PATH}?page=1&page_size=20`, body: "" },
    ]);
  });
});

test("status_badges_separate_review_pending_and_closed", async () => {
  const { page } = await loadList();
  const examples = [
    { status: "quarantined", closed_at: null, suffix: "--error" },
    { status: "take_unknown", closed_at: null, suffix: "--error" },
    { status: "awaiting_decision", closed_at: null, suffix: "--warn" },
    { status: "recheck_pending", closed_at: null, suffix: "--warn" },
    { status: "take_pending", closed_at: null, suffix: "--warn" },
    { status: "resend_pending", closed_at: null, suffix: "--warn" },
    { status: "held", closed_at: CLOSED_AT, suffix: "--neutral" },
    { status: "resent", closed_at: CLOSED_AT, suffix: "--neutral" },
    { status: "inbox_closed", closed_at: CLOSED_AT, suffix: "--neutral" },
    { status: "reviewed", closed_at: CLOSED_AT, suffix: "--neutral" },
  ];
  const items = examples.map(({ status, closed_at }, index) => ({
    ...returnSummary(),
    return_id: `00000000-0000-4000-8000-00000000001${index}`,
    status,
    closed_at,
  }));
  await withAdminTree("/settlement-returns?scope=all&page_size=20", async (tree) => {
    const requests = tree.requests.installFetch([reply(returnPage(items, 10))]);
    tree.mount(page);
    await settle();
    expect(requests).toEqual([
      { method: "GET", url: `${LIST_PATH}?page=1&page_size=20`, body: "" },
    ]);
    for (
      let round = 0;
      round < 4 && tree.find((node) => node.tag === "tbody") === null;
      round += 1
    ) {
      await settle();
    }
    const tbody = requireElement(
      tree.find((node) => node.tag === "tbody"),
      "退件表体",
    );
    const rows = tbody.querySelectorAll("tr");
    expect(rows).toHaveLength(10);
    for (const [index, { status, suffix }] of examples.entries()) {
      const row = requireElement(rows[index] ?? null, status);
      const badge = requireElement(row.querySelector(".admin-dashboard-badge"), status);
      expect(badge.getAttribute("class")?.endsWith(suffix), status).toBe(true);
      expect(row.querySelector("a")?.getAttribute("href")).toBe(
        `/settlement-returns/00000000-0000-4000-8000-00000000001${index}`,
      );
    }
  });
});
