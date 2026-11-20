// 凭据导入的敏感输入随表单作用域释放。

import { createCell } from "@auction/web-shared/reactive/cell";
import { onCleanup } from "solid-js";

/** 管理两种导入来源的敏感字段，离开表单作用域时释放草稿。 */
export function createCredentialSecretDraft() {
  const secret = createCell("");
  const username = createCell("");
  const password = createCell("");
  const clear = () => {
    secret.set("");
    username.set("");
    password.set("");
  };
  let alive = true;
  onCleanup(() => {
    alive = false;
    // 离页后宿主可能仍持有回执回调，只有冻结命令继续持有已提交的秘密。
    clear();
  });
  return { secret, username, password, clear, isAlive: () => alive };
}
