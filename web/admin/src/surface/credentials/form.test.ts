// 凭据导入的服务端错误归属、编辑离焦规则和字段播报。

import { expect, spyOn, test } from "bun:test";
import { flush } from "solid-js";
import { createAdminRequestHarness } from "../../api/api-request-fixture";
import type { TestDocument, TestElement } from "../solid-dom-fixture";
import { settle } from "../solid-dom-fixture";
import {
  field,
  loadCredentialForm,
  onlyElement,
  submitForm,
  withT10View,
} from "../t10-view-fixture.test";
import {
  credentialFieldErrorsFirstInvalid,
  credentialFieldErrorsFromServer,
} from "./field-validation";

test("credential_conflict_stays_form_level_and_a_label_error_targets_the_label", () => {
  for (const cookieSource of [true, false]) {
    expect(
      credentialFieldErrorsFirstInvalid(
        credentialFieldErrorsFromServer("credential_conflict", cookieSource),
      ),
    ).toBeNull();
    expect(
      credentialFieldErrorsFirstInvalid(
        credentialFieldErrorsFromServer("invalid_credential_label", cookieSource),
      )?.[0],
    ).toBe("import-label");
  }
});

async function withForm(run: (document: TestDocument, form: TestElement) => Promise<void> | void) {
  const Fixture = await loadCredentialForm();
  const request = createAdminRequestHarness();
  try {
    const { host } = request.registerHost();
    const requests = request.installFetch([]);
    await withT10View(
      () => Fixture({ host }),
      async (document) => {
        const form = onlyElement(document, "form");
        const label = field(document, "import-label");
        expect(typeof label.attributes.onInput).toBe("function");
        expect(typeof label.attributes.onBlur).toBe("function");
        await run(document, form);
        expect(requests).toHaveLength(0);
      },
    );
  } finally {
    request.cleanup();
  }
}

function validity(node: TestElement, invalid: boolean, describedBy: string | null) {
  expect(node.getAttribute("aria-invalid")).toBe(String(invalid));
  expect(node.getAttribute("aria-describedby")).toBe(describedBy);
}

function validityWrites(node: TestElement, dispatch: () => void): Record<string, unknown[]> {
  const original = Object.getOwnPropertyDescriptor(node, "attributes");
  if (original === undefined) throw new Error("控件缺少内存属性表");
  const writes: Record<string, unknown[]> = {};
  // 内存渲染器直接写属性表，记录单次事件导致的写入，包括写回原值。
  const attributes = new Proxy(node.attributes, {
    set(target, key, value: unknown) {
      if (key === "aria-invalid" || key === "aria-describedby") {
        writes[key] ??= [];
        writes[key].push(value);
      }
      return Reflect.set(target, key, value);
    },
  });
  try {
    Object.defineProperty(node, "attributes", { ...original, value: attributes });
    dispatch();
    flush();
    return writes;
  } finally {
    Object.defineProperty(node, "attributes", original);
  }
}

test("submitted_label_error_survives_still_invalid_edits_and_clears_on_correction", async () => {
  await withForm((document, form) => {
    const label = field(document, "import-label");
    expect(validityWrites(label, () => submitForm(form))).toEqual({
      "aria-invalid": ["true"],
      "aria-describedby": ["import-label-error"],
    });
    validity(label, true, "import-label-error");

    expect(validityWrites(label, () => label.input(" "))).toEqual({});
    validity(label, true, "import-label-error");

    expect(validityWrites(label, () => label.input("账号一"))).toEqual({
      "aria-invalid": ["false"],
      "aria-describedby": [undefined],
    });
    validity(label, false, null);
  });
});

test("blank_cookie_keeps_its_error_through_input_blur_and_submit", async () => {
  await withForm((document, form) => {
    const label = field(document, "import-label");
    const cookie = field(document, "import-secret");
    label.input("账号一");
    flush();
    expect(validityWrites(cookie, () => submitForm(form))["aria-invalid"]).toEqual(["true"]);
    validity(cookie, true, "import-secret-help import-secret-error");
    for (const [event, dispatch] of [
      ["input", () => cookie.input(" \t")],
      ["blur", () => cookie.blur()],
      ["submit", () => submitForm(form)],
    ] as const) {
      expect(validityWrites(cookie, dispatch), event).toEqual({});
      validity(cookie, true, "import-secret-help import-secret-error");
    }
    expect(validityWrites(cookie, () => cookie.input("example=value"))).toEqual({
      "aria-invalid": ["false"],
      "aria-describedby": ["import-secret-help"],
    });
    validity(cookie, false, "import-secret-help");

    expect(validityWrites(cookie, () => cookie.input(" \u{3000}"))["aria-invalid"]).toBeUndefined();
    validity(cookie, false, "import-secret-help");
    expect(validityWrites(cookie, () => cookie.blur())["aria-invalid"]).toEqual(["true"]);
    validity(cookie, true, "import-secret-help import-secret-error");
  });
});

test("label_blur_validates_only_after_input_with_the_submit_rule", async () => {
  await withForm((document) => {
    const label = field(document, "import-label");
    expect(validityWrites(label, () => label.blur())["aria-invalid"]).toBeUndefined();
    validity(label, false, null);

    expect(validityWrites(label, () => label.input(" "))["aria-invalid"]).toBeUndefined();
    validity(label, false, null);
    expect(validityWrites(label, () => label.blur())).toEqual({
      "aria-invalid": ["true"],
      "aria-describedby": ["import-label-error"],
    });
    validity(label, true, "import-label-error");
  });
});

test("correcting_cookie_keeps_the_pending_label_announcement", async () => {
  const timer = await import("@auction/web-shared/platform/timer");
  const pending = Promise.withResolvers<boolean>();
  let restoreSleep: (() => void) | undefined;
  try {
    const sleep = spyOn(timer, "sleep");
    restoreSleep = () => sleep.mockRestore();
    // 零延时是渲染后的移焦，只有播报等待由用例唤醒。
    sleep.mockImplementation((delay, signal) =>
      delay === 0 ? Promise.resolve(signal?.aborted !== true) : pending.promise,
    );
    const announcements = () => sleep.mock.calls.filter(([delay]) => delay !== 0);
    await withForm(async (document, form) => {
      const label = field(document, "import-label");
      const cookie = field(document, "import-secret");
      const live = onlyElement(document, "[aria-live]");
      expect(live.getAttribute("aria-live")).toBe("polite");
      label.input("账号一");
      flush();
      submitForm(form);
      await settle();
      expect(cookie.getAttribute("aria-invalid")).toBe("true");
      expect(announcements()).toHaveLength(0);

      label.input(" ");
      label.blur();
      await settle();
      expect(label.getAttribute("aria-invalid")).toBe("true");
      expect(live.textContent).toBe("");
      expect(announcements()).toHaveLength(1);
      const wait = announcements()[0];
      expect(wait?.[0]).toBe(500);
      const signal = wait?.[1];
      if (signal === undefined) throw new Error("标签播报没有等待可取消的定时器");
      expect(signal.aborted).toBe(false);

      cookie.input("example=value");
      await settle();
      expect(cookie.getAttribute("aria-invalid")).toBe("false");
      expect(live.textContent).toBe("");
      expect(announcements()).toHaveLength(1);
      expect(signal.aborted).toBe(false);

      pending.resolve(true);
      await settle();
      expect(live.textContent).toBe("请填写此字段。");
    });
  } finally {
    pending.resolve(false);
    restoreSleep?.();
  }
});
