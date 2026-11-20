// 挂载生产认证外壳和用户页，核对匿名发现、退出决定与一次性 Key 的可见性。

import { expect, test } from "bun:test";
import { adminWriteFailureDisplayCode } from "../../api/failure";
import {
  prepareAdminBusinessWorldSetEnabledOnce,
  setAdminBusinessWorldEnabled,
} from "../../api/worlds";
import { required } from "../solid-dom-fixture";
import { button, field, findButton, submitForm } from "../t10-view-fixture.test";
import { type AuthTestDocument, settleAuth } from "./auth-environment-fixture";
import { failure, response, sessionBody, unauthorized } from "./effects-session-fixture";
import {
  assertKeyHidden,
  beginCreate,
  COOKIE_A,
  COOKIE_B,
  click,
  GENERATED,
  generatedUser,
  text,
  VIEW_KEY,
  withLayout,
} from "./session-render-fixture.test";

const KEY_HIDDEN = "确认当前登录状态之前，暂不显示生成的 Key。";

test("已登出标签隐藏时不探测，恢复可见与广播合并一次；401 或 503 保留真实表单和焦点", async () => {
  for (const reply of [unauthorized(), failure(503, "storage_unavailable")]) {
    await withLayout("/login?return=%2Fusers", async ({ stubs, document, broadcast, pathname }) => {
      stubs.get.finish(unauthorized());
      await settleAuth();
      const actor = field(document, "admin-actor");
      const secret = field(document, "admin-secret");
      actor.input("fixture-operator");
      secret.input("仅供测试的密码");
      secret.focus();
      document.setHidden(true);
      broadcast.deliver();
      await settleAuth();
      expect(stubs.get.calls).toBe(1);
      expect(document.activeElement).toBe(secret);

      document.setHidden(false);
      broadcast.deliver();
      document.setHidden(false);
      await settleAuth();
      expect(stubs.get.calls).toBe(2);
      expect(stubs.post.calls).toBe(0);
      stubs.get.finish(reply);
      await settleAuth();
      expect(pathname()).toBe("/login");
      expect(document.getElementById("admin-actor")).toBe(actor);
      expect(document.getElementById("admin-secret")).toBe(secret);
      expect(actor.value).toBe("fixture-operator");
      expect(secret.value).toBe("仅供测试的密码");
      expect(document.activeElement).toBe(secret);
      expect(stubs.get.calls).toBe(2);
      expect(broadcast.sent).toEqual([]);
    });
  }
});

test("没有广播通道时恢复可见仍只读一次，有效会话按站内 return 直接进入后台", async () => {
  await withLayout(
    "/login?return=%2Fusers",
    async ({ stubs, document, pathname }) => {
      stubs.get.finish(unauthorized());
      await settleAuth();
      expect(document.getElementById("admin-actor")).not.toBeNull();
      document.setHidden(true);
      document.setHidden(false);
      document.setHidden(false);
      await settleAuth();
      expect(stubs.get.calls).toBe(2);
      stubs.get.finish(sessionBody());
      await settleAuth();
      expect(pathname()).toBe("/users");
      expect(document.getElementById("admin-actor")).toBeNull();
      expect(stubs.post.calls).toBe(0);
      expect(stubs.get.calls).toBe(2);
    },
    false,
  );
});

test("登录专属 GET 在同代广播后保留表单节点、焦点和唯一请求", async () => {
  await withLayout("/login?return=%2Fusers", async ({ stubs, document, broadcast, pathname }) => {
    stubs.get.finish(unauthorized());
    await settleAuth();
    const actor = field(document, "admin-actor");
    const secret = field(document, "admin-secret");
    actor.input("fixture-operator");
    secret.input("fixture-password");
    secret.focus();
    submitForm(required(document.querySelectorAll("form")[0], "登录表单"));
    await settleAuth();
    stubs.post.finish({ kind: "network" });
    await settleAuth();
    expect(stubs.get.calls).toBe(2);
    broadcast.deliver();
    await settleAuth();
    expect(document.getElementById("admin-actor")).toBe(actor);
    expect(document.activeElement).toBe(secret);
    expect(stubs.get.calls).toBe(2);
    expect(stubs.get.cancelled).toBe(0);
    stubs.get.finish(sessionBody());
    await settleAuth();
    expect(pathname()).toBe("/users");
    expect(stubs.post.calls).toBe(1);
    expect(broadcast.sent).toEqual([]);
  });
});

