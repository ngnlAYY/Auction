// 假传输驱动真实运营详情，验证解除止售绑定 hold_id 与预期版本，回执不符只能原样重放。

import { expect, test } from "bun:test";

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
  DETAIL,
  DETAIL_PAGE,
  HOLD_ID,
  heldListing,
  holdReleasedReceipt,
  LISTING_ID,
  RELEASE_HOLD,
  success,
} from "./fixtures";

type Tree = Parameters<Parameters<typeof withAdminTree>[1]>[0];
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

function button(tree: Pick<Tree, "find">, text: string) {
  return tree.find((node) => node.tag === "button" && node.textContent === text);
}

function path(request: ReturnType<typeof recordRequest>): string {
  return new URL(request.url, "https://auction.test").pathname;
}

function reply(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status });
}

function commandId(body: string): string {
  const id = parseJsonObject(body, "解除止售命令").command_id;
  if (typeof id !== "string") throw new Error("内存命令没有 ID");
  return id;
}

function confirmRelease(tree: Tree): void {
  requireElement(button(tree, "解除止售"), "解除止售").click();
  const dialog = requireElement(
    tree.find((node) => node.getAttribute("role") === "dialog"),
    "确认框",
  );
  requireElement(dialog.querySelector("form"), "确认表单").submit();
}

test("只有 admin_actions 含 release_hold 才出现解除止售入口", async () => {
  await withDetail(async (tree, mount) => {
    tree.requests.installFetch([success(adminDetail(heldListing([])))]);
    mount();
    await settle();
    expect(button(tree, "解除止售")).toBeNull();
  });
  await withDetail(async (tree, mount) => {
    tree.requests.installFetch([success(adminDetail(heldListing()))]);
    mount();
    await settle();
    expect(button(tree, "解除止售")).not.toBeNull();
  });
});

test("解除止售提交 hold_id 与读到的版本，成功后重读详情", async () => {
  await withDetail(async (tree, mount) => {
    tree.requests.installBrowserRequest();
    const posts: string[] = [];
    const calls: string[] = [];
    const restore = installFixtureFetch(async (input, init) => {
      const request = recordRequest(input, init);
      calls.push(path(request));
      switch (path(request)) {
        case DETAIL:
          return reply(JSON.parse(success(adminDetail(heldListing())).body));
        case SESSION:
          return Response.json(TEST_SESSION_BODY);
        case RELEASE_HOLD:
          posts.push(request.body);
          return reply(JSON.parse(success(holdReleasedReceipt(commandId(request.body))).body));
        default:
          throw new Error("未预期的内存请求");
      }
    });
    try {
      mount();
      await settle();
      confirmRelease(tree);
      await settle();
      expect(posts).toHaveLength(1);
      expect(parseJsonObject(posts[0] ?? "", "解除止售命令")).toEqual({
        command_id: expect.stringMatching(/^[0-9a-f-]{36}$/u),
        expected_version: 3,
        hold_id: HOLD_ID,
      });
      expect(calls).toEqual([DETAIL, SESSION, RELEASE_HOLD, DETAIL]);
      expect(tree.find((node) => node.textContent === "已解除运营止售")).not.toBeNull();
    } finally {
      restore();
    }
  });
});

const OTHER_HOLD_ID = "10000000-0000-4000-8000-0000000000ff";

// 详情版本是 3：回执的 hold_id 或版本任一不符（版本应为 4），都只能用原请求重放。
for (const [label, version, holdId] of [
  ["hold_id", 4, OTHER_HOLD_ID],
  ["listing_version", 5, HOLD_ID],
] as const) {
  test(`回执的 ${label} 不符判结果未知，只能用原请求重放`, async () => {
    await withDetail(async (tree, mount) => {
      tree.requests.installBrowserRequest();
      const posts: string[] = [];
      const restore = installFixtureFetch(async (input, init) => {
        const request = recordRequest(input, init);
        switch (path(request)) {
          case DETAIL:
            return reply(JSON.parse(success(adminDetail(heldListing())).body));
          case SESSION:
            return Response.json(TEST_SESSION_BODY);
          case RELEASE_HOLD:
            posts.push(request.body);
            return reply(
              JSON.parse(
                success(holdReleasedReceipt(commandId(request.body), version, holdId)).body,
              ),
            );
          default:
            throw new Error("未预期的内存请求");
        }
      });
      try {
        mount();
        await settle();
        confirmRelease(tree);
        await settle();
        expect(tree.find((node) => node.textContent === "已解除运营止售")).toBeNull();
        expect(button(tree, "解除止售")?.disabled).toBe(true);
        requireElement(button(tree, "原样重试"), "原样重试").click();
        await settle();
        expect(posts).toHaveLength(2);
        expect(posts[1]).toEqual(posts[0]);
        expect(button(tree, "原样重试")).not.toBeNull();
      } finally {
        restore();
      }
    });
  });
}
