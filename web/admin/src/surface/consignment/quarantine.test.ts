// Admin 隔离页：观察总数提示、重检、六种解除回执与止售命令。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { parseJson } from "@auction/web-shared/net/wire";
import { RouterContext } from "@auction/web-shared/router/context";
import { openRouter } from "@auction/web-shared/router/test-window";
import { buildBundle, DEFAULT_ADMIN_LOCALE } from "@auction/web-shared/ui/i18n";
import { LocaleSessionContext, withoutFluentIsolation } from "@auction/web-shared/ui/i18n/session";
import { transform } from "@solidjs/compiler";
import type { JSX } from "@solidjs/web";
import { createRoot, flush } from "solid-js";

import {
  type AdminRequestHarness,
  createAdminRequestHarness,
  installFixtureFetch,
  installWriteFetch,
  type RecordedRequest,
  recordRequest,
  TEST_SESSION_BODY,
} from "../../api/api-request-fixture";
import { prepareAdminHold, prepareAdminReleaseHold } from "../../api/consignment/quarantine";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { ADMIN_RENDER_SOURCES, SHARED_RENDER_SOURCES } from "../../solid-render-sources-fixture";
import { cleanupScope, preserveProperty } from "../../test-scope-fixture";
import { adminFtl, installAdminTranslation } from "../admin-locale-fixture.test";
import { PendingCommandHostContext, usePendingCommandHost } from "../pending";
import {
  createComponent,
  findAll,
  installTestDocument,
  mountNode,
  settle,
  type TestElement,
} from "../solid-dom-fixture";

const RENDER_NAMESPACE = "auction-admin-quarantine-render";
const SHARED_NAMESPACE = "auction-admin-render";
const ADMIN_ROOT = resolve(import.meta.dir, "../..");
const RUNTIME = resolve(ADMIN_ROOT, "surface/solid-dom-fixture.ts");
const PAGE = resolve(import.meta.dir, "quarantine.tsx");
const SHARED_SOURCES = new Set([
  ...ADMIN_RENDER_SOURCES.map((path) => resolve(ADMIN_ROOT, path)),
  ...SHARED_RENDER_SOURCES.map((path) => resolve(ADMIN_ROOT, "../../shared/src", path)),
]);
const stripTypes = new Bun.Transpiler({ loader: "ts" });
let rendererRegistered = false;

function registerRenderer(): void {
  if (rendererRegistered) return;
  Bun.plugin({
    name: RENDER_NAMESPACE,
    setup(build) {
      build.onResolve({ filter: /.*/u, namespace: RENDER_NAMESPACE }, ({ path }) => ({
        path,
        namespace: RENDER_NAMESPACE,
      }));
      build.onLoad({ filter: /.*/u, namespace: RENDER_NAMESPACE }, ({ path }) => {
        if (path !== PAGE) {
          throw new Error(`未登记 Admin 隔离页源码 ${path}`);
        }
        const source = readFileSync(path, "utf8");
        const { code } = transform(source, {
          filename: path,
          generate: "universal",
          moduleName: RUNTIME,
        });
        const contents = stripTypes
          .transformSync(code)
          .replace(
            /(\bfrom\s+|\bimport\()("(?:[^"\\\n]|\\.)*")/gu,
            (_, prefix: string, raw: string) => {
              const specifier = parseJson(raw);
              if (!specifier.ok || typeof specifier.value !== "string") {
                throw new Error(`无法解析隔离页导入 ${raw}`);
              }
              const dependency = Bun.resolveSync(specifier.value, dirname(path));
              return (
                prefix +
                JSON.stringify(
                  SHARED_SOURCES.has(dependency) ? `${SHARED_NAMESPACE}:${dependency}` : dependency,
                )
              );
            },
          );
        return { contents, loader: "js" };
      });
    },
  });
  rendererRegistered = true;
}

