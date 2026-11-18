// 读取页对非法场次链接的提示与显式恢复入口。

import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

/** 非法场次的提示与场次选择器共用此引用。 */
export const INVALID_READ_AUCTION_ERROR_ID = "public-read-auction-error";

/** 要求玩家明确处理非法场次，避免页面自行改变读取目标。 */
export function InvalidReadAuction(props: {
  readonly onRecover: () => void;
  readonly recoverLabel?: string;
}): JSX.Element {
  return (
    <div class="public-auction-empty">
      <p id={INVALID_READ_AUCTION_ERROR_ID} class="public-auction-empty__title" role="alert">
        {t("auction-invalid-link")}
      </p>
      <button
        class="public-auction-empty__retry"
        type="button"
        aria-describedby={INVALID_READ_AUCTION_ERROR_ID}
        onClick={props.onRecover}
      >
        {props.recoverLabel ?? t("auction-view-current")}
      </button>
    </div>
  );
}
