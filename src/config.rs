use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "safexip")]
pub struct Config {
    #[arg(long, env = "SAFEXIP_DOMAIN", default_value = "xip.example.com")]
    pub domain: String,

    #[arg(long, env = "SAFEXIP_NS_HOSTNAME", default_value = "ns1.xip.example.com")]
    pub ns_hostname: String,

    #[arg(long, env = "SAFEXIP_NS_HOSTNAME2", default_value = "ns2.xip.example.com")]
    pub ns_hostname2: String,

    #[arg(long, env = "SAFEXIP_DNS_BIND", default_value = "0.0.0.0")]
    pub dns_bind: String,

    #[arg(long, env = "SAFEXIP_DNS_PORT", default_value_t = 53)]
    pub dns_port: u16,

    #[arg(long, env = "SAFEXIP_API_BIND", default_value = "0.0.0.0")]
    pub api_bind: String,

    #[arg(long, env = "SAFEXIP_API_PORT", default_value_t = 8080)]
    pub api_port: u16,

    #[arg(long, env = "SAFEXIP_NS_IP", default_value = "127.0.0.1")]
    pub ns_ip: String,

    #[arg(long, env = "SAFEXIP_API_KEY")]
    pub api_key: String,

    #[arg(long, env = "SAFEXIP_TXT_TTL", default_value_t = 60)]
    pub txt_ttl: u32,

    #[arg(long, env = "SAFEXIP_DEFAULT_TTL", default_value_t = 60)]
    pub default_ttl: u32,
}
