//! HV 服务端唯一声明的严格解析；不执行脚本，不从字符串、注释或请求参数获取 UID。

use super::super::*;

impl MoogleMailParser {
    /// 仅 Character Equipment 已观察结构可证明当前 Cookie 的认证 UID。
    pub fn parse_authenticated_uid(
        &self,
        bytes: &[u8],
        world: MoogleMailWorld,
    ) -> Result<String, MoogleMailError> {
        let operation = MoogleMailOperation::Identity;
        let document = trusted_document(bytes, world, operation)?;
        unique(&document, "#mainpane", world, operation)?;
        let marker = unique(&document, "#eqch_outer", world, operation)?;
        if marker.parent().and_then(ElementRef::wrap).and_then(|node| node.attr("id"))
            != Some("mainpane")
        {
            return Err(protocol(operation, world));
        }
        verify_main_url(&document, world, operation)?;
        let script = script_text(&document, world, operation)?;
        let declaration = declaration_value(&script, "uid", &["let"])
            .filter(|value| auction_application::valid_external_positive_id(value))
            .ok_or_else(|| protocol(operation, world))?;
        let mut server_script = String::new();
        for node in document.select(&selector("#mainpane > script:not([src])", operation, world)?) {
            server_script.push_str(&node.text().collect::<String>());
            server_script.push('\n');
        }
        if declaration_value(&server_script, "uid", &["let"]) != Some(declaration) {
            return Err(protocol(operation, world));
        }
        Ok(declaration.to_owned())
    }

    /// 现金和 compose 也必须在各自请求世界中观察到一致 MAIN_URL。
    pub(crate) fn verify_evidence_world(
        &self,
        bytes: &[u8],
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
    ) -> Result<(), MoogleMailError> {
        let document = trusted_document(bytes, world, operation)?;
        verify_main_url(&document, world, operation)
    }
}

pub(super) fn trusted_document(
    bytes: &[u8],
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
) -> Result<Html, MoogleMailError> {
    let source = str::from_utf8(bytes).map_err(|_| protocol(operation, world))?;
    if source.trim().is_empty() {
        return Err(protocol(operation, world));
    }
    Ok(Html::parse_document(source))
}

pub(super) fn unique<'a>(
    document: &'a Html,
    query: &str,
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
) -> Result<ElementRef<'a>, MoogleMailError> {
    let selector = selector(query, operation, world)?;
    let mut nodes = document.select(&selector);
    let node = nodes.next().ok_or_else(|| protocol(operation, world))?;
    if nodes.next().is_some() {
        return Err(protocol(operation, world));
    }
    Ok(node)
}

pub(super) fn verify_main_url(
    document: &Html,
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
) -> Result<(), MoogleMailError> {
    let script = script_text(document, world, operation)?;
    let value = declaration_value(&script, "MAIN_URL", &["let", "const", "var"])
        .ok_or_else(|| protocol(operation, world))?;
    let literal = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|value| value.strip_suffix('\'')))
        .ok_or_else(|| protocol(operation, world))?;
    let expected = format!("https://hentaiverse.org{}", world.path());
    if literal != expected {
        return Err(protocol(operation, world));
    }
    Ok(())
}

/// 只接受顶层独立声明的简单右值；重复/重赋值/属性赋值、表达式和作用域嵌套失败关闭。
/// 遮罩与原文保持字节偏移一致，字符串中的伪声明不会参与匹配。
fn declaration_value<'a>(source: &'a str, name: &str, keywords: &[&str]) -> Option<&'a str> {
    let masked = executable_javascript(source)?;
    let identifier = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$';
    let mut found = None;
    for (start, _) in masked.match_indices(name) {
        let end = start + name.len();
        if start > 0 && identifier(masked.as_bytes()[start - 1])
            || masked.as_bytes().get(end).is_some_and(|byte| identifier(*byte))
        {
            continue;
        }
        let preceding = masked[..start].trim_end();
        if preceding.ends_with("++") || preceding.ends_with("--") {
            return None;
        }
        let after = masked[end..].trim_start();
        // 引用本身不是声明；任何可见的复合改写都拒绝。
        if after.starts_with("++")
            || after.starts_with("--")
            || [
                "+=", "-=", "*=", "/=", "%=", "**=", "<<=", ">>=", ">>>=", "&=", "^=", "|=", "&&=",
                "||=", "??=",
            ]
            .iter()
            .any(|operator| after.starts_with(operator))
        {
            return None;
        }
        if !after.starts_with('=') || after.starts_with("==") {
            continue;
        }
        if found.is_some() {
            return None;
        }
        let before = masked[..start].trim_end();
        let keyword_start = before
            .rfind(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .map_or(0, |index| index + 1);
        if !keywords.contains(&&before[keyword_start..]) {
            return None;
        }
        let prefix = before[..keyword_start].trim_end();
        if !prefix.is_empty() && !prefix.ends_with(';') {
            return None;
        }
        // 已观察声明位于顶层，拒绝条件/函数体中的同名局部量。
        let depth = prefix.bytes().try_fold(0_i64, |depth, byte| match byte {
            b'{' => depth.checked_add(1),
            b'}' => depth.checked_sub(1).filter(|value| *value >= 0),
            _ => Some(depth),
        })?;
        if depth != 0 {
            return None;
        }
        let equal = end + (masked[end..].len() - after.len());
        let terminator = masked[equal + 1..].find(';')? + equal + 1;
        found = Some(source[equal + 1..terminator].trim());
    }
    found
}
