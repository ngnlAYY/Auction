// 假传输驱动真实运营详情和会话命令宿主，验证只读与下架的完整流程。

import { expect, test } from "bun:test";
import type { ListingReason, ListingView } from "@auction/contracts/dto/public/consignment";

import {
  installFixtureFetch,
  parseJsonObject,
  recordRequest,
  TEST_SESSION_BODY,
} from "../../api/api-request-fixture";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { requireElement, withAdminTree } from "../settlement/settlement-tree-fixture.test";
import { settle } from "../solid-dom-fixture";
import {
  adminDetail,
  adminListing,
  DELIST,
  DETAIL,
  DETAIL_PAGE,
  delistedListing,
  delistReceipt,
  failure,
  LISTING_ID,
  SNAPSHOT,
  success,
} from "./fixtures";

type Tree = Parameters<Parameters<typeof withAdminTree>[1]>[0];
const REASON = "Reviewed by operations";
const SESSION = "/api/v1/admin/session";

function withDetail(run: (tree: Tree, mount: () => void) => Promise<void>): Promise<void> {
  return withAdminTree(DETAIL_PAGE, async (tree) => {
    const { detailPage } = await loadRenderModule<typeof import("./detail")>(
      `${import.meta.dir}/detail.tsx`,
      "detailPage",
    );
    await run(tree, () =>
      tree.mount(() => detailPage({ params: { listing_id: LISTING_ID }, query: {} })),
    );
  });
}

function findButton(tree: Tree, text: string) {
  return tree.find((node) => node.tag === "button" && node.textContent === text);
}

function present(tree: Tree, text: string): boolean {
  return tree.find((node) => node.textContent === text) !== null;
}

function dialogForm(tree: Tree) {
  const dialog = requireElement(
    tree.find((node) => node.getAttribute("role") === "dialog"),
    "下架确认框",
  );
  return {
    form: requireElement(dialog.querySelector("form"), "下架表单"),
    reason: requireElement(dialog.querySelector("textarea"), "下架说明"),
  };
}

function confirm(tree: Tree, reason = REASON): void {
  requireElement(findButton(tree, "下架"), "下架").click();
  const dialog = dialogForm(tree);
  dialog.reason.input(reason);
  dialog.form.submit();
}

function address(request: ReturnType<typeof recordRequest>): string {
  return new URL(request.url, "https://auction.test").pathname;
}

function emit(reply: ReturnType<typeof success>): Response {
  return new Response(reply.body, { status: reply.status ?? 200 });
}

function receiptFor(body: string): Response {
  const commandId = parseJsonObject(body, "内存下架命令").command_id;
  if (typeof commandId !== "string") throw new Error("内存下架命令没有 ID");
  return emit(success(delistReceipt(commandId)));
}

test("已记录与有效状态同时显示；乱序 lot 原样、50 条截断标志由服务端控制", async () => {
  await withDetail(async (tree, mount) => {
    const record = adminListing(
      {
        stored_state: "displayed",
        effective_state: "closed",
        effective_reason: "auction_lot_created",
        view: { kind: "closed", reason: "auction_lot_created", equipment: SNAPSHOT },
      },
      { can_delist: false },
    );
    const conflicts = Array.from({ length: 50 }, (_, index) => ({
      lot_id: `a0000000-0000-4000-8000-${String(index + 1).padStart(12, "0")}`,
      detected_at: index % 2 === 0 ? "2026-11-18T00:00:00Z" : "2026-11-19T00:00:00Z",
    }));
    const requests = tree.requests.installFetch([
      success(adminDetail(record, { conflicts, conflicts_truncated: true })),
      success(adminDetail(record, { conflicts })),
    ]);
    mount();
    await settle();
    expect(present(tree, "已记录状态")).toBe(true);
    expect(present(tree, "当前有效状态")).toBe(true);
    expect(present(tree, "已展示")).toBe(true);
    expect(present(tree, "装备已进入拍卖；经确认取回后可重新登记。")).toBe(true);
    expect(
      tree.find((node) => node.tag === "dd" && node.textContent.startsWith("已关闭")),
    ).not.toBeNull();
    expect(present(tree, "Fixture sword")).toBe(true);
    expect(present(tree, "Fixture blade")).toBe(true);
    expect(present(tree, "Sword · 123")).toBe(true);
    expect(present(tree, "1.4 (0)")).toBe(true);
    expect(
      tree.find(
        (node) => node.tag === "time" && node.getAttribute("datetime") === SNAPSHOT.observed_at,
      ),
    ).not.toBeNull();
    const rendered = tree.elements((node) => node.tag === "code").map((node) => node.textContent);
    expect(rendered).toEqual(conflicts.map((conflict) => conflict.lot_id));
    expect(present(tree, "仅显示最近 50 条")).toBe(true);
    expect(findButton(tree, "下架")).toBeNull();

    requireElement(findButton(tree, "刷新记录"), "刷新记录").click();
    await settle();
    expect(present(tree, "仅显示最近 50 条")).toBe(false);
    expect(tree.elements((node) => node.tag === "code")).toHaveLength(50);
    expect(requests.map(address)).toEqual([DETAIL, DETAIL]);
  });
});

