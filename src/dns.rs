use std::net::Ipv4Addr;

use hickory_proto::op::{Edns, Message, OpCode, ResponseCode};
use hickory_proto::rr::rdata::{A, NS, SOA, TXT};
use hickory_proto::rr::{Name, RData, Record, RecordType};

use crate::config::Config;
use crate::state::AcmeRecords;
use crate::xip;

const MAX_UDP_PAYLOAD: u16 = 1232;

pub struct DnsHandler {
    pub config: Config,
    pub acme: AcmeRecords,
}

impl DnsHandler {
    pub async fn handle(&self, raw: &[u8]) -> Vec<u8> {
        let request = match Message::from_vec(raw) {
            Ok(msg) => msg,
            Err(_) => return b"".to_vec(),
        };

        let response = self.build_response(&request).await;
        response.to_vec().unwrap_or_default()
    }

    pub async fn handle_udp(&self, raw: &[u8]) -> Vec<u8> {
        let request = match Message::from_vec(raw) {
            Ok(msg) => msg,
            Err(_) => return Vec::new(),
        };
        let max_payload = usize::from(request.max_payload().min(MAX_UDP_PAYLOAD));
        let response = self.build_response(&request).await;
        let encoded = response.to_vec().unwrap_or_default();
        if encoded.len() <= max_payload {
            encoded
        } else {
            response.truncate().to_vec().unwrap_or_default()
        }
    }

    async fn build_response(&self, request: &Message) -> Message {
        let mut response = Message::response(request.metadata.id, request.metadata.op_code);
        response.metadata.authoritative = true;
        response.metadata.recursion_desired = request.metadata.recursion_desired;
        response.metadata.recursion_available = false;
        if let Some(request_edns) = &request.edns {
            let mut response_edns = Edns::new();
            response_edns
                .set_max_payload(request_edns.max_payload().min(MAX_UDP_PAYLOAD))
                .set_dnssec_ok(request_edns.flags().dnssec_ok);
            response.set_edns(response_edns);
            if request_edns.version() != 0 {
                response.metadata.authoritative = false;
                response.metadata.response_code = ResponseCode::BADVERS;
                return response;
            }
        }

        if request.metadata.op_code != OpCode::Query {
            response.metadata.authoritative = false;
            response.metadata.response_code = ResponseCode::NotImp;
            return response;
        }

        if request.queries.len() != 1 {
            response.metadata.response_code = ResponseCode::FormErr;
            return response;
        }

        let query = &request.queries[0];
        let qname = query.name();
        let qtype = query.query_type();
        // Case-fold for all comparisons (RFC 4343). The response still echoes
        // the original query name via `add_query` below, so 0x20 randomization
        // used by resolvers like Google's is preserved in the question section.
        let qname_str = qname.to_ascii().trim_end_matches('.').to_lowercase();

        tracing::debug!("DNS query: {qname_str} {:?}", qtype);

        response.add_query(query.clone());

        if !xip::is_our_domain(&qname_str, &self.config.domain) {
            response.metadata.authoritative = false;
            response.metadata.response_code = ResponseCode::Refused;
            return response;
        }

        // Always include SOA in authority for in-zone responses.
        response.add_authority(self.soa());

        match qtype {
            RecordType::SOA => {
                response.add_answer(self.soa());
            }
            RecordType::NS => {
                let zone = Name::from_ascii(&self.config.domain).unwrap();
                let nss = [&self.config.ns_hostname, &self.config.ns_hostname2];
                for ns in nss {
                    let rec = Record::from_rdata(
                        zone.clone(),
                        self.config.default_ttl,
                        RData::NS(NS(Name::from_ascii(ns).unwrap())),
                    );
                    response.add_answer(rec);
                }
                for glue in self.ns_glue() {
                    response.add_additional(glue);
                }
            }
            RecordType::A => {
                if let Some(ip) = self.resolve_a(&qname_str) {
                    response.add_answer(self.a_record(qname.clone(), ip));
                } else if !self.name_exists(&qname_str) {
                    response.metadata.response_code = ResponseCode::NXDomain;
                }
                // else: NODATA — name exists in zone but has no A record.
                // MUST NOT return NXDOMAIN here: RFC 8020 lets validating
                // resolvers (Google, and thus Let's Encrypt) cache the whole
                // name as nonexistent, which then poisons TXT lookups at the
                // same name and breaks DNS-01 validation. SOA is already in
                // the authority section above.
            }
            RecordType::TXT => {
                let records = self.resolve_txt(&qname_str).await;
                if records.is_empty() {
                    if !self.name_exists(&qname_str) {
                        response.metadata.response_code = ResponseCode::NXDomain;
                    }
                } else {
                    for txt in records {
                        response.add_answer(txt);
                    }
                }
                // else: NODATA — name exists but no TXT record right now.
                // Per-type negative caching only (not NXDOMAIN-cut), so a
                // subsequent TXT query after the token is set still resolves.
            }
            _ => {
                // NODATA: name exists in zone but record type absent (e.g. CAA).
                // REFUSED would be treated as a DNS failure by CAs.
                if !self.name_exists(&qname_str) {
                    response.metadata.response_code = ResponseCode::NXDomain;
                }
            }
        }

        response
    }