test("退出未确认的两个按钮随请求禁用，复核同代后可选择留在后台或继续退出", async () => {
  for (const stay of [false, true]) {
    await withLayout("/users", async ({ stubs, document, broadcast }) => {
      stubs.get.finish(sessionBody());
      await settleAuth();
      const draft = field(document, "user-name-filter");
      draft.input("未提交的筛选草稿");
      click(document, "退出登录");
      await settleAuth();
      stubs.logout.finish({ kind: "network" });
      await settleAuth();
      expect(text(document)).toContain("退出尚未确认，此标签页的写入已关闭。");
      expect(field(document, "user-name-filter")).toBe(draft);
      expect(draft.value).toBe("未提交的筛选草稿");
      for (const label of ["继续退出", "留在后台"])
        expect(button(document, label).hasAttribute("disabled")).toBe(true);
      stubs.get.finish(sessionBody());
      await settleAuth();
      expect(stubs.logout.calls).toBe(1);
      for (const label of ["继续退出", "留在后台"])
        expect(button(document, label).hasAttribute("disabled")).toBe(false);
      click(document, stay ? "留在后台" : "继续退出");
      await settleAuth();
      expect(stubs.logout.calls).toBe(stay ? 1 : 2);
      for (const label of ["继续退出", "留在后台"])
        expect(button(document, label).hasAttribute("disabled")).toBe(true);
      if (stay) {
        stubs.get.finish(sessionBody());
        await settleAuth();
        expect(findButton(document, "继续退出")).toBeNull();
        expect(field(document, "user-name-filter")).toBe(draft);
        expect(broadcast.sent).toEqual([]);
      } else {
        document.cookie = "";
        stubs.logout.finish(response(200, '{"code":"ok","message":"已退出","data":null}'));
        await settleAuth();
        expect(findButton(document, "继续退出")).toBeNull();
        expect(document.getElementById("user-name-filter")).toBeNull();
        expect(broadcast.sent).toEqual(["session-ended"]);
      }
    });
  }
});

function closeCallback(document: AuthTestDocument): () => void {
  const close = button(document, "关闭");
  return () => close.click();
}

for (const point of ["登记", "实发"] as const) {
  test(`真实来源页的 Key 在同代 503 保持显示，${point}换代时遮蔽并保留草稿及回执`, async () => {
    await withLayout("/users", async (fixture) => {
      const { stubs, document, broadcast } = fixture;
      stubs.get.finish(sessionBody());
      await settleAuth();
      const draft = field(document, "user-name-filter");
      draft.input("未提交草稿");
      await beginCreate(fixture);
      stubs.createUser.finish(generatedUser());
      await settleAuth();
      expect(document.querySelectorAll("code").map((node) => node.textContent)).toContain(
        GENERATED,
      );
      const closeOld = closeCallback(document);
      document.setHidden(false);
      stubs.get.finish(failure(503, "storage_unavailable"));
      await settleAuth();
      expect(document.querySelectorAll("code").map((node) => node.textContent)).toContain(
        GENERATED,
      );
      document.cookie = COOKIE_B;
      if (point === "实发") document.cookieReads.push(COOKIE_A, COOKIE_B);
      document.setHidden(false);
      await settleAuth();
      assertKeyHidden(document);
      expect(button(document, VIEW_KEY).hasAttribute("disabled")).toBe(true);
      expect(text(document)).toContain(KEY_HIDDEN);
      expect(field(document, "user-name-filter")).toBe(draft);
      expect(draft.value).toBe("未提交草稿");
      document.cookie = COOKIE_A;
      document.setHidden(false);
      stubs.get.finish(sessionBody());
      await settleAuth();
      assertKeyHidden(document);
      click(document, VIEW_KEY);
      await settleAuth();
      expect(document.querySelectorAll("code").map((node) => node.textContent)).toContain(
        GENERATED,
      );
      closeOld();
      await settleAuth();
      expect(document.querySelectorAll("code").map((node) => node.textContent)).toContain(
        GENERATED,
      );
      document.setHidden(false);
      stubs.get.finish(unauthorized());
      await settleAuth();
      assertKeyHidden(document);
      expect(document.getElementById("user-name-filter")).toBeNull();
      expect(findButton(document, VIEW_KEY)).toBeNull();
      expect(broadcast.sent).toEqual(["session-ended"]);
    });
  });
}

test("遮蔽时交付到真实来源页的 Key 恢复后仍须手动开启", async () => {
  await withLayout("/users", async (fixture) => {
    const { stubs, document } = fixture;
    stubs.get.finish(sessionBody());
    await settleAuth();
    await beginCreate(fixture);
    document.cookie = COOKIE_B;
    document.setHidden(false);
    stubs.createUser.finish(generatedUser());
    await settleAuth();
    assertKeyHidden(document);
    expect(button(document, VIEW_KEY).hasAttribute("disabled")).toBe(true);

    document.cookie = COOKIE_A;
    document.setHidden(false);
    stubs.get.finish(sessionBody());
    await settleAuth();
    assertKeyHidden(document);
    click(document, VIEW_KEY);
    await settleAuth();
    expect(document.querySelectorAll("code").map((node) => node.textContent)).toContain(GENERATED);
    closeCallback(document)();
    await settleAuth();
    assertKeyHidden(document);
    expect(findButton(document, VIEW_KEY)).toBeNull();
  });
});