const REASONS: readonly {
  readonly reason: ListingReason;
  readonly state: "rejected" | "expired" | "closed";
  readonly view: ListingView;
  readonly text: string;
}[] = [
  {
    reason: "history_owner_mismatch",
    state: "rejected",
    view: {
      kind: "ineligible",
      reason: "history_owner_mismatch",
      equipment: SNAPSHOT,
    },
    text: "本站最后已证持有人与登记人不符，第一版不接受平台外转手的装备。",
  },
  {
    reason: "lookup_exhausted",
    state: "rejected",
    view: {
      kind: "lookup_failed",
      reason: "lookup_exhausted",
      last_lookup_reason: "source_rate_limited",
    },
    text: "解析尝试次数已用完",
  },
  {
    reason: "display_expired",
    state: "expired",
    view: {
      kind: "expired",
      reason: "display_expired",
      equipment: SNAPSHOT,
    },
    text: "展示期限已到",
  },
  {
    reason: "service_disabled",
    state: "closed",
    view: {
      kind: "closed",
      reason: "service_disabled",
      equipment: null,
    },
    text: "服务停用，登记已关闭",
  },
];

for (const scenario of REASONS) {
  test(`终态 ${scenario.reason} 显示明确原因且只读`, async () => {
    await withDetail(async (tree, mount) => {
      const record = adminListing(
        {
          stored_state: scenario.state,
          effective_state: scenario.state,
          effective_reason: scenario.reason,
          view: scenario.view,
        },
        { can_delist: false },
      );
      const requests = tree.requests.installFetch([success(adminDetail(record))]);
      mount();
      await settle();
      expect(present(tree, scenario.text)).toBe(true);
      expect(findButton(tree, "下架")).toBeNull();
      expect(present(tree, "Fixture sword")).toBe(
        "equipment" in scenario.view && scenario.view.equipment !== null,
      );
      expect(present(tree, "装备来源正在限流")).toBe(scenario.reason === "lookup_exhausted");
      expect(tree.elements((node) => node.tag === "form")).toHaveLength(0);
      expect(requests.map(address)).toEqual([DETAIL]);
    });
  });
}

test("有效活动但 can_delist=false 时仍没有下架入口", async () => {
  await withDetail(async (tree, mount) => {
    tree.requests.installFetch([success(adminDetail(adminListing({}, { can_delist: false })))]);
    mount();
    await settle();
    expect(present(tree, "外部读取已暂停")).toBe(true);
    expect(findButton(tree, "下架")).toBeNull();
    expect(tree.find((node) => node.tag === "textarea")).toBeNull();
  });
});

test("下架表单拒绝空、超出 UTF-8 字节上限、换行和装备链接", async () => {
  await withDetail(async (tree, mount) => {
    const requests = tree.requests.installFetch([success(adminDetail(adminListing()))]);
    mount();
    await settle();
    requireElement(findButton(tree, "下架"), "下架").click();
    const dialog = dialogForm(tree);
    dialog.form.submit();
    await settle();
    expect(present(tree, "请填写下架原因")).toBe(true);
    for (const reason of [
      "审".repeat(86),
      "https://example.invalid/equip/42/FIXTURE",
      "一行\n另一行",
    ]) {
      dialog.reason.input(reason);
      dialog.form.submit();
      await settle();
      expect(present(tree, "请求参数无效。请检查输入内容后重试。")).toBe(true);
      expect(dialog.reason.getAttribute("aria-invalid")).toBe("true");
      expect(requests.map(address)).toEqual([DETAIL]);
    }
    requireElement(findButton(tree, "取消"), "取消").click();
    expect(tree.find((node) => node.getAttribute("role") === "dialog")).toBeNull();
    requireElement(findButton(tree, "下架"), "下架").click();
    expect(dialogForm(tree).reason.value).toBe("");
  });
});