const ID = "00000000-0000-4000-8000-0000000000d1";
const OBSERVATION = "00000000-0000-4000-8000-0000000000d2";
const OBSERVATION_2 = "00000000-0000-4000-8000-0000000000d3";
const COMMAND = "00000000-0000-4000-8000-0000000000d4";
const LISTING = "00000000-0000-4000-8000-0000000000d5";
const HOLD = "00000000-0000-4000-8000-0000000000d6";
const TIME = "2026-11-21T00:00:00Z";
interface TreeFixture {
  readonly requests: AdminRequestHarness;
  mount(render: () => JSX.Element): void;
  find(predicate: (node: TestElement) => boolean): TestElement | null;
  elements(predicate: (node: TestElement) => boolean): TestElement[];
}

function quarantineTranslator() {
  const built = buildBundle("en", adminFtl("en"), "en", null);
  if (!built.ok) throw new Error("隔离页译文装配失败");
  return (id: string, args?: Record<string, unknown>) => {
    const pattern = built.value.getMessage(id)?.value;
    if (pattern == null) throw new Error(`缺少隔离页译文 ${id}`);
    const variables = Object.fromEntries(
      Object.entries(args ?? {}).map(([key, value]) => [key, String(value)]),
    );
    const errors: Error[] = [];
    const text = built.value.formatPattern(pattern, variables, errors);
    if (errors[0] !== undefined) throw errors[0];
    return withoutFluentIsolation(text);
  };
}

/** 一条观察：`ordinals` 必须是从 1 开始的连续序号，数量与 `attachment_count` 一致。 */
function observation(id: string, ordinals: readonly number[]) {
  return {
    observation_id: id,
    observed_at: TIME,
    parser_version: "p4d2-v1",
    digest: "a".repeat(64),
    attachment_count: ordinals.length,
    credits: null,
    cod: null,
    returnable: true,
    takeable: true,
    items: ordinals.map((ordinal) => ({
      ordinal,
      kind: "equipment",
      external_equipment_id: String(41 + ordinal),
      amount: null,
    })),
  };
}

function detail(total = 3) {
  return {
    quarantine_id: ID,
    version: 3,
    state: "active",
    world: "main",
    external_mid: "12345",
    first_decision_id: "00000000-0000-4000-8000-0000000000d7",
    reason: "duplicate_raw_eid",
    observation_total: total,
    observations: [observation(OBSERVATION, [1]), observation(OBSERVATION_2, [1, 2])],
    allowed_actions: ["recheck", "release"],
    release: null,
  };
}

function success(data: unknown, status = 200, code = "ok") {
  return {
    status,
    body: JSON.stringify({ code, message: "", data }),
  };
}

const SESSION_RESPONSE = { body: JSON.stringify(TEST_SESSION_BODY) };

type PageReply = { readonly status?: number; readonly body: string };
type PageRoute = (request: RecordedRequest) => PageReply;

function pathOf(request: RecordedRequest): string {
  return new URL(request.url, "http://localhost").pathname;
}

function commandIdOf(request: RecordedRequest): string {
  const parsed = parseJson(request.body);
  if (!parsed.ok || parsed.value === null || typeof parsed.value !== "object") {
    throw new Error("测试写入缺少 JSON 正文");
  }
  const commandId = (parsed.value as Record<string, unknown>).command_id;
  if (typeof commandId !== "string") throw new Error("测试写入缺少 command_id");
  return commandId;
}

