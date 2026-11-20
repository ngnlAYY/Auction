// 真实会话适配器接收写前 GET、业务读写的 HTTP 失效证据与两类退出交错。

import { expect, test } from "bun:test";
import { adminWriteFailureDisplayCode } from "../../api/failure";
import { registerAdminSessionHost } from "../../api/session";
import {
  fetchAdminBusinessWorlds,
  prepareAdminBusinessWorldSetEnabledOnce,
  setAdminBusinessWorldEnabled,
} from "../../api/worlds";
import { cleanupScope } from "../../test-scope-fixture";
import { installAuthBroadcast, settleAuth } from "./auth-environment-fixture";
import { authenticated, type MountedSession } from "./effects-mount-fixture.test";
import {
  type FetchReply,
  failure,
  response,
  sessionBody,
  type Transport,
  transport,
  unauthorized,
} from "./effects-session-fixture";

const SESSION_A = "auction_admin_csrf=fixture-cookie";
const SESSION_B = "auction_admin_csrf=cookie-b";
const COMMAND_ID = "00000000-0000-4000-8000-000000000011";
const SOURCES = ["写前 GET", "业务 GET", "业务 PATCH"] as const;
type Source = (typeof SOURCES)[number];

function registerBusinessHost(mounted: MountedSession) {
  return registerAdminSessionHost({
    check: () => mounted.session.dispatch({ kind: "check_requested" }),
    writesAllowed: () => mounted.session.view.peek().canWrite,
    confirmed: () => mounted.session.view.peek().session,
    writeReview: mounted.session.writeReview(mounted.session.view.peek().epoch),
  });
}

async function beginRequest(
  stubs: Transport,
  host: ReturnType<typeof registerBusinessHost>["host"],
  source: Source,
  beforePatch: () => void = () => {},
) {
  const signal = new AbortController().signal;
  if (source === "业务 GET") {
    const request = fetchAdminBusinessWorlds(host, signal);
    await settleAuth();
    expect(stubs.businessRead.pendingCount()).toBe(1);
    return {
      async finish(reply: FetchReply) {
        stubs.businessRead.finish(reply);
        const result = await request;
        if (result.ok) throw new Error("业务 GET 应拒绝");
        return result.error;
      },
    };
  }

  const prepared = prepareAdminBusinessWorldSetEnabledOnce("main", false, 2)(COMMAND_ID);
  if (!prepared.ok) throw new Error("世界操作命令应可冻结");
  const beforeGet = stubs.get.calls;
  const request = setAdminBusinessWorldEnabled(host, prepared.value, signal);
  await settleAuth();
  expect(stubs.get.calls).toBe(beforeGet + 1);
  if (source === "业务 PATCH") {
    beforePatch();
    stubs.get.finish(sessionBody());
    await settleAuth();
    expect(stubs.businessWrites).toEqual([
      {
        method: "PATCH",
        path: "/api/v1/admin/worlds/main",
        body: '{"command_id":"00000000-0000-4000-8000-000000000011","enabled":false,"expected_policy_revision":2}',
      },
    ]);
  }
  return {
    async finish(reply: FetchReply) {
      if (source === "写前 GET") stubs.get.finish(reply);
      else stubs.businessWrite.finish(reply);
      const result = await request;
      if (result.ok) throw new Error("业务写入应拒绝");
      return adminWriteFailureDisplayCode(result.error);
    },
  };
}

async function withBusiness(
  check: (
    stubs: Transport,
    mounted: MountedSession,
    host: ReturnType<typeof registerBusinessHost>["host"],
    notices: string[],
  ) => Promise<void>,
): Promise<void> {
  const scope = cleanupScope();
  try {
    const broadcast = installAuthBroadcast();
    scope.defer(broadcast.restore);
    const stubs = transport();
    const mounted = await authenticated(stubs);
    scope.defer(mounted.dispose);
    const registration = registerBusinessHost(mounted);
    scope.defer(registration.release);
    await check(stubs, mounted, registration.host, broadcast.sent);
  } finally {
    scope.dispose();
  }
}

