// 管理端页面分块失败的恢复提示与重载入口。

import { reloadAfterPageLoadError } from "@auction/web-shared/ui/bootstrap/page-load-recovery";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { onSettled } from "solid-js";

/** 由当前认证外壳决定能否自动重载；失败提示及手动重载入口始终留在页面区。 */
export function AdminPageLoadFailure(props: {
  readonly isCurrent: () => boolean;
  readonly canAutoReload: () => boolean;
  readonly confirmManualReload: () => boolean;
}): JSX.Element {
  onSettled(() => {
    if (props.isCurrent() && props.canAutoReload()) {
      reloadAfterPageLoadError();
    }
  });

  return (
    <div class="admin-shell__banner">
      <p role="alert">{t("admin-error-generic")}</p>
      <button
        type="button"
        onClick={() => {
          if (props.isCurrent()) {
            reloadPageManually(props.confirmManualReload());
          }
        }}
      >
        {t("stale-build-refresh")}
      </button>
    </div>
  );
}

function reloadPageManually(needsConfirmation: boolean): void {
  if (typeof window === "undefined") {
    return;
  }
  // 草稿仍由原生离开确认保护，其他会话状态由此处确认。手动重载不写防循环标记。
  try {
    if (needsConfirmation && !window.confirm(t("admin-reload-confirm"))) {
      return;
    }
    window.location.reload();
  } catch {
    // 浏览器拒绝重载时，保留页面区提示供再次操作。
  }
}
