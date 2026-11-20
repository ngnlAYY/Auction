// 结算页各项 GET 的重试控件与成功时的焦点交接。

import { focusElementById } from "@auction/web-shared/platform/a11y";
import { topModal } from "@auction/web-shared/platform/a11y/escape";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

/** 可单独重新发起的结算读取。 */
export type SettlementRead = "catalog" | "catalogLookup" | "settlement" | "recovery";

/** 一次失败读取的安全文案与原请求身份。 */
export interface SettlementReadError {
  readonly message: string;
  readonly scope: string;
}

const READ_TARGETS = {
  catalog: { retryId: "settlement-catalog-retry", heading: "settlement-selector-title" },
  catalogLookup: { retryId: "settlement-period-retry", heading: "settlement-selector-title" },
  settlement: { retryId: "settlement-report-retry", heading: "settlement-report-panel-title" },
  recovery: { retryId: "settlement-recovery-retry", heading: "settlement-recovery-title" },
} as const satisfies Record<SettlementRead, { retryId: string; heading: string }>;

/** 仍由该读取的重试按钮持有焦点时，在发布成功结果、拆除按钮之前还焦。 */
export function focusAfterSuccessfulRead(read: SettlementRead, scope: string): void {
  if (typeof document === "undefined") {
    return;
  }
  const { retryId, heading } = READ_TARGETS[read];
  const focus = document.activeElement;
  if (
    focus?.id === retryId &&
    focus.getAttribute("data-read-scope") === scope &&
    topModal(document) === null
  ) {
    focusElementById(heading);
  }
}

/** 请求期间保留可聚焦的按钮，阻止键盘与指针激活。 */
export function SettlementReadRetry(props: {
  readonly read: SettlementRead;
  readonly scope: string;
  readonly message: string;
  readonly pending: boolean;
  readonly onRetry: () => void;
}): JSX.Element {
  return (
    <div>
      <AsyncState kind="error" title={props.message} detail="" />
      <button
        id={READ_TARGETS[props.read].retryId}
        data-read-scope={props.scope}
        type="button"
        class="ui-async-state__retry"
        aria-describedby={READ_TARGETS[props.read].heading}
        aria-disabled={props.pending ? "true" : undefined}
        onClick={() => {
          if (!props.pending) {
            props.onRetry();
          }
        }}
      >
        {t("admin-retry")}
      </button>
    </div>
  );
}
