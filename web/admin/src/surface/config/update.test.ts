// 运行配置保存回执与草稿基线、原样重试和命令后的重读。

import { afterEach, beforeEach, expect, spyOn, test } from "bun:test";
import { err } from "@auction/web-shared/result";
import { flush } from "solid-js";
import { cleanupScope, preserveProperty } from "../../test-scope-fixture";
import { required } from "../solid-dom-fixture";
import { configDocumentPage } from "./config-document-fixture.test";
import { clickButton, deferredReply, maybeButton, settle } from "./config-page-fixture.test";
import {
  accepted,
  apiFailure,
  configReadReply,
  defaultProjection,
  objectAt,
} from "./config-projection-fixture.test";
import { beginConfigTestLock, endConfigTestLock } from "./config-test-scope-fixture.test";
import { runtimeConfigFormSignature } from "./model";

const LEASE = "operations.forum.lease_duration_seconds";
const SAVED = "运行时配置已写入 TOML 文件。";

function projection(lease: number) {
  const snapshot = defaultProjection();
  const forum = objectAt(snapshot.data.document, "operations.forum");
  forum.managed = true;
  forum.lease_duration_seconds = lease;
  forum.item_timeout_seconds = 240;
  return snapshot;
}

beforeEach(beginConfigTestLock);
afterEach(endConfigTestLock);

async function checkReplayedConfig(failure: "candidate" | "form"): Promise<void> {
  const originalFingerprint = `sha256:${"0".repeat(64)}`;
  const savedFingerprint = `sha256:${"1".repeat(64)}`;
  const original = projection(300);
  const saved = projection(600);
  const mismatch = projection(failure === "candidate" ? 601 : 600);
  const mismatchFingerprint = `sha256:${"2".repeat(64)}`;
  saved.data.restart_required = true;
  original.data.fingerprint = originalFingerprint;
  saved.data.fingerprint = savedFingerprint;
  mismatch.data.restart_required = true;
  mismatch.data.fingerprint = mismatchFingerprint;
  let writes = 0;
  let reads = 0;
  const refreshed = deferredReply();
  const model = await import("./model");
  const fromResponse = model.runtimeConfigFormFromResponse;
  const page = configDocumentPage((request) => {
    if (request.method === "PUT") {
      writes += 1;
      return Response.json(writes === 1 ? mismatch : saved);
    }
    reads += 1;
    return reads === 1 ? configReadReply(original) : refreshed.wait();
  });
  const project = spyOn(model, "runtimeConfigFormFromResponse");
  try {
    // 只令送达写入结果的表单投影失败，HTTP 解码和候选核对仍走真实路径。
    project.mockImplementation((response) =>
      failure === "form" && response.fingerprint === mismatchFingerprint
        ? err(null)
        : fromResponse(response),
    );
    page.mount();
    await settle();
    const lease = required(
      page.root
        .querySelectorAll("[data-runtime-number]")
        .find((input) => input.getAttribute("data-runtime-number") === LEASE),
      "租约时长",
    );
    lease.input("600");
    flush();
    page.draft.checkedSignature.set(accepted(runtimeConfigFormSignature(page.draft.form.peek())));
    flush();
    clickButton(page.root, "写入配置");
    await settle();
    clickButton(page.root, "确认");
    await settle();

    expect(reads).toBe(1);
    expect(page.root.textContent).toContain(originalFingerprint);
    expect(page.draft.fingerprint.peek()).toBe(originalFingerprint);
    expect(page.root.textContent).not.toContain(SAVED);
    expect(lease.value).toBe("600");

    clickButton(page.root, "原样重试");
    await settle();
    const requests = page.requests;
    expect(requests.map(({ method }) => method)).toEqual(["GET", "PUT", "PUT", "GET"]);
    expect(requests[2]?.body).toBe(requests[1]?.body);
    expect(JSON.parse(required(requests[1], "保存命令").body)).toMatchObject({
      command_id: expect.stringMatching(/^[\da-f-]{36}$/u),
      expected_fingerprint: originalFingerprint,
      document: { operations: { forum: { lease_duration_seconds: 600 } } },
    });
    expect(maybeButton(page.root, "原样重试")).toBeNull();
    expect(page.draft.fingerprint.peek()).toBe(savedFingerprint);
    refreshed.respond(configReadReply(saved));
    await settle();
    expect(page.root.textContent).toContain(savedFingerprint);
    expect(page.root.textContent).toContain(SAVED);
  } finally {
    project.mockRestore();
    page.dispose();
  }
}

test("配置回显错配保留编辑基线直到原样重试成功", () => checkReplayedConfig("candidate"));

test("配置回执无法投影为表单时保留原命令和编辑基线", () => checkReplayedConfig("form"));

test("配置保存结果未知后收到 409 state_conflict 仍可原样重试", async () => {
  const original = projection(300);
  const fingerprint = `sha256:${"0".repeat(64)}`;
  original.data.fingerprint = fingerprint;
  let writes = 0;
  const scope = cleanupScope();
  const tab = new EventTarget();
  const canUnload = () => tab.dispatchEvent(new Event("beforeunload", { cancelable: true }));
  const unsaved = "有未保存的修改。";
  scope.defer(preserveProperty(globalThis, "window"));
  try {
    Object.defineProperty(globalThis, "window", { configurable: true, value: tab });
    const page = configDocumentPage((request) => {
      if (request.method === "GET") return configReadReply(original);
      expect([request.method, request.path]).toEqual(["PUT", "/api/v1/admin/config"]);
      writes += 1;
      return writes === 2
        ? apiFailure(409, "state_conflict", "配置命令冲突")
        : apiFailure(503, "storage_unavailable", "暂不可用");
    });
    scope.defer(page.dispose);
    page.mount();
    await settle();
    expect(page.root.textContent).not.toContain(unsaved);
    expect(canUnload()).toBe(true);
    const lease = required(
      page.root
        .querySelectorAll("[data-runtime-number]")
        .find((input) => input.getAttribute("data-runtime-number") === LEASE),
      "租约时长",
    );
    lease.input("600");
    flush();
    page.draft.checkedSignature.set(accepted(runtimeConfigFormSignature(page.draft.form.peek())));
    flush();
    clickButton(page.root, "写入配置");
    await settle();
    clickButton(page.root, "确认");
    await settle();
    expect(page.root.textContent).toContain(unsaved);
    expect(canUnload()).toBe(false);

    clickButton(page.root, "原样重试");
    await settle();
    expect(writes).toBe(2);
    expect(maybeButton(page.root, "原样重试")).not.toBeNull();
    expect(page.draft.fingerprint.peek()).toBe(fingerprint);
    expect(lease.value).toBe("600");
    expect(page.root.textContent).not.toContain(SAVED);
    expect(page.root.textContent).toContain(unsaved);
    expect(canUnload()).toBe(false);

    clickButton(page.root, "原样重试");
    await settle();
    const requests = page.requests;
    expect(requests.map(({ method }) => method)).toEqual(["GET", "PUT", "PUT", "PUT"]);
    const first = required(requests[1], "原配置命令").body;
    expect(requests.slice(1).map(({ body }) => body)).toEqual([first, first, first]);
    expect(JSON.parse(first)).toMatchObject({
      command_id: expect.stringMatching(/^[\da-f-]{36}$/u),
      expected_fingerprint: fingerprint,
      document: { operations: { forum: { lease_duration_seconds: 600 } } },
    });
  } finally {
    scope.dispose();
  }
});