for (const source of SOURCES) {
  for (const scenario of ["429 自动复核", "429 后显式重试", "退出未知后留在后台"] as const) {
    for (const oldFirst of [false, true]) {
      test(`${source} 的 401 在${scenario}旧 200 ${oldFirst ? "之后" : "之前"}时换新票`, async () => {
        await withBusiness(async (stubs, mounted, host, notices) => {
          const pending = await beginRequest(stubs, host, source);
          // 写前 GET 仍挂起时，先用业务端点独立的队列推进退出；
          // 外壳读票与写前读票共用路径，以下用索引控制谁先交付。
          mounted.session.dispatch({ kind: "logout_clicked" });
          expect(stubs.logout.calls).toBe(1);
          stubs.logout.finish(
            scenario === "退出未知后留在后台" ? { kind: "network" } : failure(429, "rate_limited"),
          );
          await settleAuth();
          const shellReadIndex = source === "写前 GET" ? 1 : 0;
          if (scenario === "退出未知后留在后台") {
            stubs.get.finish(sessionBody(), shellReadIndex);
            await settleAuth();
            mounted.session.dispatch({ kind: "stay_after_logout" });
          } else if (scenario === "429 后显式重试") {
            mounted.session.dispatch({ kind: "check_requested" });
          }
          await settleAuth();
          expect(stubs.get.pendingCount()).toBe(shellReadIndex + 1);
          expect(mounted.session.view.peek().canWrite).toBe(false);
          if (oldFirst) {
            stubs.get.finish(sessionBody(), shellReadIndex);
            await settleAuth();
            expect(mounted.session.view.peek().canWrite).toBe(true);
          }
          const before = stubs.get.calls;
          const cancelled = stubs.get.cancelled;
          expect(await pending.finish(unauthorized())).toBe("unauthenticated");
          await settleAuth();
          expect(mounted.session.view.peek().canWrite).toBe(false);
          expect(mounted.session.view.peek().keyVisibility.hidden).toBe(true);
          expect(stubs.get.calls).toBe(before + 1);
          expect(stubs.get.cancelled - cancelled).toBe(oldFirst ? 0 : 1);
          expect(stubs.get.pendingCount()).toBe(1);
          expect(notices).toEqual([]);
          stubs.get.finish(unauthorized());
          await settleAuth();
          expect(mounted.session.view.peek().loggedOut).toBe(true);
          expect(notices).toEqual(["session-ended"]);
          expect(stubs.logout.calls).toBe(1);
          expect(stubs.businessWrite.calls).toBe(source === "业务 PATCH" ? 1 : 0);
        });
      });
    }
  }

  test(`${source} 可信 401 后只有新同代 200 恢复，保留“留在后台”的人工选择`, async () => {
    await withBusiness(async (stubs, mounted, host, notices) => {
      const pending = await beginRequest(stubs, host, source);
      const epoch = mounted.session.view.peek().epoch;
      mounted.session.dispatch({ kind: "logout_clicked" });
      stubs.logout.finish({ kind: "network" });
      await settleAuth();
      stubs.get.finish(sessionBody(), source === "写前 GET" ? 1 : 0);
      await settleAuth();
      mounted.session.dispatch({ kind: "stay_after_logout" });
      const before = stubs.get.calls;
      await pending.finish(unauthorized());
      expect(stubs.get.calls).toBe(before + 1);
      expect(mounted.session.view.peek().canWrite).toBe(false);
      expect(mounted.session.view.peek().logoutUnconfirmed).toBe(true);
      stubs.get.finish(sessionBody());
      await settleAuth();
      expect(mounted.session.view.peek().canWrite).toBe(true);
      expect(mounted.session.view.peek().epoch).toBe(epoch);
      expect(mounted.session.view.peek().logoutUnconfirmed).toBe(false);
      expect(mounted.session.view.peek().keyVisibility.hidden).toBe(false);
      expect(notices).toEqual([]);
      expect(stubs.logout.calls).toBe(1);
    });
  });

  test(`${source} 的坏信封、非 401、异代与不可读实发证据均不作废当前 GET`, async () => {
    for (const [cookie, reply] of [
      [SESSION_A, response(401, '{"code":"unauthenticated","message":"失败","data":null}')],
      [SESSION_A, failure(401, "origin_forbidden")],
      [SESSION_A, failure(503, "unauthenticated")],
      [SESSION_B, unauthorized()],
      ["", unauthorized()],
      [null, unauthorized()],
    ] as const) {
      await withBusiness(async (stubs, mounted, host, notices) => {
        const setCookie = () => {
          if (cookie === null) mounted.document.cookieReads.push(null);
          else mounted.document.cookie = cookie;
        };
        if (source !== "业务 PATCH") setCookie();
        const pending = await beginRequest(stubs, host, source, setCookie);
        mounted.document.cookie = SESSION_A;
        mounted.session.dispatch({ kind: "check_requested" });
        const before = stubs.get.calls;
        const cancelled = stubs.get.cancelled;
        await pending.finish(reply);
        await settleAuth();
        expect(stubs.get.calls).toBe(before);
        expect(stubs.get.cancelled).toBe(cancelled);
        expect(mounted.session.view.peek().keyVisibility.hidden).toBe(false);
        stubs.get.finish(sessionBody());
        await settleAuth();
        expect(mounted.session.view.peek().canWrite).toBe(true);
        expect(notices).toEqual([]);
      });
    }
  });

  test(`${source} 迟到的原宿主 401 不影响 B 或重新挂载的 A`, async () => {
    for (const sameIdentity of [false, true]) {
      await withBusiness(async (stubs, mounted, host, notices) => {
        const pending = await beginRequest(stubs, host, source);
        const epoch = mounted.session.view.peek().epoch;
        mounted.session.dispatch({ kind: "broadcast" });
        await settleAuth();
        expect(mounted.session.view.peek().session).toBeNull();
        const replaced = sameIdentity ? sessionBody() : sessionBody("cookie-b");
        mounted.document.cookie = sameIdentity ? SESSION_A : SESSION_B;
        if (!sameIdentity) mounted.session.dispatch({ kind: "visible" });
        stubs.get.finish(replaced, source === "写前 GET" ? 1 : 0);
        await settleAuth();
        expect(mounted.session.view.peek().canWrite).toBe(true);
        expect(mounted.session.view.peek().epoch).toBeGreaterThan(epoch);
        const before = stubs.get.calls;
        await pending.finish(unauthorized());
        await settleAuth();
        expect(stubs.get.calls).toBe(before);
        expect(mounted.session.view.peek().canWrite).toBe(true);
        expect(mounted.session.view.peek().session?.csrfToken).toBe(
          sameIdentity ? "fixture-cookie" : "cookie-b",
        );
        expect(notices).toEqual([]);
      });
    }
  });
}

