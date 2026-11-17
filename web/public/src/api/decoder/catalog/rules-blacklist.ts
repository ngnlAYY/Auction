// 规则文档与公开黑名单编号页的字段闭集校验。

import { jsonKeysOnly } from "@auction/web-shared/net/wire";

import { accountNonempty, object, positive, string, time } from "..";
import { validNumberedPage } from "./auction";

/** 校验已发布规则文档的字段闭集与出处。 */
export function validRules(value: unknown): boolean {
  const rules = object(value);
  if (rules === null) return false;
  return (
    accountNonempty(rules.locale) &&
    accountNonempty(rules.title) &&
    string(rules.markdown) !== null &&
    positive(rules.revision) &&
    time(rules.published_at)
  );
}

/** 校验公开黑名单编号页与逐项字段闭集。 */
export function validBlacklistPage(value: unknown): boolean {
  const page = object(value);
  if (page === null) return false;
  const users = page.users;
  return (
    jsonKeysOnly(page, ["users", "total", "page", "page_size"], []) &&
    validNumberedPage(page) &&
    Array.isArray(users) &&
    users.every((entry: unknown) => {
      const user = object(entry);
      return (
        user !== null &&
        jsonKeysOnly(user, ["username", "reason", "blacklisted_at"], []) &&
        accountNonempty(user.username) &&
        string(user.reason) !== null &&
        time(user.blacklisted_at)
      );
    })
  );
}
