use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use clap::Parser;
use hickory_proto::rr::Name;

const MIN_API_KEY_LEN: usize = 32;

#[derive(Parser, Debug, Clone)]
#[command(name = "safexip", version, about)]
pub struct Config {
    #[arg(
        long,
        env = "SAFEXIP_DOMAIN",
        default_value = "xip.example.com",
        value_parser = parse_dns_name
    )]
    pub domain: String,

    #[arg(
        long,
        env = "SAFEXIP_NS_HOSTNAME",
        default_value = "ns1.xip.example.com",
        value_parser = parse_dns_name
    )]
    pub ns_hostname: String,

    #[arg(
        long,
        env = "SAFEXIP_NS_HOSTNAME2",
        default_value = "ns2.xip.example.com",
        value_parser = parse_dns_name
    )]
    pub ns_hostname2: String,

    #[arg(long, env = "SAFEXIP_DNS_BIND", default_value = "0.0.0.0")]
    pub dns_bind: IpAddr,

    #[arg(
        long,
        env = "SAFEXIP_DNS_PORT",
        default_value_t = 53,
        value_parser = clap::value_parser!(u16).range(1..)
    )]
    pub dns_port: u16,

    #[arg(long, env = "SAFEXIP_API_BIND", default_value = "127.0.0.1")]
    pub api_bind: IpAddr,

    #[arg(
        long,
        env = "SAFEXIP_API_PORT",
        default_value_t = 8080,
        value_parser = clap::value_parser!(u16).range(1..)
    )]
    pub api_port: u16,

    #[arg(long, env = "SAFEXIP_NS_IP", default_value = "127.0.0.1")]
    pub ns_ip: Ipv4Addr,

    #[arg(long, env = "SAFEXIP_API_KEY", value_parser = parse_api_key)]
    pub api_key: String,

    #[arg(
        long,
        env = "SAFEXIP_TXT_TTL",
        default_value_t = 60,
        value_parser = clap::value_parser!(u32).range(1..=86400)
    )]
    pub txt_ttl: u32,

    #[arg(
        long,
        env = "SAFEXIP_DEFAULT_TTL",
        default_value_t = 60,
        value_parser = clap::value_parser!(u32).range(1..=86400)
    )]
    pub default_ttl: u32,

    /// Maximum time an ACME challenge token remains available without cleanup.
    #[arg(
        long,
        env = "SAFEXIP_TOKEN_LIFETIME",
        default_value_t = 600,
        value_parser = clap::value_parser!(u64).range(1..=86400)
    )]
    pub token_lifetime: u64,

    /// Maximum number of simultaneously active ACME challenge tokens.
    #[arg(
        long,
        env = "SAFEXIP_MAX_TOKENS",
        default_value_t = 100,
        value_parser = parse_max_tokens
    )]
    pub max_tokens: usize,
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.ns_hostname == self.ns_hostname2 {
            return Err("the two nameserver hostnames must be different".into());
        }
        for ns in [&self.ns_hostname, &self.ns_hostname2] {
            if ns != &self.domain && !ns.ends_with(&format!(".{}", self.domain)) {
                return Err(format!(
                    "nameserver {ns} must be inside the delegated zone {}",
                    self.domain
                ));
            }
        }
        for derived in [self.acme_name(), format!("admin.{}", self.domain)] {
            Name::from_ascii(&derived)
                .map_err(|error| format!("derived DNS name {derived} is invalid: {error}"))?;
        }
        Ok(())
    }

    pub fn token_lifetime(&self) -> Duration {
        Duration::from_secs(self.token_lifetime)
    }

    pub fn acme_name(&self) -> String {
        format!("_acme-challenge.{}", self.domain)
    }
}

fn parse_dns_name(raw: &str) -> Result<String, String> {
    let normalized = raw.trim_end_matches('.').to_ascii_lowercase();
    if normalized.is_empty() || normalized == "." {
        return Err("DNS name must not be empty or the root zone".into());
    }
    Name::from_ascii(&normalized).map_err(|error| format!("invalid DNS name: {error}"))?;
    Ok(normalized)
}

fn parse_api_key(raw: &str) -> Result<String, String> {
    if raw.len() < MIN_API_KEY_LEN {
        return Err(format!(
            "API key must contain at least {MIN_API_KEY_LEN} characters"
        ));
    }
    Ok(raw.to_owned())
}

fn parse_max_tokens(raw: &str) -> Result<usize, String> {
    let value = raw
        .parse::<usize>()
        .map_err(|error| format!("invalid token limit: {error}"))?;
    if !(1..=10_000).contains(&value) {
        return Err("token limit must be between 1 and 10000".into());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    fn valid_config() -> Config {
        Config {
            domain: "xip.test".into(),
            ns_hostname: "ns1.xip.test".into(),
            ns_hostname2: "ns2.xip.test".into(),
            dns_bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            dns_port: 53,
            api_bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            api_port: 8080,
            ns_ip: Ipv4Addr::LOCALHOST,
            api_key: "0123456789abcdef0123456789abcdef".into(),
            txt_ttl: 60,
            default_ttl: 60,
            token_lifetime: 600,
            max_tokens: 100,
        }
    }

    #[test]
    fn normalizes_dns_names() {
        assert_eq!(
            parse_dns_name("XIP.Example.COM.").unwrap(),
            "xip.example.com"
        );
    }

    #[test]
    fn rejects_invalid_dns_names() {
        assert!(parse_dns_name("").is_err());
        assert!(parse_dns_name("bad name.example").is_err());
    }

    #[test]
    fn rejects_empty_and_short_api_keys() {
        assert!(parse_api_key("").is_err());
        assert!(parse_api_key("too-short").is_err());
        assert!(parse_api_key(&"x".repeat(MIN_API_KEY_LEN)).is_ok());
    }

    #[test]
    fn requires_distinct_in_zone_nameservers() {
        let mut config = valid_config();
        config.ns_hostname2 = config.ns_hostname.clone();
        assert!(config.validate().is_err());

        config.ns_hostname2 = "ns.other.test".into();
        assert!(config.validate().is_err());
    }
}
