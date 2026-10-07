//! Разбор ввода адресной строки: URL, «похоже на хост» или поисковый запрос.

use url::Url;

pub const DEFAULT_SEARCH: &str = "https://duckduckgo.com/?q={}";

const SCHEMES_WITH_SLASHES: &[&str] = &["http", "https", "file", "ws", "wss", "ftp"];
const OPAQUE_SCHEMES: &[&str] = &["about", "data", "view-source", "javascript", "blob"];

/// Превращает ввод пользователя в URL. `search` — шаблон поиска с `{}` на месте запроса.
/// `None` — пустой ввод.
pub fn resolve(input: &str, search: &str) -> Option<Url> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    // Явная схема: `https://…`, `file:///…`, `about:blank`, `data:…`. Важно не принять
    // `localhost:3000` за схему `localhost`, поэтому проверяем по белому списку.
    if let Some((scheme, rest)) = input.split_once(':') {
        let scheme_lc = scheme.to_ascii_lowercase();
        let known_slashes =
            SCHEMES_WITH_SLASHES.contains(&scheme_lc.as_str()) && rest.starts_with("//");
        let known_opaque = OPAQUE_SCHEMES.contains(&scheme_lc.as_str());
        if (known_slashes || known_opaque)
            && let Ok(url) = Url::parse(input)
        {
            return Some(url);
        }
    }

    if !input.chars().any(char::is_whitespace) && looks_like_host(input) {
        let scheme = if is_local_host(input) {
            "http"
        } else {
            "https"
        };
        if let Ok(url) = Url::parse(&format!("{scheme}://{input}")) {
            return Some(url);
        }
    }

    let query: String = url::form_urlencoded::byte_serialize(input.as_bytes()).collect();
    Url::parse(&search.replace("{}", &query)).ok()
}

/// Часть до пути/запроса/фрагмента, без userinfo и порта.
fn host_part(input: &str) -> &str {
    let authority = input.split(['/', '?', '#']).next().unwrap_or(input);
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if let Some(rest) = authority.strip_prefix('[') {
        // IPv6: `[::1]:8080`
        return rest.split_once(']').map_or(authority, |(host, _)| host);
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => authority,
    }
}

fn is_local_host(input: &str) -> bool {
    let host = host_part(input);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback() || ip.is_private())
        || host
            .parse::<std::net::Ipv6Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn looks_like_host(input: &str) -> bool {
    let host = host_part(input);
    if host.is_empty() {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost")
        || host.parse::<std::net::Ipv4Addr>().is_ok()
        || host.parse::<std::net::Ipv6Addr>().is_ok()
    {
        return true;
    }
    // Домен: хотя бы одна точка, метки из букв/цифр/дефиса/подчёркивания, TLD не из одних цифр.
    // Юникод (кириллические домены) допускаем: Url::parse сам приведёт к punycode.
    let labels: Vec<&str> = host.trim_end_matches('.').split('.').collect();
    if labels.len() < 2 || labels.iter().any(|label| label.is_empty()) {
        return false;
    }
    let label_ok = |label: &&str| {
        label
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    };
    labels.iter().all(label_ok)
        && !labels
            .last()
            .is_some_and(|tld| tld.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(input: &str) -> String {
        resolve(input, DEFAULT_SEARCH)
            .map(|u| u.to_string())
            .unwrap_or_default()
    }

    #[test]
    fn full_urls_pass_through() {
        assert_eq!(r("https://example.org/a?b=1"), "https://example.org/a?b=1");
        assert_eq!(r("http://example.org"), "http://example.org/");
        assert_eq!(r("about:blank"), "about:blank");
        assert_eq!(r("file:///tmp/x.html"), "file:///tmp/x.html");
    }

    #[test]
    fn bare_hosts_get_https() {
        assert_eq!(r("example.com"), "https://example.com/");
        assert_eq!(r("  example.com/path?x=1 "), "https://example.com/path?x=1");
        assert_eq!(
            r("sub.example.co.uk:8443/x"),
            "https://sub.example.co.uk:8443/x"
        );
        assert_eq!(r("пример.рф"), "https://xn--e1afmkfd.xn--p1ai/");
    }

    #[test]
    fn local_hosts_get_http() {
        assert_eq!(r("localhost:3000"), "http://localhost:3000/");
        assert_eq!(r("127.0.0.1:8080/x"), "http://127.0.0.1:8080/x");
        assert_eq!(r("192.168.1.1"), "http://192.168.1.1/");
        assert_eq!(r("[::1]:9000"), "http://[::1]:9000/");
    }

    #[test]
    fn public_ip_gets_https() {
        assert_eq!(r("8.8.8.8"), "https://8.8.8.8/");
    }

    #[test]
    fn everything_else_is_a_search() {
        assert_eq!(
            r("rust lifetimes"),
            "https://duckduckgo.com/?q=rust+lifetimes"
        );
        assert_eq!(
            r("привет мир"),
            "https://duckduckgo.com/?q=%D0%BF%D1%80%D0%B8%D0%B2%D0%B5%D1%82+%D0%BC%D0%B8%D1%80"
        );
        assert_eq!(r("rust"), "https://duckduckgo.com/?q=rust");
        assert_eq!(r("1.2"), "https://duckduckgo.com/?q=1.2");
        assert_eq!(
            r("what is example.com"),
            "https://duckduckgo.com/?q=what+is+example.com"
        );
    }

    #[test]
    fn empty_is_none() {
        assert!(resolve("   ", DEFAULT_SEARCH).is_none());
    }
}
