//! The origin of a URL, for keeping the write token to the page's own server.

/// The origin of an absolute URL: `scheme://host[:port]` in lowercase,
/// without user info or a default port, and with a websocket's scheme as
/// the page's (`ws` as `http`, `wss` as `https`), so a socket and a page
/// on one server have one origin. `None` for anything else, including a
/// URL with a backslash, a control character or white space: a browser
/// reads those otherwise (`\` is `/` to it, so `ws://evil\@server/` is a
/// socket to `evil`).
pub fn url_origin(url: &str) -> Option<String> {
    // In a browser, first as the browser reads it.
    #[cfg(target_arch = "wasm32")]
    let url = &web_sys::Url::new(url).ok()?.href();
    let url = url.trim();
    if url
        .chars()
        .any(|c| c == '\\' || c.is_control() || c.is_whitespace())
    {
        return None;
    }
    let (scheme, rest) = url.split_once("://")?;
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "http" | "ws" => "http",
        "https" | "wss" => "https",
        _ => return None,
    };
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if host.is_empty() {
        return None;
    }
    let host = host.to_ascii_lowercase();
    let default = if scheme == "http" { ":80" } else { ":443" };
    let host = host.strip_suffix(default).unwrap_or(&host);
    Some(format!("{scheme}://{host}"))
}

/// Whether two URLs are on one server (see [`url_origin`]).
pub fn same_origin(a: &str, b: &str) -> bool {
    url_origin(a).is_some_and(|a| Some(a) == url_origin(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        let o = |u| url_origin(u);
        assert_eq!(
            o("http://Host:8080/a.bob?x").as_deref(),
            Some("http://host:8080")
        );
        assert_eq!(o("https://host:443/").as_deref(), Some("https://host"));
        assert_eq!(o("wss://host/pvws/pv").as_deref(), Some("https://host"));
        assert_eq!(o("ws://u:p@host:80").as_deref(), Some("http://host"));
        assert_eq!(
            o("http://[::1]:8080/x").as_deref(),
            Some("http://[::1]:8080")
        );
        assert_eq!(o("file:///x"), None);
        assert_eq!(o("a.bob"), None);
        assert_eq!(o("http:///x"), None);
    }

    #[test]
    fn same_origins() {
        assert!(same_origin(
            "http://server:8080/?display=a.bob",
            "ws://server:8080/pvws/pv"
        ));
        assert!(same_origin("https://server/x", "wss://server:443/pvws/pv"));
        assert!(!same_origin("https://server/x", "ws://server/pvws/pv"));
        assert!(!same_origin(
            "http://server:8080/",
            "ws://evil:8080/pvws/pv"
        ));
        assert!(!same_origin("http://server:8080/", "https://evil/x.bob"));
        // A user-info trick names another host.
        assert!(!same_origin("http://server/", "http://server@evil/x.bob"));
        assert!(!same_origin("x", "x"));
        // A browser takes `\` for `/`: these are sockets to `evil`.
        let page = "http://server:8080/";
        for socket in [
            r"ws://evil\@server:8080/pvws/pv",
            r"ws://evil\\@server:8080/pvws/pv",
            "ws://evil\t@server:8080/pvws/pv",
            "ws://evil\n@server:8080/pvws/pv",
            "ws://evil @server:8080/pvws/pv",
            r"ws:\\server:8080/pvws/pv",
        ] {
            assert!(!same_origin(page, socket), "{socket}");
            assert_eq!(url_origin(socket), None, "{socket}");
        }
        // Encoded, a backslash is only text in the user info.
        assert!(same_origin(page, "ws://evil%5C@server:8080/pvws/pv"));
    }
}