function releaseReceipt(kind: string, commandId: string) {
  const common = {
    schema_version: 2,
    operation: "release_quarantine",
    disposition: "quarantine_released",
    command_id: commandId,
    quarantine_id: ID,
    release_id: "00000000-0000-4000-8000-0000000000d8",
    decision_id: "00000000-0000-4000-8000-0000000000d9",
    decision_kind: kind,
  };
  switch (kind) {
    case "accept":
      return {
        ...common,
        listing_changes: [{ listing_id: LISTING, listing_version: 4, state: "deposit_accepted" }],
      };
    case "acquire_return":
      return {
        ...common,
        return_intake_id: "00000000-0000-4000-8000-0000000000da",
        original_round: 1,
        take_work_id: "00000000-0000-4000-8000-0000000000db",
        take_delivery_id: "00000000-0000-4000-8000-0000000000dc",
      };
    case "auction_return_case":
      return {
        ...common,
        return_mail_id: "00000000-0000-4000-8000-0000000000dd",
        return_evidence_id: "00000000-0000-4000-8000-0000000000de",
        obligation_id: null,
      };
    case "auction_recovery_return":
      return {
        ...common,
        obligation_id: "00000000-0000-4000-8000-0000000000df",
        return_mail_id: "00000000-0000-4000-8000-0000000000dd",
        recovery_id: "00000000-0000-4000-8000-0000000000e0",
        recovery_handoff_id: "00000000-0000-4000-8000-0000000000e1",
      };
    default:
      return common;
  }
}

async function loadPage(): Promise<typeof import("./quarantine")> {
  registerRenderer();
  return import(`${RENDER_NAMESPACE}:${PAGE}`) as Promise<typeof import("./quarantine")>;
}

async function withQuarantineTree(
  href: string,
  run: (tree: TreeFixture) => Promise<void>,
): Promise<void> {
  const { AdminSessionHostContext } = await loadRenderModule<
    Pick<typeof import("../auth/shell"), "AdminSessionHostContext">
  >(new URL("../auth/shell.tsx", import.meta.url).pathname, "AdminSessionHostContext");
  const requests = createAdminRequestHarness();
  const scope = cleanupScope();
  scope.defer(requests.cleanup);
  try {
    requests.installBrowserRequest();
    scope.defer(installAdminTranslation(quarantineTranslator()));
    const document = installTestDocument();
    scope.defer(document.restore);
    scope.defer(preserveProperty(globalThis, "window"));
    const { router, disposeRouter } = openRouter(href);
    scope.defer(disposeRouter);
    const { host } = requests.registerHost();
    await run({
      requests,
      mount(render) {
        createRoot((dispose) => {
          scope.defer(dispose);
          const pendingHost = usePendingCommandHost(host);
          const tree = createComponent(RouterContext, {
            value: router,
            get children() {
              return createComponent(AdminSessionHostContext, {
                value: host,
                get children() {
                  return createComponent(PendingCommandHostContext, {
                    value: pendingHost,
                    get children() {
                      return createComponent(LocaleSessionContext, {
                        value: {
                          current: () => DEFAULT_ADMIN_LOCALE,
                          selected: () => DEFAULT_ADMIN_LOCALE,
                          isReady: () => true,
                          error: () => null,
                          bootstrap() {},
                          switch() {},
                        },
                        get children() {
                          return createComponent(render, {});
                        },
                      });
                    },
                  });
                },
              });
            },
          });
          mountNode(document, tree);
        });
        flush();
      },
      find: (predicate) => document.find(predicate),
      elements: (predicate) => findAll(document.roots, predicate),
    });
  } finally {
    try {
      await settle();
    } finally {
      scope.dispose();
    }
  }
}

async function withPage(
  route: PageRoute,
  run: (tree: TreeFixture, mount: () => void) => Promise<void>,
) {
  await withQuarantineTree(`/consignment/deposits/${ID}`, async (tree) => {
    const { quarantinePage } = await loadPage();
    const restore = installFixtureFetch(async (input, init) => {
      const reply = route(recordRequest(input, init));
      return new Response(reply.body, { status: reply.status ?? 200 });
    });
    try {
      await run(tree, () =>
        tree.mount(() => quarantinePage({ params: { quarantine_id: ID }, query: {} })),
      );
    } finally {
      restore();
    }
  });
}

function findButton(tree: TreeFixture, text: string) {
  return tree.find((node) => node.tag === "button" && node.textContent === text);
}

