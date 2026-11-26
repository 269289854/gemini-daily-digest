//! DNS-aware SSRF guard blocking loopback, private, link-local,
//! and cloud metadata IPs with `SSRF_ALLOWLIST_HOSTS` support.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use ipnet::{Ipv4Net, Ipv6Net};
use url::Url;

use crate::config::AppConfig;
use crate::constants::{
    SSRF_ALLOWED_SCHEMES, SSRF_BLOCKED_IPV4_CIDRS, SSRF_BLOCKED_IPV6_CIDRS,
    SSRF_BLOCKED_METADATA_HOSTS,
};
use crate::error::CoreError;

/// Validates outbound URLs and resolved IP addresses against SSRF policies.
#[derive(Debug, Clone)]
pub struct SsrfGuard {
    allowlist_hosts: Vec<String>,
}

impl SsrfGuard {
    /// Creates a new `SsrfGuard` with an explicit host allowlist.
    #[must_use]
    pub fn new(allowlist_hosts: Vec<String>) -> Self {
        let normalized = allowlist_hosts
            .into_iter()
            .map(|host| normalize_host(&host))
            .filter(|host| !host.is_empty())
            .collect();
        Self {
            allowlist_hosts: normalized,
        }
    }

    /// Creates a `SsrfGuard` configured from `AppConfig`.
    #[must_use]
    pub fn from_config(config: &AppConfig) -> Self {
        Self::new(config.ssrf_allowlist_hosts.clone())
    }

    /// Returns `true` if `host` matches an explicitly allowlisted host entry.
    #[must_use]
    pub fn is_allowlisted(&self, host: &str) -> bool {
        let normalized = normalize_host(host);
        !normalized.is_empty()
            && self
                .allowlist_hosts
                .iter()
                .any(|allowed| allowed == &normalized)
    }

    /// Validates that `ip` is a globally routable public IP address.
    pub fn validate_ip(&self, ip: IpAddr) -> Result<(), CoreError> {
        match ip {
            IpAddr::V4(v4) => validate_ipv4(v4),
            IpAddr::V6(v6) => validate_ipv6(v6),
        }
    }

    /// Parses `raw_url`, checks scheme/host rules, resolves DNS, and validates every resolved IP.
    pub async fn validate_url(&self, raw_url: &str) -> Result<Url, CoreError> {
        let trimmed = raw_url.trim();
        let url = Url::parse(trimmed)
            .map_err(|err| CoreError::InvalidInput(format!("invalid url: {err}")))?;

        let scheme = url.scheme();
        if !SSRF_ALLOWED_SCHEMES.contains(&scheme) {
            return Err(CoreError::SsrfBlocked(format!(
                "URL scheme '{scheme}' is not allowed"
            )));
        }

        if !url.username().is_empty() || url.password().is_some() {
            return Err(CoreError::SsrfBlocked(
                "embedded URL credentials are not allowed".to_owned(),
            ));
        }

        let raw_host = url
            .host_str()
            .ok_or_else(|| CoreError::SsrfBlocked("URL must contain a host".to_owned()))?;
        let host = normalize_host(raw_host);
        if host.is_empty() {
            return Err(CoreError::SsrfBlocked(
                "URL host cannot be empty".to_owned(),
            ));
        }

        if self.is_allowlisted(&host) {
            return Ok(url);
        }

        if is_blocked_metadata_host(&host) {
            return Err(CoreError::SsrfBlocked(format!(
                "host '{host}' is blocked by metadata/local policy"
            )));
        }

        if let Ok(ip) = host.parse::<IpAddr>() {
            self.validate_ip(ip)?;
            return Ok(url);
        }

        let port = url.port_or_known_default().unwrap_or(443);
        let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|err| {
                CoreError::SsrfBlocked(format!("DNS resolution failed for '{host}': {err}"))
            })?
            .collect();

        if addrs.is_empty() {
            return Err(CoreError::SsrfBlocked(format!(
                "DNS resolution returned no addresses for '{host}'"
            )));
        }

        for addr in addrs {
            self.validate_ip(addr.ip())?;
        }

        Ok(url)
    }
}

