// 登录页注册名单的加载、失败与成功状态。

import { expect, spyOn, test } from "bun:test";
import { createRoot, flush } from "solid-js";

import { useRegistrationAccounts } from "./login";

function registrationInfo(players: readonly string[]): Response {
  return Response.json({ code: "ok", message: "ok", data: { bot_players: players } });
}

function mountRegistration(initialResponse: Response) {
  const fetcher = spyOn(globalThis, "fetch").mockRejectedValue(new Error("缺少测试响应"));
  fetcher.mockResolvedValueOnce(initialResponse);
  const root = createRoot((dispose) => {
    const registration = useRegistrationAccounts();
    flush();
    return { ...registration, dispose };
  });
  return {
    ...root,
    fetcher,
    dispose() {
      root.dispose();
      fetcher.mockRestore();
    },
  };
}

async function settleRead(): Promise<void> {
  await Bun.sleep(0);
  flush();
}

test("注册名单失败后单独重试，等待期间显示加载并按接口顺序展示成功名单", async () => {
  const mounted = mountRegistration(
    Response.json({ code: "service_unavailable", message: "Unavailable" }, { status: 503 }),
  );
  try {
    expect(mounted.view()).toEqual({ kind: "loading" });
    await settleRead();
    expect(mounted.view()).toEqual({ kind: "error" });
    expect(mounted.fetcher).toHaveBeenCalledTimes(1);

    mounted.fetcher.mockResolvedValueOnce(registrationInfo(["First", "Second"]));
    mounted.retry();
    flush();
    expect(mounted.view()).toEqual({ kind: "loading" });
    await settleRead();
    expect(mounted.view()).toEqual({ kind: "ready", accounts: "First, Second" });
    expect(mounted.fetcher).toHaveBeenCalledTimes(2);
    for (const [url, request] of mounted.fetcher.mock.calls) {
      expect(url).toBe("/api/v1/registration-info");
      expect(request?.method).toBe("GET");
    }
  } finally {
    mounted.dispose();
  }
});

test("成功空名单与加载及失败状态区分", async () => {
  const mounted = mountRegistration(registrationInfo([]));
  try {
    expect(mounted.view()).toEqual({ kind: "loading" });
    await settleRead();
    expect(mounted.view()).toEqual({ kind: "ready", accounts: "" });
    expect(mounted.fetcher).toHaveBeenCalledTimes(1);
  } finally {
    mounted.dispose();
  }
});