test("外壳接管离页交付的 Key，收包才换代时遮蔽；同代恢复后手动重开且旧关闭无效", async () => {
  await withLayout("/users", async (fixture) => {
    const { stubs, document, router } = fixture;
    stubs.get.finish(sessionBody());
    await settleAuth();
    await beginCreate(fixture);
    router.push("/status");
    await settleAuth();
    stubs.createUser.finish(generatedUser());
    await settleAuth();
    assertKeyHidden(document);
    click(document, VIEW_KEY);
    await settleAuth();
    expect(document.querySelectorAll("code").map((node) => node.textContent)).toContain(GENERATED);
    const closeOld = closeCallback(document);
    document.setHidden(false);
    document.cookie = COOKIE_B;
    stubs.get.finish(sessionBody());
    await settleAuth();
    assertKeyHidden(document);
    expect(button(document, VIEW_KEY).hasAttribute("disabled")).toBe(true);
    expect(document.querySelectorAll(".admin-command-notices__receipt")).toHaveLength(1);

    document.cookie = COOKIE_A;
    document.setHidden(false);
    stubs.get.finish(sessionBody());
    await settleAuth();
    assertKeyHidden(document);
    click(document, VIEW_KEY);
    await settleAuth();
    closeOld();
    await settleAuth();
    expect(document.querySelectorAll("code").map((node) => node.textContent)).toContain(GENERATED);
    expect(document.querySelectorAll(".admin-command-notices__receipt")).toHaveLength(1);
    closeCallback(document)();
    await settleAuth();
    assertKeyHidden(document);
    expect(document.querySelectorAll(".admin-command-notices__receipt")).toHaveLength(0);
  });
});

test("外壳两张回执中旧 X 的关闭晚到，Y 的 Key 和两张回执都保留", async () => {
  await withLayout("/users", async (fixture) => {
    const { stubs, document, router } = fixture;
    const secondKey = `${"Y".repeat(42)}A`;
    stubs.get.finish(sessionBody());
    await settleAuth();
    await beginCreate(fixture);
    router.push("/status");
    await settleAuth();
    stubs.createUser.finish(generatedUser());
    await settleAuth();
    click(document, VIEW_KEY);
    await settleAuth();
    const closeX = closeCallback(document);

    document.cookie = COOKIE_B;
    document.setHidden(false);
    await settleAuth();
    assertKeyHidden(document);
    document.cookie = COOKIE_A;
    document.setHidden(false);
    stubs.get.finish(sessionBody());
    await settleAuth();
    assertKeyHidden(document);
    router.push("/users");
    await settleAuth();
    await beginCreate(fixture, "SecondPlayer");
    router.push("/status");
    await settleAuth();
    stubs.createUser.finish(
      generatedUser("SecondPlayer", secondKey, "00000000-0000-4000-8000-000000000027"),
    );
    await settleAuth();

    const receipts = document.querySelectorAll(".admin-command-notices__receipt");
    expect(receipts).toHaveLength(2);
    const newer = required(
      receipts.find((receipt) => receipt.textContent.includes("SecondPlayer")),
      "Y 回执",
    );
    required(newer.querySelector("button"), "查看 Y").click();
    await settleAuth();
    expect(document.querySelectorAll("code").map((node) => node.textContent)).toContain(secondKey);
    closeX();
    await settleAuth();
    expect(document.querySelectorAll("code").map((node) => node.textContent)).toContain(secondKey);
    expect(document.querySelectorAll(".admin-command-notices__receipt")).toHaveLength(2);
  });
});

test("写前复核认出异代正文时，挂起或 503 都收起真实外壳 Key 并保留回执", async () => {
  await withLayout("/users", async (fixture) => {
    const { stubs, document, router, requests, broadcast } = fixture;
    stubs.get.finish(sessionBody());
    await settleAuth();
    await beginCreate(fixture);
    router.push("/test-authenticated-commands");
    await settleAuth();
    stubs.createUser.finish(generatedUser());
    await settleAuth();
    click(document, VIEW_KEY);
    await settleAuth();
    const host = required(requests.hosts[0], "已认证命令页宿主").session;
    const prepared = prepareAdminBusinessWorldSetEnabledOnce(
      "main",
      false,
      2,
    )("00000000-0000-4000-8000-000000000011");
    if (!prepared.ok) throw new Error("应能冻结真实业务命令");
    const write = setAdminBusinessWorldEnabled(host, prepared.value, new AbortController().signal);
    document.cookieReads.push(null, null);
    stubs.get.finish(sessionBody("cookie-b"));
    const failure = await write;
    if (failure.ok) throw new Error("异代正文不应授权业务写");
    expect(adminWriteFailureDisplayCode(failure.error)).toBe("unauthenticated");
    await settleAuth();
    expect(stubs.get.pendingCount()).toBe(1);
    expect(stubs.businessWrite.calls).toBe(0);
    assertKeyHidden(document);
    expect(button(document, VIEW_KEY).hasAttribute("disabled")).toBe(true);
    expect(document.querySelectorAll(".admin-command-notices__receipt")).toHaveLength(1);
    document.cookieReads.push(null);
    stubs.get.finish(
      response(
        503,
        '{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}',
      ),
    );
    await settleAuth();
    assertKeyHidden(document);
    expect(document.querySelectorAll(".admin-command-notices__receipt")).toHaveLength(1);
    expect(broadcast.sent).toEqual([]);
  });
});
