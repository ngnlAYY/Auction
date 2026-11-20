// 通过真实导入表单、会话宿主和传输，核对跨页冻结与失败裁定。

import { expect, spyOn, test } from "bun:test";
import { inspect } from "node:util";
import { sleep } from "@auction/web-shared/platform/timer";
import { createCell } from "@auction/web-shared/reactive/cell";
import type { LocaleCode } from "@auction/web-shared/ui/i18n";
import { createRoot, onCleanup } from "solid-js";
import {
  type CapturedWrite,
  createAdminRequestHarness,
  installWriteFetch,
  parseJsonObject,
} from "../../api/api-request-fixture";
import type { AdminCredential } from "../../api/credentials";
import { registerAdminSessionHost } from "../../api/session";
import { createAdminTranslator, installAdminTranslation } from "../admin-locale-fixture.test";
import * as pending from "../pending";
import { settle, type TestDocument, visit } from "../solid-dom-fixture";
import {
  button,
  clickButton,
  field,
  findButton,
  loadCredentialForm,
  onlyElement,
  submitForm,
  withT10View,
} from "../t10-view-fixture.test";
import { createCredentialSecretDraft } from "./secret-draft";

const COOKIE = "fake-test-cookie=placeholder";
const USERNAME = "fake-test-forum-account";
const PASSWORD = "fake-test-forum-password";
const LABEL = "测试凭据";
const PATH = "/api/v1/admin/credentials";
const RESTART = "操作已完成。请重启 Worker 以应用凭据变更。";
const REJECTIONS = [
  [501, "feature_not_supported"],
  [503, "credential_login_unavailable"],
] as const;

function rejection(status: number, code: string): Response {
  return Response.json(
    { code, message: "测试请求未完成", data: null, request_id: null },
    { status },
  );
}

function success(label = LABEL, projection: Partial<AdminCredential> = {}): Response {
  return Response.json(
    {
      code: "ok",
      message: "",
      data: {
        credential: {
          id: "11111111-1111-4111-8111-111111111111",
          kind: "mail_cookie",
          world: "main",
          label,
          player_name: null,
          status: "disabled",
          state_version: 1,
          created_at: "2026-11-18T00:00:00Z",
          rotated_at: null,
          updated_at: "2026-11-18T00:00:00Z",
          ...projection,
        },
        restart_required: true,
      },
    },
    { status: 201 },
  );
}

async function submitImport(document: TestDocument, source: "cookie" | "forum_login") {
  field(document, "import-label").input(LABEL);
  field(document, "import-source").change(source);
  await settle();
  if (source === "forum_login") {
    field(document, "import-username").input(USERNAME);
    field(document, "current-password").input(PASSWORD);
  } else {
    field(document, "import-secret").input(COOKIE);
  }
  submitForm(onlyElement(document, "form"));
  await settle();
}

function requireWrite(writes: CapturedWrite[], index: number): CapturedWrite {
  const request = writes[index];
  if (request === undefined) throw new Error("缺少预期的凭据写入请求");
  expect(request.method).toBe("POST");
  expect(request.path).toBe(PATH);
  return request;
}

async function withImport(
  replies: readonly ((signal?: AbortSignal) => Response | Promise<Response>)[],
  run: (document: TestDocument, writes: CapturedWrite[], pause: () => void) => Promise<void>,
  locale?: LocaleCode,
) {
  const Fixture = await loadCredentialForm();
  const browser = createAdminRequestHarness();
  const allowed = createCell(true);
  const session = registerAdminSessionHost({
    check() {},
    writesAllowed: allowed,
    confirmed: () => ({
      actor: "fixture",
      csrfToken: "csrf-fixture",
      csrf_token: "csrf-fixture",
      expiresAt: 1n,
      absoluteExpiresAt: 2n,
      serverTime: 0n,
      receivedAtMillis: 0,
    }),
  });
  const fetch = installWriteFetch((_request, init, index) => {
    const reply = replies[index];
    if (reply === undefined) throw new Error("出现计划之外的凭据请求");
    return reply(init?.signal ?? undefined);
  });
  try {
    browser.installBrowserRequest();
    await withT10View(
      () => {
        if (locale !== undefined) {
          onCleanup(installAdminTranslation(createAdminTranslator(() => locale)));
        }
        return Fixture({ host: session.host, navigate: true });
      },
      (document) => run(document, fetch.writes, () => allowed.set(false)),
    );
  } finally {
    fetch.restore();
    browser.cleanup();
    session.release();
  }
}