fn normalize_host(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

fn is_blocked_metadata_host(host: &str) -> bool {
    SSRF_BLOCKED_METADATA_HOSTS.iter().any(|blocked| {
        if host == *blocked {
            return true;
        }
        // Only apply subdomain matching to hostnames (not IP literals).
        blocked.parse::<IpAddr>().is_err() && host.ends_with(&format!(".{blocked}"))
    })
}

fn validate_ipv4(v4: Ipv4Addr) -> Result<(), CoreError> {
    if v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.is_multicast()
        || v4.is_documentation()
    {
        return Err(CoreError::SsrfBlocked(format!(
            "IPv4 address {v4} is in a non-public range"
        )));
    }

    for cidr in SSRF_BLOCKED_IPV4_CIDRS {
        let net: Ipv4Net = cidr.parse().map_err(|err| {
            CoreError::Internal(format!("invalid blocked IPv4 CIDR '{cidr}': {err}"))
        })?;
        if net.contains(&v4) {
            return Err(CoreError::SsrfBlocked(format!(
                "IPv4 address {v4} matches blocked CIDR {cidr}"
            )));
        }
    }

    Ok(())
}

fn validate_ipv6(v6: Ipv6Addr) -> Result<(), CoreError> {
    if let Some(mapped_v4) = v6.to_ipv4_mapped() {
        return validate_ipv4(mapped_v4);
    }

    let [first_segment, ..] = v6.segments();
    let is_unique_local = (first_segment & 0xfe00) == 0xfc00;
    let is_unicast_link_local = (first_segment & 0xffc0) == 0xfe80;

    if v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        || is_unique_local
        || is_unicast_link_local
    {
        return Err(CoreError::SsrfBlocked(format!(
            "IPv6 address {v6} is in a non-public range"
        )));
    }

    for cidr in SSRF_BLOCKED_IPV6_CIDRS {
        let net: Ipv6Net = cidr.parse().map_err(|err| {
            CoreError::Internal(format!("invalid blocked IPv6 CIDR '{cidr}': {err}"))
        })?;
        if net.contains(&v6) {
            return Err(CoreError::SsrfBlocked(format!(
                "IPv6 address {v6} matches blocked CIDR {cidr}"
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn blocks_loopback_private_cgnat_metadata_and_ipv4_mapped_urls() {
        let guard = SsrfGuard::new(Vec::new());

        let blocked_urls = [
            "http://127.0.0.1/feed.xml",
            "http://localhost:8080/api",
            "http://sub.localhost/api",
            "http://10.0.0.1/internal",
            "http://172.16.5.4/internal",
            "http://192.168.1.10/internal",
            "http://100.64.0.1/cgnat",
            "http://169.254.169.254/latest/meta-data",
            "http://metadata.google.internal/computeMetadata/v1/",
            "http://[::1]/",
            "http://[fc00::1]/",
            "http://[fe80::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:169.254.169.254]/",
            "file:///etc/passwd",
            "https://user:pass@example.com/rss",
        ];

        for raw in blocked_urls {
            let result = guard.validate_url(raw).await;
            assert!(
                matches!(result, Err(CoreError::SsrfBlocked(_))),
                "expected SsrfBlocked for {raw}, got {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn allows_public_ip_and_allowlisted_hosts() {
        let guard = SsrfGuard::new(vec!["127.0.0.1".to_owned(), "Mock.Local".to_owned()]);

        let public_ok = guard.validate_url("https://93.184.216.34/rss.xml").await;
        assert!(
            public_ok.is_ok(),
            "expected public IP to pass: {public_ok:?}"
        );

        let allowlisted_ip_ok = guard.validate_url("http://127.0.0.1:18080/feed").await;
        assert!(
            allowlisted_ip_ok.is_ok(),
            "expected allowlisted IP to pass: {allowlisted_ip_ok:?}"
        );

        let allowlisted_host_ok = guard.validate_url("http://mock.local/feed.xml").await;
        assert!(
            allowlisted_host_ok.is_ok(),
            "expected allowlisted host to pass: {allowlisted_host_ok:?}"
        );
    }

    #[test]
    fn validate_ip_covers_ipv4_and_ipv6_edge_cases() {
        let guard = SsrfGuard::from_config(&AppConfig::default());

        assert!(guard.validate_ip("93.184.216.34".parse().unwrap()).is_ok());
        assert!(guard
            .validate_ip("2606:2800:220:1:248:1893:25c8:1946".parse().unwrap())
            .is_ok());

        for blocked in [
            "0.0.0.0",
            "127.0.0.2",
            "10.20.30.40",
            "172.31.255.254",
            "192.168.255.1",
            "100.127.255.254",
            "169.254.1.1",
            "198.18.0.1",
            "224.0.0.1",
            "::",
            "::1",
            "fc00::1",
            "fd00:ec2::254",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
            "::ffff:10.0.0.1",
        ] {
            let ip: IpAddr = blocked.parse().unwrap();
            assert!(
                matches!(guard.validate_ip(ip), Err(CoreError::SsrfBlocked(_))),
                "expected {blocked} to be blocked"
            );
        }
    }
}