test("连续两个外部 401 分别替换在途票，最后一张票才能结束会话", async () => {
  await withBusiness(async (stubs, mounted, host, notices) => {
    const first = await beginRequest(stubs, host, "业务 GET");
    const second = await beginRequest(stubs, host, "业务 PATCH");
    mounted.session.dispatch({ kind: "check_requested" });
    const before = stubs.get.calls;
    await first.finish(unauthorized());
    expect(stubs.get.calls).toBe(before + 1);
    await second.finish(unauthorized());
    expect(stubs.get.calls).toBe(before + 2);
    expect(stubs.get.cancelled).toBe(2);
    expect(mounted.session.view.peek().canWrite).toBe(false);
    expect(notices).toEqual([]);
    stubs.get.finish(unauthorized());
    await settleAuth();
    expect(mounted.session.view.peek().loggedOut).toBe(true);
    expect(notices).toEqual(["session-ended"]);
  });
});

test("写前 GET 认出 B 后暂停写入并遮住 Key，缺失 401 或不可读 503 都不误播", async () => {
  for (const after of ["", null] as const) {
    await withBusiness(async (stubs, mounted, host, notices) => {
      const pending = await beginRequest(stubs, host, "写前 GET");
      if (after === null) mounted.document.cookieReads.push(null, null);
      else mounted.document.cookie = after;
      expect(await pending.finish(sessionBody("cookie-b"))).toBe("unauthenticated");
      await settleAuth();
      expect(mounted.session.view.peek().session?.csrfToken).toBe("fixture-cookie");
      expect(mounted.session.view.peek().keyVisibility.hidden).toBe(true);
      expect(mounted.session.view.peek().canWrite).toBe(false);
      expect(stubs.businessWrite.calls).toBe(0);
      expect(stubs.get.calls).toBe(3);
      if (after === null) mounted.document.cookieReads.push(null);
      stubs.get.finish(after === null ? failure(503, "storage_unavailable") : unauthorized());
      await settleAuth();
      expect(mounted.session.view.peek().loggedOut).toBe(after !== null);
      expect(mounted.session.view.peek().canWrite).toBe(false);
      expect(notices).toEqual([]);
    });
  }
});

