// 用真实登录表单与会话适配器验证提交后的焦点恢复。

import { expect, test } from "bun:test";
import { createRoot, flush } from "solid-js";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { cleanupScope } from "../../test-scope-fixture";
import { installAdminTranslation } from "../admin-locale-fixture.test";
import { createComponent, mountNode, required, type TestDocument } from "../solid-dom-fixture";
import { installAuthDocument, settleAuth } from "./auth-environment-fixture";
import { useSession } from "./effects";
import { failure, installFetch, transport } from "./effects-session-fixture";

async function withLogin(
  check: (document: TestDocument, requests: ReturnType<typeof transport>) => Promise<void>,
): Promise<void> {
  const { LoginHarness } = await loadRenderModule<typeof import("./effects-fixture")>(
    new URL("./effects-fixture.tsx", import.meta.url).pathname,
    "LoginHarness",
  );
  const scope = cleanupScope();
  try {
    const { document, restore } = installAuthDocument();
    scope.defer(restore);
    scope.defer(installAdminTranslation((id) => id));
    const requests = transport();
    scope.defer(installFetch(requests));
    createRoot((disposeRoot) => {
      scope.defer(() => {
        try {
          disposeRoot();
        } finally {
          flush();
        }
      });
      const session = useSession();
      mountNode(document, createComponent(LoginHarness, { session }));
    });
    await settleAuth();
    expect(requests.get.calls).toBe(1);
    requests.get.finish(failure(401, "unauthenticated"));
    await settleAuth();
    await check(document, requests);
  } finally {
    scope.dispose();
  }
}

function filledForm(document: TestDocument, username: string) {
  const actor = required(document.getElementById("admin-actor"), "账号框");
  const secret = required(document.getElementById("admin-secret"), "密码框");
  const form = required(document.querySelectorAll("form")[0], "登录表单");
  actor.input(username);
  secret.input("fixture-password");
  flush();
  return { actor, secret, form };
}

test("账号只有空格时，从密码框提交会显示本地错误并聚焦账号框", async () => {
  await withLogin(async (document, requests) => {
    const { actor, secret, form } = filledForm(document, " ");
    // 两个控件的值均非空，模拟在密码框按回车后浏览器派发的提交事件。
    expect(actor.hasAttribute("required")).toBe(true);
    expect(actor.value).toBe(" ");
    expect(secret.hasAttribute("required")).toBe(true);
    expect(secret.value).not.toBe("");
    secret.focus();
    expect(document.activeElement).toBe(secret);
    form.submit();
    await settleAuth();

    expect(document.getElementById("admin-login-error")?.getAttribute("role")).toBe("alert");
    expect(actor.getAttribute("aria-invalid")).toBe("true");
    expect(document.activeElement).toBe(actor);
    expect(requests.post.calls).toBe(0);
    expect(requests.get.calls).toBe(1);
  });
});

test("不编辑输入，连续两次相同的本地失败都会从密码框聚焦回账号框", async () => {
  await withLogin(async (document, requests) => {
    const { actor, secret, form } = filledForm(document, " ");
    for (let attempt = 0; attempt < 2; attempt += 1) {
      secret.focus();
      expect(document.activeElement).toBe(secret);
      form.submit();
      await settleAuth();
      expect(document.activeElement).toBe(actor);
    }
    expect(requests.post.calls).toBe(0);
    expect(requests.get.calls).toBe(1);
  });
});

test("登录请求与会话复核期间不抢焦点，服务端拒绝后聚焦密码框", async () => {
  await withLogin(async (document, requests) => {
    const { actor, secret, form } = filledForm(document, "fixture-operator");
    actor.focus();
    form.submit();
    await settleAuth();
    expect(requests.post.calls).toBe(1);
    expect(form.getAttribute("aria-busy")).toBe("true");
    expect(document.activeElement).toBe(actor);

    requests.post.finish(failure(401, "invalid_credentials"));
    await settleAuth();
    expect(requests.get.calls).toBe(2);
    expect(document.activeElement).toBe(actor);

    requests.get.finish(failure(401, "unauthenticated"));
    await settleAuth();
    expect(form.getAttribute("aria-busy")).toBe("false");
    expect(document.getElementById("admin-login-error")?.getAttribute("role")).toBe("alert");
    expect(secret.value).toBe("");
    expect(document.activeElement).toBe(secret);
  });
});
