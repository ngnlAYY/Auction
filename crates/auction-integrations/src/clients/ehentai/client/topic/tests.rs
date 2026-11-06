//! 确认页回执解析的固定 HTML 样本；只验证本地结构判定，不访问远端。

use self::parent::*;
use super as parent;

/// 按固定生产论坛配置解析编辑回执，返回两份证明一致时的帖子身份。
fn acknowledged_post(html: &[u8]) -> Option<ForumPublishedPost> {
    let config = EhentaiClientConfig::production()
        .unwrap_or_else(|error| panic!("固定论坛配置应合法: {error:?}"));
    let client = EhentaiClient::with_config(config)
        .unwrap_or_else(|error| panic!("安全论坛客户端应可构造: {error:?}"));
    let base = Url::parse("https://forums.e-hentai.org/index.php")
        .unwrap_or_else(|error| panic!("固定论坛地址应合法: {error:?}"));
    client.post_from_acknowledgement_page(&base, html, 4, "300433", EhentaiOperation::EditPost)
}

#[test]
fn fixed_post_acknowledgement_requires_matching_meta_and_link_proof() {
    let html = br#"
        <html>
          <head>
            <title>Please stand by...</title>
            <meta http-equiv="refresh" content="2; url=index.php?showtopic=300433&amp;p=6956800#entry6956800">
          </head>
          <body>
            <p>Thanks. The post has been edited.</p>
            <a href="index.php?showtopic=300433#entry6956800">click here</a>
          </body>
        </html>
    "#;

    let proof = acknowledged_post(html).unwrap_or_else(|| panic!("两份一致回执应证明固定帖子身份"));

    assert_eq!(proof.thread_id, "300433");
    assert_eq!(proof.post_id, "6956800");
}

#[test]
fn fixed_post_acknowledgement_rejects_conflicting_proofs() {
    let html = br#"
        <html>
          <head>
            <title>Please stand by...</title>
            <meta http-equiv="refresh" content="2; url=index.php?showtopic=300433&amp;p=6956800#entry6956800">
          </head>
          <body>
            <a href="index.php?showtopic=300433#entry6956801">click here</a>
          </body>
        </html>
    "#;

    assert!(acknowledged_post(html).is_none());
}
