// 凭据行写入的回执裁定、导入隔离与列表世代。

import { expect, spyOn, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { ok, type Result } from "@auction/web-shared/result";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import {
  createAdminRequestHarness,
  installWriteFetch,
  parseJsonObject,
  TEST_SESSION,
  type WriteReply,
} from "../../api/api-request-fixture";
import type { AdminCredential, AdminCredentialPage } from "../../api/credentials";
import { registerAdminSessionHost } from "../../api/session";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { settle, type TestDocument } from "../solid-dom-fixture";
import {
  button,
  clickButton,
  field,
  findButton,
  hasText,
  loadCredentialTable,
  onlyElement,
  submitForm,
  withT10View,
} from "../t10-view-fixture.test";

const ENDPOINT = "/api/v1/admin/credentials";
const PAGE_URL = "/cookies?page=2&page_size=10";
const CREDENTIAL: AdminCredential = {
  id: "0f8fad5b-d9cb-469f-a165-70867728950e",
  kind: "mail_cookie",
  world: "main",
  label: "测试凭据",
  player_name: null,
  status: "active",
  state_version: 1,
  created_at: "2026-11-18T00:00:00Z",
  rotated_at: null,
  updated_at: "2026-11-18T00:00:00Z",
};
const DISABLED = { ...CREDENTIAL, status: "disabled", state_version: 2 };
const RESTART = "操作已完成。请重启 Worker 以应用凭据变更。";

function projection(credential: AdminCredential = CREDENTIAL): AdminCredentialPage {
  return { credentials: [credential], page: 2, page_size: 10, total: 11, restart_required: true };
}

function receipt(data: object): Response {
  return Response.json({ code: "ok", message: "", data, request_id: null });
}

function submitImport(document: TestDocument): void {
  field(document, "import-label").input("新的凭据");
  field(document, "import-secret").input("fake-test-cookie=second-placeholder");
  submitForm(onlyElement(document, ".admin-credentials-import"));
}

function pendingId(document: TestDocument): string | null {
  const ids = new Set(
    document
      .querySelectorAll("[data-command-id]")
      .map((node) => node.getAttribute("data-command-id")),
  );
  expect(ids.size).toBe(1);
  return [...ids][0] ?? null;
}

async function withCredentials(
  run: (
    document: TestDocument,
    browser: ReturnType<typeof installFakeWindow>["fake"],
  ) => Promise<void>,
) {
  const Fixture = await loadCredentialTable();
  const { fake, restore } = installFakeWindow("/audit");
  fake.history.pushState(null, "", PAGE_URL);
  const harness = createAdminRequestHarness();
  const { host, release } = registerAdminSessionHost({
    check() {},
    writesAllowed: () => true,
    confirmed: () => ({
      ...TEST_SESSION,
      actor: "fixture",
      csrfToken: "csrf-fixture",
      csrf_token: "csrf-fixture",
    }),
  });
  try {
    harness.installBrowserRequest();
    await withT10View(
      () => Fixture({ host }),
      (document) => run(document, fake),
    );
  } finally {
    release();
    harness.cleanup();
    restore();
  }
}

function replyAt(...replies: readonly (() => Response | Promise<Response>)[]): WriteReply {
  let index = 0;
  return (request) => {
    if (request.path === "/api/v1/browser-errors") return new Response(null, { status: 204 });
    const reply = replies[index++];
    if (reply === undefined) throw new Error("出现计划之外的凭据请求");
    return reply();
  };
}

for (const method of ["PATCH", "DELETE"] as const) {
  test(`credential_${method}_version_mismatch_keeps_the_command_and_entity_locked`, async () => {
    const credentials = await import("../../api/credentials");
    const read = spyOn(credentials, "fetchAdminCredentials");
    read.mockResolvedValueOnce(ok(projection()));
    const refreshed = Promise.withResolvers<Result<AdminCredentialPage, string>>();
    read.mockImplementationOnce(() => refreshed.promise);
    const accepted = Promise.withResolvers<Response>();
    const deletion = { credential_id: CREDENTIAL.id, state_version: 2, restart_required: true };
    const transport = installWriteFetch(
      replyAt(
        () =>
          receipt(
            method === "PATCH"
              ? { credential: { ...DISABLED, state_version: 1 }, restart_required: true }
              : { ...deletion, state_version: 1 },
          ),
        () => accepted.promise,
      ),
    );
    const writes = () => transport.writes.filter(({ path }) => path === ENDPOINT);
    try {
      await withCredentials(async (document) => {
        if (method === "PATCH") {
          await clickButton(document, "停用");
        } else {
          await clickButton(document, "删除");
          const confirm = onlyElement(document, '[role="dialog"]').querySelector(
            ".admin-btn-danger",
          );
          if (confirm === null) throw new Error("缺少删除确认按钮");
          confirm.click();
          await settle();
        }
        expect(read).toHaveBeenCalledTimes(1);
        expect(hasText(document, RESTART)).toBe(false);
        expect(writes().map((request) => [request.method, request.path])).toEqual([
          [method, ENDPOINT],
        ]);
        const first = writes()[0];
        if (first === undefined) throw new Error("缺少首次写入");
        const command = parseJsonObject(first.body, "凭据操作");
        const commandId = command.command_id;
        if (typeof commandId !== "string") throw new Error("缺少命令 ID");
        expect(pendingId(document)).toBe(commandId);
        for (const name of ["停用", "改名", "删除"]) {
          const control = button(document, name);
          expect(control.hasAttribute("disabled")).toBe(true);
          control.dispatchClick();
        }
        await settle();
        expect(writes()).toHaveLength(1);

        await clickButton(document, "原样重试");
        expect(writes()).toHaveLength(2);
        const retry = writes()[1];
        expect(retry?.method).toBe(method);
        expect(retry?.path).toBe(ENDPOINT);
        expect(retry?.body === first.body).toBe(true);
        expect(pendingId(document)).toBe(commandId);
        expect(read).toHaveBeenCalledTimes(1);

        accepted.resolve(
          receipt(method === "PATCH" ? { credential: DISABLED, restart_required: true } : deletion),
        );
        await settle();
        expect(hasText(document, RESTART)).toBe(true);
        expect(document.querySelector("[data-command-id]")).toBeNull();
        expect(findButton(document, "原样重试")).toBeNull();
        expect(read).toHaveBeenCalledTimes(2);
        refreshed.resolve(
          ok(method === "PATCH" ? projection(DISABLED) : { ...projection(), credentials: [] }),
        );
        await settle();
        if (method === "PATCH") {
          expect(button(document, "停用").hasAttribute("disabled")).toBe(true);
          expect(button(document, "启用").hasAttribute("disabled")).toBe(false);
        } else {
          expect(findButton(document, "停用") === null).toBe(true);
        }
      });
    } finally {
      read.mockRestore();
      transport.restore();
    }
  });
}

test("credential_rename_submits_only_its_own_fields_while_import_is_unknown", async () => {
  const credentials = await import("../../api/credentials");
  const read = spyOn(credentials, "fetchAdminCredentials");
  const renamed = { ...CREDENTIAL, label: "新标签", state_version: 2 };
  read.mockResolvedValueOnce(ok(projection()));
  read.mockResolvedValueOnce(ok(projection(renamed)));
  const unavailable = () =>
    Response.json(
      { code: "storage_unavailable", message: "", data: null, request_id: null },
      { status: 503 },
    );
  const transport = installWriteFetch(
    replyAt(
      unavailable,
      () => receipt({ credential: renamed, restart_required: true }),
      unavailable,
    ),
  );
  try {
    await withCredentials(async (document) => {
      submitImport(document);
      await settle();
      const importId = pendingId(document);
      const first = transport.writes[0];
      if (first === undefined || importId === null) throw new Error("缺少未决导入");

      await clickButton(document, "改名");
      field(document, `label-${CREDENTIAL.id}`).input("新标签");
      await settle();
      const form = onlyElement(document, '[role="dialog"]').querySelector("form");
      if (form === null) throw new Error("缺少改名表单");
      submitForm(form);
      await settle();

      const patch = transport.writes[1];
      expect([patch?.method, patch?.path]).toEqual(["PATCH", ENDPOINT]);
      const body = parseJsonObject(patch?.body ?? "", "凭据改名");
      expect(Object.keys(body).sort()).toEqual([
        "action",
        "command_id",
        "credential_id",
        "expected_state_version",
        "label",
        "status",
      ]);
      expect(
        body.credential_id === CREDENTIAL.id &&
          body.expected_state_version === 1 &&
          body.action === "rename" &&
          body.label === "新标签" &&
          body.status === null &&
          typeof body.command_id === "string" &&
          body.command_id !== importId,
      ).toBe(true);
      expect(read).toHaveBeenCalledTimes(2);
      expect(hasText(document, "新标签")).toBe(true);
      expect(pendingId(document)).toBe(importId);
      await clickButton(document, "原样重试");
      expect(transport.writes.map(({ method, path }) => [method, path])).toEqual([
        ["POST", ENDPOINT],
        ["PATCH", ENDPOINT],
        ["POST", ENDPOINT],
      ]);
      expect(transport.writes[2]?.body === first.body).toBe(true);
      expect(pendingId(document)).toBe(importId);
    });
  } finally {
    read.mockRestore();
    transport.restore();
  }
});

test("credential_late_empty_read_cannot_replace_the_new_generation_page", async () => {
  const credentials = await import("../../api/credentials");
  const read = spyOn(credentials, "fetchAdminCredentials");
  const older = Promise.withResolvers<Result<AdminCredentialPage, string>>();
  const newer = Promise.withResolvers<Result<AdminCredentialPage, string>>();
  read.mockImplementationOnce(() => older.promise);
  read.mockImplementationOnce(() => newer.promise);
  const transport = installWriteFetch(
    replyAt(() =>
      receipt({ credential: { ...CREDENTIAL, label: "新的凭据" }, restart_required: true }),
    ),
  );
  try {
    await withCredentials(async (document, browser) => {
      browser.writes.splice(0);
      expect(read).toHaveBeenCalledTimes(1);
      submitImport(document);
      await settle();
      expect(read.mock.calls.map(([, , , , page, size]) => [page, size])).toEqual([
        [2, 10],
        [2, 10],
      ]);
      newer.resolve(ok(projection()));
      await settle();
      expect(hasText(document, CREDENTIAL.label)).toBe(true);
      older.resolve(ok({ ...projection(), credentials: [], total: 10 }));
      await settle();

      expect(hasText(document, CREDENTIAL.label)).toBe(true);
      expect(browser.location.pathname + browser.location.search).toBe(PAGE_URL);
      expect(browser.writes).toHaveLength(0);
      expect(read).toHaveBeenCalledTimes(2);
      browser.history.back();
      expect(browser.location.pathname).toBe("/audit");
      browser.history.forward();
      expect(browser.location.pathname + browser.location.search).toBe(PAGE_URL);
    });
  } finally {
    read.mockRestore();
    transport.restore();
  }
});

test("credential_page_rejects_a_stale_empty_projection_at_the_correction_boundary", async () => {
  const pagination = await loadRenderModule<typeof import("../pagination")>(
    new URL("../pagination/index.tsx", import.meta.url).pathname,
    "useVersionedAdminPage",
  );
  const page = createCell<Result<readonly [number, AdminCredentialPage], string> | null>(null);
  const credentialsPagination: {
    useVersionedAdminPage: typeof pagination.useVersionedAdminPage<bigint, AdminCredentialPage>;
  } = pagination;
  const versioned = spyOn(credentialsPagination, "useVersionedAdminPage");
  // 只替换这一页的资源交付，独立验证真实页面的最后一道世代门禁。
  versioned.mockImplementationOnce(() => page);
  const transport = installWriteFetch(
    replyAt(() =>
      receipt({ credential: { ...CREDENTIAL, label: "新的凭据" }, restart_required: true }),
    ),
  );
  try {
    await withCredentials(async (document, browser) => {
      browser.writes.splice(0);
      submitImport(document);
      await settle();
      page.set(ok([1, projection()]));
      await settle();
      expect(hasText(document, CREDENTIAL.label)).toBe(true);
      page.set(ok([0, { ...projection(), credentials: [], total: 10 }]));
      await settle();
      expect(browser.location.pathname + browser.location.search).toBe(PAGE_URL);
      expect(browser.writes).toHaveLength(0);
      page.set(ok([1, { ...projection(), credentials: [], total: 10 }]));
      await settle();
      expect(browser.location.search).toBe("?page_size=10");
      expect(browser.writes.map(({ mode }) => mode)).toEqual(["replace"]);
    });
  } finally {
    versioned.mockRestore();
    transport.restore();
  }
});
