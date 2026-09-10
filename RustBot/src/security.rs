use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::{Client, Proxy, StatusCode, header::LOCATION};
use tokio::net::lookup_host;
use url::{Host, Url};

const MAX_REDIRECTS: usize = 5;
const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone)]
pub struct SecureHttpClient {
    proxy: Option<Url>,
    timeout: Duration,
}

impl SecureHttpClient {
    pub fn new(proxy: Option<Url>, timeout: Duration) -> Self {
        Self { proxy, timeout }
    }

    pub async fn get(&self, raw_url: &str) -> Result<Vec<u8>> {
        let mut current = Url::parse(raw_url).context("URL 格式无效")?;
        for redirects in 0..=MAX_REDIRECTS {
            let resolved = validate_public_url(&current).await?;
            let client = self.client_for(&current, &resolved)?;
            let response = client
                .get(current.clone())
                .header("User-Agent", "TGBot_RSS-Rust/1.0")
                .send()
                .await
                .with_context(|| format!("请求 RSS 失败: {}", redact_url(&current)))?;
            if response.status().is_redirection() {
                if redirects == MAX_REDIRECTS {
                    bail!("RSS 重定向次数超过 {MAX_REDIRECTS}");
                }
                let location = response
                    .headers()
                    .get(LOCATION)
                    .context("重定向响应缺少 Location")?
                    .to_str()
                    .context("重定向 Location 不是有效文本")?;
                current = current.join(location).context("重定向地址无效")?;
                continue;
            }
            if response.status() != StatusCode::OK {
                bail!("RSS 返回 HTTP {}", response.status());
            }
            let mut stream = response.bytes_stream();
            let mut body = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                    bail!("RSS 响应超过 {} MiB", MAX_RESPONSE_BYTES / 1024 / 1024);
                }
                body.extend_from_slice(&chunk);
            }
            return Ok(body);
        }
        unreachable!("redirect loop always returns or fails")
    }

    fn client_for(&self, url: &Url, resolved: &[SocketAddr]) -> Result<Client> {
        let mut builder = Client::builder()
            .timeout(self.timeout)
            .redirect(reqwest::redirect::Policy::none());
        if let Some(proxy) = &self.proxy {
            builder = builder.proxy(Proxy::all(proxy.as_str())?);
        } else if let Some(host) = url.host_str() {
            builder = builder.resolve_to_addrs(host, resolved);
        }
        Ok(builder.build()?)
    }
}

pub fn standard_client(proxy: Option<&str>, timeout: Duration) -> Result<Client> {
    let mut builder = Client::builder().timeout(timeout);
    if let Some(raw) = proxy.filter(|value| !value.trim().is_empty()) {
        builder = builder.proxy(Proxy::all(raw)?);
    }
    Ok(builder.build()?)
}

pub async fn validate_public_http_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw.trim()).context("无效的 URL 格式")?;
    validate_public_url(&url).await?;
    Ok(url)
}

async fn validate_public_url(url: &Url) -> Result<Vec<SocketAddr>> {
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        bail!("请使用带主机名的完整 http/https URL");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("URL 不允许包含用户凭据");
    }
    let port = url.port_or_known_default().context("URL 缺少有效端口")?;
    let ips: Vec<IpAddr> = match url.host().expect("host checked above") {
        Host::Ipv4(ip) => vec![IpAddr::V4(ip)],
        Host::Ipv6(ip) => vec![IpAddr::V6(ip)],
        Host::Domain(host) => lookup_host((host, port))
            .await
            .with_context(|| format!("无法解析 RSS 主机 {host}"))?
            .map(|address| address.ip())
            .collect(),
    };
    if ips.is_empty() {
        bail!("RSS 主机没有可用地址");
    }
    if ips.iter().any(|ip| !is_public_ip(*ip)) {
        bail!("RSS 地址不能指向本机、内网或保留地址");
    }
    Ok(ips
        .into_iter()
        .map(|ip| SocketAddr::new(ip, port))
        .collect())
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified()
                || ip.is_multicast()
                || octets[0] == 0
                || octets[0] >= 240
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 192 && octets[1] == 88 && octets[2] == 99)
                || (octets[0] == 198 && (18..=19).contains(&octets[1])))
        }
        IpAddr::V6(ip) => {
            if let Some(embedded) = ip.to_ipv4() {
                return is_public_ip(IpAddr::V4(embedded));
            }
            let segments = ip.segments();
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (segments[0] & 0xe000) != 0x2000
                || (segments[0] == 0x2001 && (segments[1] & 0xfe00) == 0)
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80
                || (segments[0] & 0xffc0) == 0xfec0
                || (segments[0] == 0x2001 && segments[1] == 0x0db8)
                || (segments[0] == 0x0064 && segments[1] == 0xff9b)
                || segments[0] == 0x2002
                || (segments[0] == 0x3fff && (segments[1] & 0xf000) == 0)
                || segments[0] == 0x5f00)
        }
    }
}

fn redact_url(url: &Url) -> String {
    format!(
        "{}://{}{}",
        url.scheme(),
        url.host_str().unwrap_or("?"),
        url.path()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_local_and_credentialed_urls() {
        for raw in [
            "http://127.0.0.1/feed",
            "http://[::1]/feed",
            "http://[::ffff:127.0.0.1]/feed",
            "http://[::127.0.0.1]/feed",
            "http://[64:ff9b::7f00:1]/feed",
            "http://[2002:7f00:1::]/feed",
            "http://[fec0::1]/feed",
            "http://192.0.0.1/feed",
            "http://192.0.0.170/feed",
            "http://192.88.99.2/feed",
            "file:///tmp/feed",
            "http://user:pass@example.com/feed",
        ] {
            assert!(
                validate_public_http_url(raw).await.is_err(),
                "accepted {raw}"
            );
        }
    }

    #[tokio::test]
    async fn accepts_public_literal() {
        assert!(
            validate_public_http_url("https://8.8.8.8/feed")
                .await
                .is_ok()
        );
        assert!(
            validate_public_http_url("https://[2001:4860:4860::8888]/feed")
                .await
                .is_ok()
        );
    }
}