for (const outcome of ["success", "stale_version", "listing_not_active"] as const) {
  test(`下架 ${outcome} 重读详情；按新版本重新确认或保持只读`, async () => {
    await withDetail(async (tree, mount) => {
      tree.requests.installBrowserRequest();
      const calls: string[] = [];
      const posts: string[] = [];
      let reads = 0;
      const restore = installFixtureFetch(async (input, init) => {
        const request = recordRequest(input, init);
        const url = address(request);
        calls.push(url);
        switch (url) {
          case DETAIL:
            reads += 1;
            if (reads === 1) return emit(success(adminDetail(adminListing())));
            if (outcome === "stale_version" && reads === 2) {
              return emit(success(adminDetail(adminListing({ version: 4 }))));
            }
            return emit(success(adminDetail(delistedListing(REASON))));
          case SESSION:
            return Response.json(TEST_SESSION_BODY);
          case DELIST:
            posts.push(request.body);
            if (outcome !== "success" && posts.length === 1) return emit(failure(409, outcome));
            if (outcome === "stale_version") {
              const commandId = parseJsonObject(request.body, "内存命令").command_id;
              if (typeof commandId !== "string") throw new Error("缺少命令 ID");
              return emit(success(delistReceipt(commandId, 5)));
            }
            return receiptFor(request.body);
          default:
            throw new Error("未预期的内存请求");
        }
      });
      try {
        mount();
        await settle();
        confirm(tree);
        await settle();
        expect(calls).toEqual([DETAIL, SESSION, DELIST, DETAIL]);
        const first = parseJsonObject(posts[0] ?? "", "首次命令");
        expect(first).toEqual({
          command_id: expect.stringMatching(/^[0-9a-f-]{36}$/u),
          expected_version: 3,
          reason: REASON,
        });
        expect(findButton(tree, "原样重试")).toBeNull();
        expect(tree.find((node) => node.getAttribute("role") === "dialog")).toBeNull();
        if (outcome === "stale_version") {
          expect(present(tree, "挂单已变化，请刷新后确认")).toBe(true);
          expect(posts).toHaveLength(1);
          requireElement(findButton(tree, "下架"), "新版本下架").click();
          const dialog = dialogForm(tree);
          expect(dialog.reason.value).toBe("");
          dialog.reason.input(REASON);
          dialog.form.submit();
          await settle();
          expect(calls).toEqual([DETAIL, SESSION, DELIST, DETAIL, SESSION, DELIST, DETAIL]);
          const second = parseJsonObject(posts[1] ?? "", "重新确认命令");
          expect(second.expected_version).toBe(4);
          expect(second.command_id).not.toBe(first.command_id);
        }
        if (outcome === "listing_not_active")
          expect(present(tree, "挂单已停止接受操作")).toBe(true);
        expect(present(tree, REASON)).toBe(true);
        expect(findButton(tree, "下架")).toBeNull();
        expect(present(tree, "Fixture server message")).toBe(false);
      } finally {
        restore();
      }
    });
  });
}

for (const unknown of ["timeout", "outcome_unknown"] as const) {
  test(`${unknown} 后只允许同一 ID 和字节完全相同的显式重试`, async () => {
    await withDetail(async (tree, mount) => {
      tree.requests.installBrowserRequest();
      const posts: string[] = [];
      let reads = 0;
      let reviews = 0;
      const restore = installFixtureFetch(async (input, init) => {
        const request = recordRequest(input, init);
        switch (address(request)) {
          case DETAIL:
            reads += 1;
            return emit(
              success(adminDetail(reads === 1 ? adminListing() : delistedListing(REASON))),
            );
          case SESSION:
            reviews += 1;
            return Response.json(TEST_SESSION_BODY);
          case DELIST:
            posts.push(request.body);
            if (posts.length === 1) {
              if (unknown === "timeout") throw new TypeError("内存传输中断");
              return emit(failure(503, "outcome_unknown"));
            }
            return receiptFor(request.body);
          default:
            throw new Error("未预期的内存请求");
        }
      });
      try {
        mount();
        await settle();
        confirm(tree);
        await settle();
        expect(reads).toBe(1);
        expect(posts).toHaveLength(1);
        const action = requireElement(findButton(tree, "下架"), "禁用的下架");
        expect(action.disabled).toBe(true);
        action.dispatchClick();
        expect(tree.find((node) => node.getAttribute("role") === "dialog")).toBeNull();
        requireElement(findButton(tree, "原样重试"), "原样重试").click();
        await settle();
        expect(posts).toHaveLength(2);
        expect(posts[1]).toBe(posts[0]);
        expect(reviews).toBe(2);
        expect(reads).toBe(2);
        expect(present(tree, REASON)).toBe(true);
        expect(findButton(tree, "原样重试")).toBeNull();
      } finally {
        restore();
      }
    });
  });
}

