//! 论坛登录态的只读正向证明；公开页面 HTTP 200 本身不是认证成功。

use super::*;

impl EhentaiClient {
    /// 只读取用户控制台并核对系统导航，不加载发帖表单、不执行 POST。
    pub(crate) async fn check_session(&self, cookie: &EhentaiCookie) -> Result<(), EhentaiError> {
        let operation = EhentaiOperation::CheckSession;
        if !self.valid_for() || !valid_cookie(cookie) {
            return Err(EhentaiError::credential(operation));
        }
        let target = self.index_url(&[("act", "UserCP"), ("CODE", "00")], operation)?;
        let (body, _) = self.get_page(operation, target.clone(), None, cookie).await?;
        let source =
            std::str::from_utf8(&body).map_err(|_| EhentaiError::protocol(operation, false))?;
        let document = Html::parse_document(source);
        let navigation =
            Selector::parse("#userlinks").map_err(|_| EhentaiError::protocol(operation, false))?;
        let links =
            Selector::parse("a[href]").map_err(|_| EhentaiError::protocol(operation, false))?;
        let navigation = document.select(&navigation).collect::<Vec<_>>();
        if navigation.len() != 1 {
            return Err(EhentaiError::protocol(operation, false));
        }
        let mut profile = false;
        let mut logout = false;
        for link in navigation[0].select(&links) {
            let Some(reference) = link.attr("href") else { continue };
            let Ok(url) = self.resolve_same_origin(&target, reference, operation) else { continue };
            if url.path() != "/index.php" {
                continue;
            }
            let pairs = url.query_pairs().collect::<Vec<_>>();
            profile |= pairs.len() == 1
                && pairs[0].0 == "showuser"
                && pairs[0].1.bytes().all(|byte| byte.is_ascii_digit())
                && pairs[0].1.parse::<u64>().is_ok_and(|value| value > 0);
            logout |= pairs.iter().filter(|(key, _)| key == "act").count() == 1
                && pairs.iter().any(|(key, value)| key == "act" && value == "Login")
                && pairs.iter().filter(|(key, _)| key == "CODE").count() == 1
                && pairs.iter().any(|(key, value)| key == "CODE" && value == "03");
        }
        if profile && logout { Ok(()) } else { Err(EhentaiError::protocol(operation, false)) }
    }
}
