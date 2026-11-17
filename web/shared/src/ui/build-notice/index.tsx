// Public 与 Admin 共用的服务端构建漂移检测与刷新提示。
// 只采信 `no-store` 响应上的 x-auction-build；与自身构建号不一致时提示刷新，
// 不自动重载正在操作的页面。

import type { JSX } from "@solidjs/web";
import { type Accessor, onCleanup, Show } from "solid-js";
import { createCell } from "../../reactive/cell";
import { buildIsStale, waitForBuildDrift } from "./drift";

export {
  observeServerBuildId,
  responseReportsCurrentBuild,
  SERVER_BUILD_HEADER,
} from "./drift";

/** 订阅构建号偏差；已偏差时立即为 true，之后保持到页面刷新。 */
export function useBuildDrift(): Accessor<boolean> {
  const stale = createCell(buildIsStale());
  if (stale.peek()) {
    return stale;
  }
  const wait = waitForBuildDrift();
  let active = true;
  // 等待者在组件初始化时已经登记，清理也必须同时建立；只放到 onSettled 会漏掉此前卸载。
  onCleanup(() => {
    active = false;
    wait.cancel();
  });
  void wait.promise.then(() => {
    if (active) {
      stale.set(true);
    }
  });
  return stale;
}

/** 部署后旧页面的刷新提示；文案由调用方提供，不使用浏览器自动刷新。 */
export function StaleBuildBanner(props: { message: string; refreshLabel: string }): JSX.Element {
  const stale = useBuildDrift();
  return (
    <Show when={stale()}>
      <div class="shared-stale-build" role="status">
        <p class="shared-stale-build__message">{props.message}</p>
        <button type="button" class="shared-stale-build__refresh" onClick={() => reloadPage()}>
          {props.refreshLabel}
        </button>
      </div>
    </Show>
  );
}

function reloadPage(): void {
  if (typeof window === "undefined") {
    return;
  }
  try {
    window.location.reload();
  } catch {
    // 浏览器拒绝重载时保持当前页面；下一次成功响应仍会再次提示。
  }
}