test("观察总数大于返回条数时提示只显示最新一页", async () => {
  await withPage(
    () => success(detail()),
    async (tree, mount) => {
      mount();
      await settle();
      expect(
        tree.find((node) => node.textContent.includes("Showing the latest 2 of 3")),
      ).not.toBeNull();
      expect(tree.find((node) => node.textContent === "Duplicate raw EID")).not.toBeNull();
      expect(tree.find((node) => node.textContent === "42")).not.toBeNull();
    },
  );
});

test("重检回执显示 work ID，并推进详情读取世代", async () => {
  await withPage(
    (request) => {
      if (
        pathOf(request).startsWith(`/api/v2/admin/consignment/deposits/${ID}`) &&
        request.method === "GET"
      ) {
        return success(detail());
      }
      if (pathOf(request) === "/api/v1/admin/session") return SESSION_RESPONSE;
      if (request.method === "POST" && pathOf(request).endsWith("/recheck")) {
        const commandId = commandIdOf(request);
        return success(
          {
            schema_version: 2,
            operation: "recheck_quarantine",
            disposition: "recheck_queued",
            command_id: commandId,
            quarantine_id: ID,
            work_id: "00000000-0000-4000-8000-0000000000e2",
          },
          202,
          "accepted",
        );
      }
      throw new Error(`未预期的隔离页请求 ${request.method} ${request.url}`);
    },
    async (tree, mount) => {
      mount();
      await settle();
      const recheck = findButton(tree, "Recheck quarantine");
      expect(recheck).not.toBeNull();
      recheck?.click();
      await settle();
      expect(
        tree.find((node) =>
          node.textContent.includes(
            "Recheck queued. Work ID: 00000000-0000-4000-8000-0000000000e2",
          ),
        ),
      ).not.toBeNull();
    },
  );
});

const BRANCHES = [
  ["accept", "Accepted listings", "deposit_accepted"],
  ["return", "Return the envelope", null],
  ["archive_empty", "Archive as empty", null],
  ["acquire_return", "Return intake ID", "1"],
  ["auction_return_case", "Return mail ID", "00000000-0000-4000-8000-0000000000dd"],
  ["auction_recovery_return", "Recovery ID", "00000000-0000-4000-8000-0000000000e0"],
] as const;

for (const [kind, label, expected] of BRANCHES) {
  test(`${kind} 回执按分支字段展示`, async () => {
    await withPage(
      (request) => {
        if (
          pathOf(request).startsWith(`/api/v2/admin/consignment/deposits/${ID}`) &&
          request.method === "GET"
        ) {
          return success(detail());
        }
        if (pathOf(request) === "/api/v1/admin/session") return SESSION_RESPONSE;
        if (request.method === "POST" && pathOf(request).endsWith("/release-quarantine")) {
          return success(releaseReceipt(kind, commandIdOf(request)));
        }
        throw new Error(`未预期的隔离页请求 ${request.method} ${request.url}`);
      },
      async (tree, mount) => {
        mount();
        await settle();
        const selects = tree.elements((node) => node.tag === "select");
        expect(selects).toHaveLength(2);
        selects[1]?.change(kind);
        const form = tree.find((node) => node.tag === "form");
        expect(form).not.toBeNull();
        form?.submit();
        await settle();
        expect(
          tree.find((node) => node.textContent.includes("Quarantine released.")),
        ).not.toBeNull();
        expect(tree.find((node) => node.textContent.includes(label))).not.toBeNull();
        if (expected !== null) {
          expect(tree.find((node) => node.textContent.includes(expected))).not.toBeNull();
        }
      },
    );
  });
}