const ARRIVAL_ORDERS = [
  ["external", "review", "renewal"],
  ["external", "renewal", "review"],
  ["review", "external", "renewal"],
  ["review", "renewal", "external"],
  ["renewal", "external", "review"],
  ["renewal", "review", "external"],
] as const;

for (const source of SOURCES) {
  for (const stay of [false, true]) {
    test(`${source} 与${stay ? "留在后台" : "显式复核"}及活动 POST 的六种到达顺序`, async () => {
      for (const renewed of [true, false]) {
        for (const order of ARRIVAL_ORDERS) {
          for (const restored of renewed ? [true, false] : [false]) {
            await withBusiness(async (stubs, mounted, host, notices) => {
              mounted.session.dispatch({ kind: "input", onLogout: false });
              await settleAuth();
              expect(stubs.activity.calls).toBe(1);
              const request = await beginRequest(stubs, host, source);
              const oldIndex = source === "写前 GET" ? 1 : 0;
              if (stay) {
                mounted.session.dispatch({ kind: "logout_clicked" });
                stubs.logout.finish({ kind: "network" });
                await settleAuth();
                stubs.get.finish(sessionBody(), oldIndex);
                await settleAuth();
                mounted.session.dispatch({ kind: "stay_after_logout" });
              } else {
                mounted.session.dispatch({ kind: "check_requested" });
              }
              let oldPending = true;
              let invalidated = false;
              let externalPending = true;
              for (const arrival of order) {
                switch (arrival) {
                  case "external": {
                    const reads = stubs.get.calls;
                    expect(await request.finish(unauthorized())).toBe("unauthenticated");
                    expect(stubs.get.calls).toBe(reads + (invalidated ? 0 : 1));
                    externalPending = false;
                    invalidated = true;
                    oldPending = false;
                    break;
                  }
                  case "review":
                    // 被取消的旧 GET 不能再进适配器；纯状态机另重放其迟到 200。
                    if (oldPending) {
                      stubs.get.finish(sessionBody(), externalPending ? oldIndex : 0);
                      oldPending = false;
                    }
                    break;
                  case "renewal":
                    stubs.activity.finish(renewed ? sessionBody() : unauthorized());
                    if (!renewed) {
                      invalidated = true;
                      oldPending = false;
                    }
                    break;
                }
                await settleAuth();
                if (invalidated) expect(mounted.session.view.peek().canWrite).toBe(false);
                expect(stubs.activity.calls).toBe(1);
                expect(stubs.logout.calls).toBe(stay ? 1 : 0);
              }
              if (stubs.get.pendingCount() > 0) {
                expect(stubs.get.pendingCount()).toBe(1);
                stubs.get.finish(restored ? sessionBody() : unauthorized());
                await settleAuth();
              }
              expect(mounted.session.view.peek().canWrite).toBe(restored);
              expect(mounted.session.view.peek().loggedOut).toBe(!restored);
              expect(notices).toEqual(restored ? [] : ["session-ended"]);
            });
          }
        }
      }
    });
  }
}

