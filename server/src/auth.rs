//! 请求身份提取：`Authorization: Bearer <device_token>` 与客户端 IP。
use axum::http::HeaderMap;
use std::net::{IpAddr, SocketAddr};

/// 设备令牌长度上限。超限一律拒绝，避免攻击者用超长字符串撑大限流表的 key。
const MAX_TOKEN_LEN: usize = 128;

/// 从 `Authorization` 头取出设备令牌。
///
/// 令牌只走请求头，不再出现在 query string 中——后者会落进反向代理访问日志与
/// `TraceLayer` 输出，等于把不记名密钥广播出去。
pub fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let mut parts = value.split_ascii_whitespace();
    let scheme = parts.next()?;
    let token = parts.next()?;
    (scheme.eq_ignore_ascii_case("bearer") && parts.next().is_none() && valid_token(token))
        .then(|| token.to_string())
}

fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= MAX_TOKEN_LEN
}

/// 解析限流用的客户端 IP。
///
/// 服务通过 docker-compose 绑在 127.0.0.1 上，真实来源只能由反向代理经
/// `X-Forwarded-For` 传递，因此这里必须信任该头。取**最右侧**一项：客户端若自行
/// 伪造 `X-Forwarded-For`，受信任代理会把真实对端追加到末尾，只有末尾这项不可被
/// 客户端控制。若服务被直接暴露到公网，这层限流即可被绕过——部署时不要这么做。
pub fn client_ip(headers: &HeaderMap, peer: Option<&SocketAddr>) -> String {
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|list| list.rsplit(',').next())
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty())
        .and_then(|candidate| candidate.parse::<IpAddr>().ok());
    if let Some(ip) = forwarded {
        return ip.to_string();
    }
    peer.map(|addr| addr.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// 遮蔽机器人消息里的绑定验证码，供日志使用。
///
/// `/bind 123456` 中的 6 位码是 10 分钟内可绑定该设备的一次性凭据，把用户原文打进日志
/// 等于交给任何能读日志的人。保留命令名以便排查，只遮蔽其参数。
pub fn redact_bind_code(text: &str) -> String {
    let (command, has_args) = match text.split_once(char::is_whitespace) {
        Some((command, _)) => (command, true),
        None => (text, false),
    };
    if !command.starts_with("/bind") {
        return text.to_string();
    }
    if has_args {
        format!("{command} ******")
    } else {
        command.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (key, value) in pairs {
            map.insert(
                axum::http::HeaderName::from_bytes(key.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    #[test]
    fn accepts_wellformed_bearer_token() {
        let map = headers(&[("authorization", "Bearer desktop_abc123")]);
        assert_eq!(bearer_token(&map).as_deref(), Some("desktop_abc123"));
        let map = headers(&[("authorization", "bearer\tdesktop_abc123")]);
        assert_eq!(bearer_token(&map).as_deref(), Some("desktop_abc123"));
    }

    #[test]
    fn rejects_missing_or_malformed_authorization() {
        assert_eq!(bearer_token(&headers(&[])), None);
        assert_eq!(
            bearer_token(&headers(&[("authorization", "desktop_abc")])),
            None
        );
        assert_eq!(
            bearer_token(&headers(&[("authorization", "Basic Zm9v")])),
            None
        );
        assert_eq!(
            bearer_token(&headers(&[("authorization", "Bearer ")])),
            None
        );
        assert_eq!(
            bearer_token(&headers(&[("authorization", "Bearer one two")])),
            None
        );
    }

    #[test]
    fn rejects_oversized_token() {
        let token = "x".repeat(MAX_TOKEN_LEN + 1);
        let map = headers(&[("authorization", &format!("Bearer {token}"))]);
        assert_eq!(bearer_token(&map), None);
    }

    #[test]
    fn takes_rightmost_forwarded_entry() {
        let map = headers(&[("x-forwarded-for", "6.6.6.6, 203.0.113.9")]);
        assert_eq!(client_ip(&map, None), "203.0.113.9");
    }

    #[test]
    fn falls_back_to_peer_and_ignores_unparsable_forwarding() {
        let peer: SocketAddr = "192.168.1.5:54321".parse().unwrap();
        assert_eq!(client_ip(&headers(&[]), Some(&peer)), "192.168.1.5");
        assert_eq!(
            client_ip(&headers(&[("x-forwarded-for", "not-an-ip")]), Some(&peer)),
            "192.168.1.5"
        );
        assert_eq!(client_ip(&headers(&[]), None), "unknown");
    }

    #[test]
    fn redacts_bind_code_arguments() {
        assert_eq!(redact_bind_code("/bind 123456"), "/bind ******");
        assert_eq!(redact_bind_code("/bind 123456 extra"), "/bind ******");
        assert_eq!(redact_bind_code("/bind@MyBot 654321"), "/bind@MyBot ******");
        assert_eq!(redact_bind_code("/bind"), "/bind");
    }

    #[test]
    fn leaves_other_messages_untouched() {
        assert_eq!(redact_bind_code("/today"), "/today");
        assert_eq!(redact_bind_code("/plans"), "/plans");
        assert_eq!(redact_bind_code("帮助"), "帮助");
        assert_eq!(redact_bind_code(""), "");
    }

    #[test]
    fn only_authorization_bearer_header_is_accepted() {
        assert_eq!(
            bearer_token(&headers(&[("x-device-token", "legacy-token")])),
            None
        );
        assert_eq!(
            bearer_token(&headers(&[("authorization", "Basic Zm9v")])),
            None
        );
    }
}