test("止售与解除止售复用 Admin 写宿主并核对原命令", async () => {
  const harness = createAdminRequestHarness();
  try {
    harness.installBrowserRequest();
    const { host } = harness.registerHost();
    const { writes, restore } = installWriteFetch((request) => {
      const commandId = commandIdOf(request);
      if (request.path.endsWith("/delist")) {
        return Response.json({
          code: "ok",
          message: "",
          data: {
            schema_version: 2,
            operation: "admin_delist",
            disposition: "hold_set",
            command_id: commandId,
            listing_id: LISTING,
            listing_version: 4,
            state: "held_off_market",
            hold_id: HOLD,
          },
        });
      }
      if (request.path.endsWith("/release-hold")) {
        return Response.json({
          code: "ok",
          message: "",
          data: {
            schema_version: 2,
            operation: "release_hold",
            disposition: "hold_released",
            command_id: commandId,
            listing_id: LISTING,
            listing_version: 4,
            state: "held_off_market",
            hold_id: HOLD,
          },
        });
      }
      throw new Error(`未预期的 Admin 写入 ${request.path}`);
    });
    const holdCommand = await prepareAdminHold(host, LISTING, 3, "运营夹具");
    const hold = await holdCommand(COMMAND, new AbortController().signal);
    expect(hold.ok && hold.value.kind).toBe("hold");
    const releaseCommand = await prepareAdminReleaseHold(host, LISTING, 3, HOLD);
    const release = await releaseCommand(COMMAND, new AbortController().signal);
    expect(release.ok && release.value.disposition).toBe("hold_released");
    expect(writes.map((request) => request.path)).toEqual([
      `/api/v2/admin/consignment/listings/${LISTING}/delist`,
      `/api/v2/admin/consignment/listings/${LISTING}/release-hold`,
    ]);
    restore();
  } finally {
    harness.cleanup();
  }
});

test("下架命令把 v2 closed 回执识别为关闭", async () => {
  const harness = createAdminRequestHarness();
  try {
    harness.installBrowserRequest();
    const { host } = harness.registerHost();
    const { restore } = installWriteFetch((request) => {
      if (!request.path.endsWith("/delist")) {
        throw new Error(`未预期的 Admin 写入 ${request.path}`);
      }
      return Response.json({
        code: "ok",
        message: "",
        data: {
          schema_version: 2,
          operation: "admin_delist",
          disposition: "closed",
          command_id: commandIdOf(request),
          listing_id: LISTING,
          listing_version: 4,
          state: "closed",
        },
      });
    });
    try {
      const command = await prepareAdminHold(host, LISTING, 3, "运营夹具");
      const result = await command(COMMAND, new AbortController().signal);
      expect(result.ok && result.value.kind).toBe("closed");
      expect(result.ok && result.value.receipt.disposition).toBe("closed");
    } finally {
      restore();
    }
  } finally {
    harness.cleanup();
  }
});

// 不变量 1、6：回执的命令、挂单或版本与本次请求不符时不能当作成功了结，只能判为结果未知、原样重试。
for (const [label, mismatch] of [
  ["command_id", { command_id: "00000000-0000-4000-8000-0000000000aa" }],
  ["listing_id", { listing_id: "00000000-0000-4000-8000-0000000000bb" }],
  ["listing_version", { listing_version: 3 }],
] as const) {
  test(`下架回执的 ${label} 不符时判为结果未知`, async () => {
    const harness = createAdminRequestHarness();
    try {
      harness.installBrowserRequest();
      const { host } = harness.registerHost();
      const { restore } = installWriteFetch((request) =>
        Response.json({
          code: "ok",
          message: "",
          data: {
            schema_version: 2,
            operation: "admin_delist",
            disposition: "closed",
            command_id: commandIdOf(request),
            listing_id: LISTING,
            listing_version: 4,
            state: "closed",
            ...mismatch,
          },
        }),
      );
      try {
        const command = await prepareAdminHold(host, LISTING, 3, "运营夹具");
        const result = await command(COMMAND, new AbortController().signal);
        expect(result.ok).toBe(false);
        expect(!result.ok && result.error.kind).toBe("outcome-unknown");
      } finally {
        restore();
      }
    } finally {
      harness.cleanup();
    }
  });
}
