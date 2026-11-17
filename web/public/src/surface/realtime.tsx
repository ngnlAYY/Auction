//! Public 静态服务说明页。
//! 旧版 `/status` 只说明 Public surface 的用途；健康检查和实时事件仍由机器 API 与页头连接
//! 状态承担，不在这里重复暴露诊断表或建立第二条订阅。

import { unreachable } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

import type { PublicConnectionState } from "./board/realtime";

/** 顶栏徽章从页头七态取文案。 */
export function publicConnectionMessage(state: PublicConnectionState): string {
  switch (state) {
    case "connecting":
      return t("realtime-connection-connecting");
    case "degraded":
      return t("realtime-connection-degraded");
    case "live":
      return t("realtime-connection-live");
    case "manual":
      return t("realtime-connection-manual");
    case "preview":
      return t("realtime-connection-preview");
    case "idle":
      return t("realtime-connection-idle");
    case "unavailable":
      return t("realtime-connection-unavailable");
    default:
      return unreachable(state);
  }
}

/** 徽章的稳定状态名；同时作为 `data-state` 供样式与诊断使用。 */
export function publicConnectionStateName(state: PublicConnectionState): string {
  switch (state) {
    case "connecting":
    case "degraded":
    case "live":
    case "manual":
    case "preview":
    case "idle":
    case "unavailable":
      return state;
    default:
      return unreachable(state);
  }
}

/** 徽章状态点的修饰 class；中性状态沿用基础灰点，不新增无样式类。 */
export function publicConnectionClass(state: PublicConnectionState): string {
  switch (state) {
    case "live":
      return "public-connection-badge--live";
    case "degraded":
      return "public-connection-badge--degraded";
    case "unavailable":
      return "public-connection-badge--unavailable";
    case "connecting":
    case "manual":
    case "preview":
    case "idle":
      return "";
    default:
      return unreachable(state);
  }
}

export function page(): JSX.Element {
  return (
    <section class="public-status-page" aria-label={t("page-status-title")}>
      <p>{t("page-status-note")}</p>
    </section>
  );
}