    fn resolve_a(&self, qname: &str) -> Option<Ipv4Addr> {
        if qname == self.config.ns_hostname
            || qname == self.config.ns_hostname2
            || qname == self.config.domain
        {
            return Some(self.config.ns_ip);
        }
        if let Some(ip) = xip::parse_xip_ip(qname, &self.config.domain) {
            return Some(ip);
        }
        None
    }

    async fn resolve_txt(&self, qname: &str) -> Vec<Record> {
        let acme_name = self.config.acme_name();
        if qname != acme_name {
            return Vec::new();
        }

        let values = self.acme.get(&acme_name).await;
        let Ok(name) = Name::from_ascii(qname) else {
            return Vec::new();
        };
        values
            .into_iter()
            .map(|value| {
                Record::from_rdata(
                    name.clone(),
                    self.config.txt_ttl,
                    RData::TXT(TXT::new(vec![value])),
                )
            })
            .collect()
    }

    /// Whether `qname` is a name that exists in this zone (even if it has no
    /// record of the requested type). Existing names must answer NODATA, never
    /// NXDOMAIN, to avoid RFC 8020 NXDOMAIN-cut poisoning sibling record types.
    fn name_exists(&self, qname: &str) -> bool {
        if qname == self.config.domain
            || qname == self.config.ns_hostname
            || qname == self.config.ns_hostname2
        {
            return true;
        }
        if qname == self.config.acme_name() {
            return true;
        }
        // Any name encoding a valid xip IP octets exists and has an A record.
        if xip::parse_xip_ip(qname, &self.config.domain).is_some() {
            return true;
        }
        false
    }

    fn a_record(&self, name: Name, ip: Ipv4Addr) -> Record {
        Record::from_rdata(
            name,
            self.config.default_ttl,
            RData::A(A::new(
                ip.octets()[0],
                ip.octets()[1],
                ip.octets()[2],
                ip.octets()[3],
            )),
        )
    }

    fn ns_glue(&self) -> Vec<Record> {
        vec![
            self.a_record(
                Name::from_ascii(&self.config.ns_hostname).unwrap(),
                self.config.ns_ip,
            ),
            self.a_record(
                Name::from_ascii(&self.config.ns_hostname2).unwrap(),
                self.config.ns_ip,
            ),
        ]
    }

