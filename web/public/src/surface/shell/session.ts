//! 外壳会话资源与路由焦点标识的构造辅助；会话撤销判断保持与后端截止语义一致。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { RouteMatch } from "@auction/web-shared/router";
import { createEffect } from "solid-js";

import { estimatedNow } from "../../api/clock";
import { fetchPublicSession, type PublicSession, parseStrictRfc3339 } from "../../api/session";
import type { PublicSessionResource, PublicSessionResult } from "../index";

export function routeFocusToken(route: RouteMatch): string {
  if (route.kind === "not-found") {
    return `NotFound { segments: [${route.segments.map(rustDebugString).join(", ")}] }`;
  }
  // Dioxus 的 Route 调试串只包含枚举字段；普通路由的查询变化不应抢占主内容焦点。
  switch (route.name) {
    case "LotDetail":
      return `LotDetail { lot_code: ${rustDebugString(route.params.lot_code ?? "")} }`;
    case "Login": {
      const returnTo = route.query.return_to;
      return `Login { return_to: ${returnTo === undefined ? "None" : `Some(${rustDebugString(returnTo)})`} }`;
    }
    default:
      return route.name;
  }
}

/** Rust `Debug` 的字符串引号与转义足够覆盖路由字段；令牌只用于检测路由身份变化。 */
function rustDebugString(value: string): string {
  return JSON.stringify(value);
}

export function createPublicSessionResource(): PublicSessionResource {
  const resource = createTaskResource(
    () => null,
    async (_dependencies, context) => fetchPublicSession(context.signal),
  );
  const override = createCell<PublicSessionResult | undefined>(undefined);
  const session = (() => override() ?? resource()) as PublicSessionResource;
  session.peek = () => override.peek() ?? resource.peek();
  // Resource::set 只写值，不改变任务阶段；覆盖值不能把 pending 伪装成 ready。
  session.state = () => resource.state();
  session.restart = () => {
    resource.restart();
  };
  session.cancel = () => resource.cancel();
  session.clear = () => {
    override.set(undefined);
    resource.clear();
  };
  session.set = (value) => {
    if (value === undefined) {
      override.set(undefined);
      resource.clear();
    } else {
      override.set(value);
    }
  };
  // Resource::set 写入的值会被下一次资源结果覆盖；覆盖值也必须在该结果到达时让位。
  let previousResourceValue = resource.peek();
  createEffect(
    () => resource(),
    (value) => {
      if (value !== previousResourceValue) {
        previousResourceValue = value;
        if (override.peek() !== undefined) {
          override.set(undefined);
        }
      }
    },
  );
  return session;
}

/** 未到期却失效的认证会话只来自服务端撤销；自然到期不显示“已在其他设备登录”。 */
export function sessionEndedByRevocation(identity: PublicSession): boolean {
  if (!identity.authenticated) {
    return false;
  }
  // 永久会话没有截止字段，不可能自然到期。
  if (identity.expires_at === null) {
    return true;
  }
  // 截止字符串不可解析时保守地不提示，避免把未知状态当作撤销。
  const expiry = parseStrictRfc3339(identity.expires_at);
  return expiry !== null && expiry > BigInt(estimatedNow()) * 1_000_000n;
}

/** 旧版只把显式 Isekai 视为第二世界；缺失、Main 与非法 query 都在内存回退 Main。 */
export function selectedWorldFromQuery(value: string | null): string {
  return value === "isekai" ? "isekai" : "main";
}