for (const code of ["unauthenticated", "csrf_invalid"] as const) {
  test(`下架的 ${code} 沿用会话确认及安全错误提示`, async () => {
    await withDetail(async (tree, mount) => {
      tree.requests.installBrowserRequest();
      const ready = success(adminDetail(adminListing()));
      const requests = tree.requests.installFetch(
        code === "unauthenticated"
          ? [ready, failure(401, code), ready]
          : [ready, { body: JSON.stringify(TEST_SESSION_BODY) }, failure(403, code), ready],
      );
      mount();
      await settle();
      confirm(tree);
      await settle();
      expect(requests.map(address)).toEqual(
        code === "unauthenticated" ? [DETAIL, SESSION, DETAIL] : [DETAIL, SESSION, DELIST, DETAIL],
      );
      expect(
        present(
          tree,
          code === "unauthenticated"
            ? "管理会话已过期。请重新登录后再试。"
            : "当前操作不被允许。请重新加载页面并核对权限。",
        ),
      ).toBe(true);
      expect(findButton(tree, "原样重试")).toBeNull();
    });
  });
}

test("首次依赖 503 可结束本次尝试；已有未知结果时 503 与 CSRF 拒绝均不能解除原命令", async () => {
  for (const priorUnknown of [false, true]) {
    await withDetail(async (tree, mount) => {
      tree.requests.installBrowserRequest();
      const current = success(adminDetail(adminListing()));
      const session = { body: JSON.stringify(TEST_SESSION_BODY) };
      const unavailable = failure(503, "consignment_unavailable");
      const requests = tree.requests.installFetch(
        priorUnknown
          ? [
              current,
              session,
              failure(503, "outcome_unknown"),
              session,
              unavailable,
              session,
              failure(403, "csrf_invalid"),
            ]
          : [current, session, unavailable, current],
      );
      mount();
      await settle();
      confirm(tree);
      await settle();
      if (!priorUnknown) {
        expect(requests.map(address)).toEqual([DETAIL, SESSION, DELIST, DETAIL]);
        expect(present(tree, "所需服务暂时不可用。请稍后再试。")).toBe(true);
        expect(findButton(tree, "原样重试")).toBeNull();
        return;
      }
      for (let attempt = 0; attempt < 2; attempt += 1) {
        requireElement(findButton(tree, "原样重试"), "原样重试").click();
        await settle();
        expect(requireElement(findButton(tree, "下架"), "下架").disabled).toBe(true);
      }
      const posts = requests.filter((request) => address(request) === DELIST);
      expect(posts).toHaveLength(3);
      expect(posts[1]).toEqual(posts[0]);
      expect(posts[2]).toEqual(posts[0]);
      expect(requests.filter((request) => address(request) === DETAIL)).toHaveLength(1);
      expect(findButton(tree, "原样重试")).not.toBeNull();
    });
  }
});

test("旧未知结果不会被错误状态或非空错误 data 的 409 了结", async () => {
  await withDetail(async (tree, mount) => {
    tree.requests.installBrowserRequest();
    const session = { body: JSON.stringify(TEST_SESSION_BODY) };
    const wrongData = failure(409, "stale_version");
    const requests = tree.requests.installFetch([
      success(adminDetail(adminListing())),
      session,
      failure(503, "outcome_unknown"),
      session,
      failure(400, "stale_version"),
      session,
      { ...wrongData, body: wrongData.body.replace('"data":null', '"data":{}') },
    ]);
    mount();
    await settle();
    confirm(tree);
    await settle();
    for (let attempt = 0; attempt < 2; attempt += 1) {
      requireElement(findButton(tree, "原样重试"), "原样重试").click();
      await settle();
      expect(requireElement(findButton(tree, "下架"), "下架").disabled).toBe(true);
    }
    expect(requests.map(address)).toEqual([
      DETAIL,
      SESSION,
      DELIST,
      SESSION,
      DELIST,
      SESSION,
      DELIST,
    ]);
    expect(findButton(tree, "原样重试")).not.toBeNull();
  });
});