for (const source of ["cookie", "forum_login"] as const) {
  test(`import_retries_the_same_payload_after_leaving_${source}`, async () => {
    const lost = Promise.withResolvers<Response>();
    const [status, code] = REJECTIONS[source === "cookie" ? 0 : 1];
    await withImport(
      [
        () => lost.promise,
        () => rejection(status, code),
        () => rejection(status === 501 ? 503 : 501, code),
        () => new Response('{"code":"feature_not_supported"}', { status: 501 }),
        () => success("错配标签"),
        () => success(),
      ],
      async (document, writes) => {
        await submitImport(document, source);
        expect(writes).toHaveLength(1);
        expect(document.getElementById("import-unknown-help")).toBeNull();
        expect(findButton(document, "放弃")).toBeNull();
        submitForm(onlyElement(document, "form"));
        await clickButton(document, "切换凭据页");
        await settle();
        expect(writes).toHaveLength(1);
        expect(document.getElementById("import-label")).toBeNull();

        lost.reject(new Error("测试回执丢失"));
        await settle();
        const first = requireWrite(writes, 0);
        const body = parseJsonObject(first.body, "凭据导入正文");
        expect(
          typeof body.command_id === "string" &&
            /^[\da-f]{8}-(?:[\da-f]{4}-){1}4[\da-f]{3}-[89ab][\da-f]{3}-[\da-f]{12}$/u.test(
              body.command_id,
            ),
        ).toBe(true);
        expect(body.kind).toBe("mail_cookie");
        expect(body.world).toBe("main");
        expect(body.label).toBe(LABEL);
        expect(body.player_name ?? null).toBeNull();
        expect(body.secret === (source === "cookie" ? COOKIE : undefined)).toBe(true);
        const login = body.login;
        expect(
          source === "cookie"
            ? login === undefined
            : typeof login === "object" &&
                login !== null &&
                "username" in login &&
                login.username === USERNAME &&
                "password" in login &&
                login.password === PASSWORD,
        ).toBe(true);

        const notices = onlyElement(document, ".admin-command-notices");
        expect(notices.textContent.includes("导入邮件 Cookie（测试凭据）")).toBe(true);
        for (const secret of [COOKIE, USERNAME, PASSWORD]) {
          expect(
            document.roots
              .map((node) => node.textContent)
              .join("")
              .includes(secret),
          ).toBe(false);
        }

        await clickButton(document, "切换凭据页");
        await settle();
        expect(document.getElementById("import-unknown-help")).not.toBeNull();
        expect(field(document, "credential-fixture-reload").textContent).toBe("0");
        for (const attempt of [1, 2, 3, 4, 5]) {
          await clickButton(document, "原样重试");
          await settle();
          // 只报告相等与否，避免测试失败时把冻结正文写进输出。
          expect(requireWrite(writes, attempt).body === first.body).toBe(true);
          if (attempt < 5) {
            expect(findButton(document, "原样重试")).not.toBeNull();
            expect(field(document, "credential-fixture-reload").textContent).toBe("0");
          }
        }
        expect(writes).toHaveLength(6);
        expect(findButton(document, "原样重试")).toBeNull();
        expect(document.getElementById("import-unknown-help")).toBeNull();
        expect(field(document, "credential-fixture-reload").textContent).toBe("1");
        expect(onlyElement(document, ".admin-command-notices__receipt").textContent).toContain(
          RESTART,
        );
      },
    );
  });
}

test("import_rejection_releases_the_first_attempt_but_plain_503_can_be_abandoned_while_paused", async () => {
  await withImport(
    [
      ...REJECTIONS.map(
        ([status, code]) =>
          () =>
            rejection(status, code),
      ),
      () => rejection(503, "storage_unavailable"),
    ],
    async (document, writes, pause) => {
      for (const [index, source] of (["cookie", "forum_login"] as const).entries()) {
        await submitImport(document, source);
        expect(findButton(document, "原样重试")).toBeNull();
        const draft = field(document, "import-label");
        expect(draft.value).toBe(LABEL);
        expect(draft.getAttribute("disabled")).toBeNull();
        requireWrite(writes, index);
      }
      await submitImport(document, "cookie");
      expect(document.getElementById("import-unknown-help")).not.toBeNull();
      expect(writes).toHaveLength(3);
      await clickButton(document, "放弃");
      await settle();
      await clickButton(document, "保留未决操作");
      await settle();
      expect(findButton(document, "原样重试")).not.toBeNull();
      pause();
      await settle();
      expect(button(document, "原样重试").attributes.disabled).toBe(true);
      button(document, "原样重试").dispatchClick();
      await settle();
      expect(writes).toHaveLength(3);
      await clickButton(document, "放弃");
      await settle();
      await clickButton(document, "确认放弃");
      await sleep(0);
      await settle();
      expect(findButton(document, "原样重试")).toBeNull();
      expect(document.getElementById("import-unknown-help")).toBeNull();
      expect(field(document, "import-label").value).toBe(LABEL);
      expect(document.activeElement?.id).toBe("admin-content");
      submitForm(onlyElement(document, "form"));
      await settle();
      expect(writes).toHaveLength(3);
    },
  );
});

