use super::super::*;

#[test]
fn split_proxy_recognises_schemes() {
    assert_eq!(split_proxy(""), ("none".into(), "".into()));
    assert_eq!(
        split_proxy("http://10.0.0.1:1022"),
        ("http".into(), "10.0.0.1:1022".into())
    );
    assert_eq!(
        split_proxy("socks5://127.0.0.1:1080"),
        ("socks5".into(), "127.0.0.1:1080".into())
    );
    assert_eq!(
        split_proxy("http://u:p@host:8080"),
        ("http".into(), "u:p@host:8080".into())
    );
    assert_eq!(
        split_proxy("127.0.0.1:1080"),
        ("socks5".into(), "127.0.0.1:1080".into())
    );
}