test("已发 DELETE 在清 Cookie 后断线，坏 401 和 503 都不结账，后续可信 401 只通知一次", async () => {
  await withBusiness(async (stubs, mounted, _host, notices) => {
    mounted.session.dispatch({ kind: "logout_clicked" });
    expect(stubs.logout.calls).toBe(1);
    mounted.document.cookie = "";
    stubs.logout.finish({ kind: "network" });
    await settleAuth();
    expect(notices).toEqual([]);
    stubs.get.finish(response(401, '{"code":"unauthenticated","message":"不可信","data":null}'));
    await settleAuth();
    expect(notices).toEqual([]);
    expect(mounted.session.view.peek().logoutUnconfirmed).toBe(true);
    mounted.session.dispatch({ kind: "check_requested" });
    stubs.get.finish(failure(503, "storage_unavailable"));
    await settleAuth();
    expect(notices).toEqual([]);
    mounted.session.dispatch({ kind: "check_requested" });
    stubs.get.finish(unauthorized());
    await settleAuth();
    expect(notices).toEqual(["session-ended"]);
    expect(mounted.session.view.peek().loggedOut).toBe(true);
    mounted.document.setHidden(false);
    stubs.get.finish(unauthorized());
    await settleAuth();
    expect(notices).toEqual(["session-ended"]);
    expect(stubs.logout.calls).toBe(1);
  });
});

for (const request of ["活动 POST", "退出 DELETE"] as const) {
  test(`${request} 实发前重新读取 Cookie，同代发出，换代或缺失只发 GET`, async () => {
    for (const sentCookie of [SESSION_A, SESSION_B, ""]) {
      await withBusiness(async (stubs, mounted, _host, notices) => {
        const epoch = mounted.session.view.peek().epoch;
        mounted.document.cookie = sentCookie;
        mounted.document.cookieReads.push(SESSION_A, sentCookie);
        mounted.session.dispatch(
          request === "活动 POST" ? { kind: "input", onLogout: false } : { kind: "logout_clicked" },
        );
        await settleAuth();
        const same = sentCookie === SESSION_A;
        expect(stubs.activity.calls).toBe(same && request === "活动 POST" ? 1 : 0);
        expect(stubs.logout.calls).toBe(same && request === "退出 DELETE" ? 1 : 0);
        expect(stubs.get.calls).toBe(same ? 1 : 2);
        if (!same) {
          const view = mounted.session.view.peek();
          expect(view.canWrite).toBe(false);
          expect(view.keyVisibility.hidden).toBe(true);
          expect(view.epoch).toBe(epoch);
          expect(view.session?.csrfToken).toBe("fixture-cookie");
          stubs.get.finish(unauthorized());
          await settleAuth();
          expect(mounted.session.view.peek().loggedOut).toBe(true);
          expect(notices).toEqual(sentCookie === "" ? ["session-ended"] : []);
        }
      });
    }
  });
}

test("同代 503 后，活动收包读到 B 时先遮 Key，再丢弃旧的成功或失败回执", async () => {
  for (const reply of [sessionBody(), failure(503, "storage_unavailable")]) {
    await withBusiness(async (stubs, mounted, _host, notices) => {
      mounted.session.dispatch({ kind: "input", onLogout: false });
      await settleAuth();
      expect(stubs.activity.calls).toBe(1);
      mounted.document.setHidden(false);
      stubs.get.finish(failure(503, "storage_unavailable"));
      await settleAuth();
      expect(mounted.session.view.peek().keyVisibility.hidden).toBe(false);

      mounted.document.cookie = SESSION_B;
      stubs.activity.finish(reply);
      await settleAuth();
      expect(mounted.session.view.peek().keyVisibility.hidden).toBe(true);
      expect(mounted.session.view.peek().session?.csrfToken).toBe("fixture-cookie");
      expect(stubs.get.calls).toBe(3);
      mounted.document.cookie = "";
      mounted.document.setHidden(false);
      stubs.get.finish(unauthorized());
      await settleAuth();
      expect(mounted.session.view.peek().loggedOut).toBe(true);
      expect(stubs.activity.calls).toBe(1);
      expect(notices).toEqual([]);
    });
  }
});