test("import_success_clears_the_mounted_draft", async () => {
  const draftModule = await import("./secret-draft");
  const createDraft = createCredentialSecretDraft;
  const drafts: ReturnType<typeof createDraft>[] = [];
  const observer = spyOn(draftModule, "createCredentialSecretDraft").mockImplementation(() => {
    const draft = createDraft();
    drafts.push(draft);
    return draft;
  });
  try {
    for (const [index, source] of (["cookie", "forum_login"] as const).entries()) {
      const reply = Promise.withResolvers<Response>();
      await withImport([() => reply.promise], async (document, writes) => {
        await submitImport(document, source);
        const draft = drafts[index];
        if (draft === undefined) throw new Error("未捕获挂载表单的凭据草稿");
        expect(writes).toHaveLength(1);
        if (source === "forum_login") {
          expect(draft.username.peek() === USERNAME).toBe(true);
          expect(draft.password.peek() === PASSWORD).toBe(true);
        } else {
          expect(draft.secret.peek() === COOKIE).toBe(true);
        }

        reply.resolve(success());
        await settle();
        expect(field(document, "import-label").value).toBe("");
        expect(field(document, "import-secret").value === "").toBe(true);
        expect(document.getElementById("current-password")).toBeNull();
        expect(field(document, "credential-fixture-reload").textContent).toBe("1");
        expect(findButton(document, "原样重试")).toBeNull();
        expect(writes).toHaveLength(1);
        if (source === "forum_login") {
          // 来源切换也会清理草稿，须先观察成功回执处理完毕时的隐藏字段。
          expect(draft.isAlive()).toBe(true);
          expect(draft.username.peek() === "").toBe(true);
          expect(draft.password.peek() === "").toBe(true);
          field(document, "import-source").change("forum_login");
          await settle();
          expect(field(document, "import-username").value === "").toBe(true);
          expect(field(document, "current-password").value === "").toBe(true);
        }
      });
    }
  } finally {
    observer.mockRestore();
  }
});

test("import_first_rejection_after_leaving_is_delivered_to_the_session", async () => {
  const delayed = Promise.withResolvers<Response>();
  await withImport([() => delayed.promise, () => success()], async (document, writes) => {
    await submitImport(document, "cookie");
    await clickButton(document, "切换凭据页");
    delayed.resolve(rejection(501, "feature_not_supported"));
    await settle();
    const notice = onlyElement(document, ".admin-command-notices__receipt");
    expect(notice.textContent.includes("当前部署未提供此操作")).toBe(true);
    expect(findButton(document, "原样重试")).toBeNull();
    expect(writes).toHaveLength(1);
    await clickButton(document, "切换凭据页");
    await submitImport(document, "cookie");
    expect(writes).toHaveLength(2);
    const original = requireWrite(writes, 0);
    const next = requireWrite(writes, 1);
    expect(
      parseJsonObject(next.body, "新的导入正文").command_id ===
        parseJsonObject(original.body, "已拒绝的导入正文").command_id,
    ).toBe(false);
  });
});

test("import_ignores_a_late_receipt_after_the_session_host_is_replaced", async () => {
  const delayed = Promise.withResolvers<Response>();
  let signal: AbortSignal | undefined;
  await withImport(
    [
      (current) => {
        signal = current;
        return delayed.promise;
      },
      () => success(),
    ],
    async (document, writes) => {
      await submitImport(document, "cookie");
      const original = requireWrite(writes, 0);
      expect(signal?.aborted).toBe(false);
      await clickButton(document, "切换认证宿主");
      expect(signal?.aborted).toBe(true);
      await clickButton(document, "切换认证宿主");
      field(document, "import-label").input("新会话草稿");

      delayed.resolve(success());
      await settle();
      expect(field(document, "import-label").value).toBe("新会话草稿");
      expect(field(document, "credential-fixture-reload").textContent).toBe("0");
      expect(document.querySelector(".admin-command-notices__receipt")).toBeNull();
      expect(document.getElementById("import-unknown-help")).toBeNull();

      await submitImport(document, "cookie");
      expect(writes).toHaveLength(2);
      const next = requireWrite(writes, 1);
      expect(
        parseJsonObject(next.body, "新会话正文").command_id ===
          parseJsonObject(original.body, "旧会话正文").command_id,
      ).toBe(false);
    },
  );
});