    fn soa(&self) -> Record {
        let serial = chrono::Utc::now().timestamp() as u32;
        let soa = SOA::new(
            Name::from_ascii(&self.config.ns_hostname).unwrap(),
            Name::from_ascii(format!("admin.{}", self.config.domain)).unwrap(),
            serial,
            3600,
            900,
            86400,
            10, // minimum TTL — governs negative caching (NXDOMAIN + NODATA). Low for fast ACME retry.
        );
        Record::from_rdata(
            Name::from_ascii(&self.config.domain).unwrap(),
            10, // SOA record TTL — also used for negative caching by some resolvers.
            RData::SOA(soa),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Duration;

    use super::*;
    use hickory_proto::op::{Message, MessageType, Query};

    fn make_config() -> Config {
        Config {
            domain: "xip.test".to_string(),
            ns_hostname: "ns1.xip.test".to_string(),
            ns_hostname2: "ns2.xip.test".to_string(),
            dns_bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            dns_port: 53,
            api_bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            api_port: 8080,
            ns_ip: Ipv4Addr::LOCALHOST,
            api_key: "0123456789abcdef0123456789abcdef".to_string(),
            txt_ttl: 60,
            default_ttl: 60,
            token_lifetime: 600,
            max_tokens: 100,
        }
    }

    fn acme() -> AcmeRecords {
        AcmeRecords::new(Duration::from_secs(60), 100)
    }

    fn query_msg(name: &str, qtype: RecordType) -> Vec<u8> {
        let mut msg = Message::new(1, MessageType::Query, OpCode::Query);
        msg.metadata.recursion_desired = true;
        msg.add_query(Query::query(Name::from_ascii(name).unwrap(), qtype));
        msg.to_vec().unwrap()
    }

    async fn run(handler: &DnsHandler, name: &str, qtype: RecordType) -> Message {
        let raw = query_msg(name, qtype);
        let resp_bytes = handler.handle(&raw).await;
        Message::from_vec(&resp_bytes).unwrap()
    }

    // Regression for the NXDOMAIN-cut bug (RFC 8020): an A query on the ACME
    // challenge name must be NODATA, never NXDOMAIN. Previously it returned
    // NXDOMAIN, which let validating resolvers cache the whole name as
    // nonexistent and break the subsequent TXT lookup that LE relies on.
    #[tokio::test]
    async fn acme_challenge_a_is_nodata_not_nxdomain() {
        let handler = DnsHandler {
            config: make_config(),
            acme: acme(),
        };
        let resp = run(&handler, "_acme-challenge.xip.test", RecordType::A).await;
        assert_ne!(
            resp.metadata.response_code,
            ResponseCode::NXDomain,
            "A query on an existing name must not be NXDOMAIN (RFC 8020 NXDOMAIN-cut)"
        );
        assert_eq!(resp.metadata.response_code, ResponseCode::NoError);
        assert!(resp.answers.is_empty(), "expected NODATA (no A record)");
    }

    #[tokio::test]
    async fn acme_challenge_txt_no_token_is_nodata_not_nxdomain() {
        let handler = DnsHandler {
            config: make_config(),
            acme: acme(),
        };
        let resp = run(&handler, "_acme-challenge.xip.test", RecordType::TXT).await;
        assert_ne!(
            resp.metadata.response_code,
            ResponseCode::NXDomain,
            "TXT with no token must not be NXDOMAIN (would poison the name via NXDOMAIN-cut)"
        );
        assert_eq!(resp.metadata.response_code, ResponseCode::NoError);
        assert!(resp.answers.is_empty());
    }

    #[tokio::test]
    async fn acme_challenge_txt_with_token_returns_answer() {
        let acme = acme();
        acme.add(
            "_acme-challenge.xip.test".to_string(),
            "token123".to_string(),
        )
        .await
        .unwrap();
        let handler = DnsHandler {
            config: make_config(),
            acme,
        };
        let resp = run(&handler, "_acme-challenge.xip.test", RecordType::TXT).await;
        assert_eq!(resp.metadata.response_code, ResponseCode::NoError);
        assert_eq!(resp.answers.len(), 1);
    }

    #[tokio::test]
    async fn concurrent_tokens_are_separate_txt_records() {
        let acme = acme();
        for value in ["token-one", "token-two"] {
            acme.add("_acme-challenge.xip.test".into(), value.into())
                .await
                .unwrap();
        }
        let handler = DnsHandler {
            config: make_config(),
            acme,
        };

        let resp = run(&handler, "_acme-challenge.xip.test", RecordType::TXT).await;
        assert_eq!(resp.answers.len(), 2);
        for answer in &resp.answers {
            assert!(matches!(
                &answer.data,
                RData::TXT(txt) if txt.txt_data.len() == 1
            ));
        }
    }

    #[tokio::test]
    async fn oversized_udp_response_is_truncated() {
        let acme = AcmeRecords::new(Duration::from_secs(60), 100);
        for index in 0..20 {
            acme.add(
                "_acme-challenge.xip.test".into(),
                format!("{index:02}{}", "x".repeat(198)),
            )
            .await
            .unwrap();
        }
        let handler = DnsHandler {
            config: make_config(),
            acme,
        };

        let raw = query_msg("_acme-challenge.xip.test", RecordType::TXT);
        let response = Message::from_vec(&handler.handle_udp(&raw).await).unwrap();
        assert!(response.metadata.truncation);
        assert!(response.answers.is_empty());
    }

    #[tokio::test]
    async fn nonexistent_in_zone_name_is_nxdomain() {
        let handler = DnsHandler {
            config: make_config(),
            acme: acme(),
        };
        let resp = run(&handler, "totally-bogus.xip.test", RecordType::A).await;
        assert_eq!(resp.metadata.response_code, ResponseCode::NXDomain);
    }

    #[tokio::test]
    async fn nonexistent_name_is_nxdomain_for_unsupported_type() {
        let handler = DnsHandler {
            config: make_config(),
            acme: acme(),
        };
        let resp = run(&handler, "totally-bogus.xip.test", RecordType::CAA).await;
        assert_eq!(resp.metadata.response_code, ResponseCode::NXDomain);
    }

    #[tokio::test]
    async fn out_of_zone_name_is_refused_without_authority_claim() {
        let handler = DnsHandler {
            config: make_config(),
            acme: acme(),
        };
        let resp = run(&handler, "example.com", RecordType::A).await;
        assert_eq!(resp.metadata.response_code, ResponseCode::Refused);
        assert!(!resp.metadata.authoritative);
        assert!(resp.authorities.is_empty());
    }

    #[tokio::test]
    async fn unsupported_opcode_is_not_implemented() {
        let handler = DnsHandler {
            config: make_config(),
            acme: acme(),
        };
        let mut request = Message::new(1, MessageType::Query, OpCode::Update);
        request.add_query(Query::query(
            Name::from_ascii("xip.test").unwrap(),
            RecordType::SOA,
        ));

        let response =
            Message::from_vec(&handler.handle(&request.to_vec().unwrap()).await).unwrap();
        assert_eq!(response.metadata.response_code, ResponseCode::NotImp);
        assert!(!response.metadata.authoritative);
    }

    #[tokio::test]
    async fn unsupported_edns_version_returns_badvers() {
        let handler = DnsHandler {
            config: make_config(),
            acme: acme(),
        };
        let mut request = Message::new(1, MessageType::Query, OpCode::Query);
        request.add_query(Query::query(
            Name::from_ascii("xip.test").unwrap(),
            RecordType::SOA,
        ));
        let mut edns = Edns::new();
        edns.set_version(1);
        request.set_edns(edns);

        let response =
            Message::from_vec(&handler.handle(&request.to_vec().unwrap()).await).unwrap();
        // BADVERS and BADSIG both have numeric RCODE 16. Hickory decodes the
        // shared wire value as BADSIG, so assert the protocol value directly.
        assert_eq!(u16::from(response.metadata.response_code), 16);
        assert_eq!(response.edns.as_ref().unwrap().version(), 0);
        assert!(!response.metadata.authoritative);
    }

    #[tokio::test]
    async fn xip_ip_name_has_a_record() {
        let handler = DnsHandler {
            config: make_config(),
            acme: acme(),
        };
        let resp = run(&handler, "127-0-0-1.xip.test", RecordType::A).await;
        assert_eq!(resp.metadata.response_code, ResponseCode::NoError);
        assert_eq!(resp.answers.len(), 1);
    }

    // Regression for the 0x20 case-randomization bug (RFC 4343): Google and
    // other resolvers send mixed-case query names (e.g. `_aCMe-cHAlLeNGe...`)
    // for cache-poisoning mitigation. Comparisons must be case-insensitive,
    // otherwise these queries were rejected as "not our domain" (NXDOMAIN)
    // before any record logic ran — which is what broke Let's Encrypt.
    #[tokio::test]
    async fn acme_challenge_a_with_0x20_case_is_nodata_not_nxdomain() {
        let handler = DnsHandler {
            config: make_config(),
            acme: acme(),
        };
        let resp = run(&handler, "_aCMe-cHAlLeNGe.xIp.tEsT", RecordType::A).await;
        assert_ne!(
            resp.metadata.response_code,
            ResponseCode::NXDomain,
            "mixed-case (0x20) query must be treated as in-zone (RFC 4343)"
        );
        assert_eq!(resp.metadata.response_code, ResponseCode::NoError);
    }

    #[tokio::test]
    async fn acme_challenge_txt_with_0x20_case_and_token_returns_answer() {
        let acme = acme();
        acme.add("_acme-challenge.xip.test".to_string(), "tok".to_string())
            .await
            .unwrap();
        let handler = DnsHandler {
            config: make_config(),
            acme,
        };
        let resp = run(&handler, "_aCMe-cHAlLeNGe.xIp.tEsT", RecordType::TXT).await;
        assert_eq!(resp.metadata.response_code, ResponseCode::NoError);
        assert_eq!(resp.answers.len(), 1);
    }

    #[tokio::test]
    async fn xip_ip_name_with_0x20_case_has_a_record() {
        let handler = DnsHandler {
            config: make_config(),
            acme: acme(),
        };
        let resp = run(&handler, "127-0-0-1.xIp.TeSt", RecordType::A).await;
        assert_eq!(resp.metadata.response_code, ResponseCode::NoError);
        assert_eq!(resp.answers.len(), 1);
    }
}
