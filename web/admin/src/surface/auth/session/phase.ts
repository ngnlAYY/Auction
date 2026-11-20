// 会话阶段上的认证实例存取；只读写阶段本身，不触碰状态机其他字段。

import { unreachable } from "@auction/web-shared/result";

import type { Draft } from "./draft";
import type { Phase, Verified } from "./index";

/** Authenticated 被清空后回到 Starting；其余宿主状态只移除当前认证实例。 */
export function takeVerified(draft: Draft): Verified | null {
  const phase = draft.phase;
  switch (phase.kind) {
    case "authenticated":
      draft.phase = { kind: "starting" };
      return phase.verified;
    case "checking":
    case "unavailable":
    case "logging_out":
      draft.phase = withVerified(phase, null);
      return phase.verified;
    case "checking_login":
    case "starting":
    case "logged_out":
      return null;
    default:
      return unreachable(phase);
  }
}

/** 对应源码的 `verified_mut`：退出阶段不向续期结果暴露宿主。 */
export function verifiedMut(phase: Phase): Verified | null {
  switch (phase.kind) {
    case "authenticated":
      return phase.verified;
    case "checking":
    case "unavailable":
      return phase.verified;
    case "checking_login":
    case "starting":
    case "logging_out":
    case "logged_out":
      return null;
    default:
      return unreachable(phase);
  }
}

/** 用新的认证实例替换当前阶段，其他阶段保持原值。 */
export function withVerified(phase: Phase, verified: Verified | null): Phase {
  switch (phase.kind) {
    case "authenticated":
      return verified === null ? { kind: "starting" } : { kind: "authenticated", verified };
    case "checking":
      return { kind: "checking", verified };
    case "unavailable":
      return { kind: "unavailable", verified, error: phase.error };
    case "logging_out":
      return { kind: "logging_out", verified, logout: phase.logout };
    case "checking_login":
    case "starting":
    case "logged_out":
      return phase;
    default:
      return unreachable(phase);
  }
}