test("import_unknown_notice_localizes_only_safe_identity_and_retry_guidance", async () => {
  const locales = [
    ["en", "Import Mail cookie (测试凭据)", "Retry it with the same data or abandon it there."],
    ["zh-CN", "导入邮件 Cookie（测试凭据）", "请在未决操作区原样重试或放弃。"],
    ["zh-TW", "匯入郵件 Cookie（测试凭据）", "請在未決操作區以相同內容重試或放棄。"],
    ["ja", "メール Cookie のインポート（测试凭据）", "同じ内容を再試行するか、放棄してください。"],
    ["ko", "메일 쿠키 가져오기 (测试凭据)", "같은 내용으로 다시 시도하거나 포기하세요."],
  ] as const;
  for (const [locale, title, guidance] of locales) {
    await withImport(
      [() => rejection(503, "storage_unavailable")],
      async (document) => {
        expect(document.getElementById("import-unknown-help")).toBeNull();
        await submitImport(document, "forum_login");
        expect(field(document, "import-unknown-help").textContent).toContain(guidance);
        const panel = onlyElement(document, ".admin-command-notices");
        expect(panel.textContent.includes(title)).toBe(true);
        for (const secret of [COOKIE, USERNAME, PASSWORD]) {
          expect(panel.textContent.includes(secret)).toBe(false);
        }
      },
      locale,
    );
  }
});

function assertSafeOutput(output: string): void {
  for (const secret of [COOKIE, USERNAME, PASSWORD]) {
    expect(output.includes(secret)).toBe(false);
  }
}

function renderedText(document: TestDocument): string {
  return document.roots.map((node) => node.textContent).join("");
}

function assertSafeView(document: TestDocument): void {
  assertSafeOutput(renderedText(document));
  visit(document.roots, (node) => {
    for (const [key, value] of Object.entries(node.attributes)) {
      // 密码和 Cookie 输入控件自身的可编辑值是预期输入。
      if (typeof value === "string" && !(node.tag === "input" && key === "value")) {
        assertSafeOutput(value);
      }
    }
  });
}

test("import_keeps_the_original_command_after_each_ownership_mismatch", async () => {
  await withImport(
    [
      () => success(LABEL, { kind: "forum_cookie" }),
      () => success(LABEL, { world: "isekai" }),
      () => success(LABEL, { player_name: "Other Player" }),
      () => success(),
    ],
    async (document, writes) => {
      await submitImport(document, "cookie");
      const first = requireWrite(writes, 0);
      const id = parseJsonObject(first.body, "导入命令").command_id;
      if (typeof id !== "string") throw new Error("缺少导入命令 ID");
      for (const attempt of [0, 1, 2]) {
        expect(writes).toHaveLength(attempt + 1);
        expect(requireWrite(writes, attempt).body === first.body).toBe(true);
        expect(onlyElement(document, "[data-command-id]").getAttribute("data-command-id")).toBe(id);
        expect(document.getElementById("import-unknown-help")).not.toBeNull();
        expect(field(document, "credential-fixture-reload").textContent).toBe("0");
        expect(renderedText(document).includes(RESTART)).toBe(false);
        await clickButton(document, "原样重试");
      }
      expect(writes).toHaveLength(4);
      expect(requireWrite(writes, 3).body === first.body).toBe(true);
      expect(document.querySelector("[data-command-id]") === null).toBe(true);
      expect(field(document, "credential-fixture-reload").textContent).toBe("1");
      expect(renderedText(document).includes(RESTART)).toBe(true);
    },
  );
});

