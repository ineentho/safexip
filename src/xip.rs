use std::net::Ipv4Addr;

// DNS names are case-insensitive (RFC 4343). Resolvers like Google's use 0x20
// case randomization in query names (e.g. `_acmE-CHaLlENge.xIp.ExAmPle.COM`)
// as a cache-poisoning mitigation, so every comparison must be case-folded.
pub fn is_our_domain(hostname: &str, domain: &str) -> bool {
    let hostname = hostname.to_lowercase();
    let domain = domain.to_lowercase();
    hostname == domain || hostname.ends_with(&format!(".{domain}"))
}

pub fn parse_xip_ip(hostname: &str, domain: &str) -> Option<Ipv4Addr> {
    let hostname = hostname.to_lowercase();
    let domain = domain.to_lowercase();
    let prefix = hostname.strip_suffix(&format!(".{domain}"))?;
    let parts: Vec<&str> = prefix.split('-').collect();

    // Scan from the end for 4 consecutive numeric parts
    if parts.len() < 4 {
        return None;
    }
    for i in (0..=parts.len() - 4).rev() {
        if let Some(ip) = parse_octets(&parts[i..i + 4]) {
            return Some(ip);
        }
    }
    None
}

fn parse_octets(octets: &[&str]) -> Option<Ipv4Addr> {
    let mut b = [0u8; 4];
    for (i, s) in octets.iter().enumerate() {
        let val: u16 = s.parse().ok()?;
        if val > 255 {
            return None;
        }
        b[i] = val as u8;
    }
    Some(Ipv4Addr::from(b))
}