test("import_first_matching_forum_success_after_leaving_is_delivered_to_the_session", async () => {
  const delayed = Promise.withResolvers<Response>();
  await withImport([() => delayed.promise], async (document, writes) => {
    field(document, "import-kind").change("forum_cookie");
    field(document, "import-world").change("shared");
    await settle();
    await submitImport(document, "forum_login");
    const sent = parseJsonObject(requireWrite(writes, 0).body, "论坛导入命令");
    expect(sent.kind).toBe("forum_cookie");
    expect(sent.world).toBe("shared");
    expect(sent.player_name ?? null).toBeNull();

    await clickButton(document, "切换凭据页");
    expect(document.getElementById("import-label") === null).toBe(true);
    expect(renderedText(document).includes(RESTART)).toBe(false);
    delayed.resolve(success(LABEL, { kind: "forum_cookie", world: "shared" }));
    await settle();
    expect(document.getElementById("import-label") === null).toBe(true);
    expect(
      onlyElement(document, ".admin-command-notices__receipt").textContent.includes(RESTART),
    ).toBe(true);
    expect(document.querySelector("[data-command-id]") === null).toBe(true);
    expect(findButton(document, "原样重试") === null).toBe(true);
    assertSafeView(document);

    await clickButton(document, "切换凭据页");
    expect(field(document, "import-secret").value === "").toBe(true);
    expect(writes).toHaveLength(1);
  });
});

test("credential_secret_draft_releases_each_field_when_its_owner_is_disposed", () => {
  const draft = createRoot((dispose) => ({ fields: createCredentialSecretDraft(), dispose }));
  try {
    const { secret, username, password } = draft.fields;
    secret.set(COOKIE);
    username.set(USERNAME);
    password.set(PASSWORD);
    expect(
      secret.peek() === COOKIE && username.peek() === USERNAME && password.peek() === PASSWORD,
    ).toBe(true);
  } finally {
    draft.dispose();
  }
  const { secret, username, password } = draft.fields;
  expect(secret.peek() === "").toBe(true);
  expect(username.peek() === "").toBe(true);
  expect(password.peek() === "").toBe(true);
});

test("import_draft_and_retry_disappear_after_abandonment_or_session_end", async () => {
  const diagnostics: unknown[] = [];
  const observers = (["debug", "error", "info", "log", "warn"] as const).map((level) =>
    spyOn(console, level).mockImplementation((...args) => diagnostics.push(args)),
  );
  const useHost = pending.usePendingCommandHost;
  let mountedHost: pending.PendingCommandHost | null = null;
  const hostObserver = spyOn(pending, "usePendingCommandHost").mockImplementation((session) => {
    const host = useHost(session);
    mountedHost = host;
    return host;
  });
  function currentHost(): pending.PendingCommandHost {
    if (mountedHost === null) throw new Error("未捕获会话的命令宿主");
    return mountedHost;
  }
  try {
    for (const source of ["cookie", "forum_login"] as const) {
      for (const endSession of [false, true]) {
        const delayed = Promise.withResolvers<Response>();
        await withImport([() => delayed.promise], async (document, writes) => {
          const oldHost = currentHost();
          await submitImport(document, source);
          assertSafeView(document);
          await clickButton(document, "切换凭据页");
          expect(document.getElementById("import-label") === null).toBe(true);
          delayed.resolve(rejection(503, "storage_unavailable"));
          await settle();
          const retry = button(document, "原样重试");
          assertSafeView(document);
          await clickButton(document, "切换凭据页");
          expect(field(document, "import-secret").value === "").toBe(true);
          expect(document.getElementById("import-unknown-help") !== null).toBe(true);
          await clickButton(document, "切换凭据页");
          if (endSession) {
            expect(oldHost.entries().length).toBe(1);
            await clickButton(document, "切换认证宿主");
            expect(oldHost.entries().length).toBe(0);
            expect(findButton(document, "原样重试") === null).toBe(true);
            await clickButton(document, "切换认证宿主");
          } else {
            await clickButton(document, "放弃");
            expect(findButton(document, "原样重试")).not.toBeNull();
            await clickButton(document, "确认放弃");
            await clickButton(document, "切换凭据页");
          }
          expect(document.querySelector("[data-command-id]") === null).toBe(true);
          expect(findButton(document, "原样重试") === null).toBe(true);
          retry.dispatchClick();
          await settle();
          expect(writes).toHaveLength(1);
          expect(field(document, "import-secret").value === "").toBe(true);
          field(document, "import-source").change("forum_login");
          await settle();
          expect(field(document, "import-username").value === "").toBe(true);
          expect(field(document, "current-password").value === "").toBe(true);
          assertSafeView(document);
        });
      }
    }
  } finally {
    hostObserver.mockRestore();
    for (const observer of observers) observer.mockRestore();
  }
  assertSafeOutput(
    inspect(diagnostics, { depth: null, maxArrayLength: null, maxStringLength: null }),
  );
});
