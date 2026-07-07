use std::net::Ipv4Addr;

use hickory_proto::op::{Message, MessageType, ResponseCode};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use hickory_proto::rr::rdata::{A, NS, SOA, TXT};

use crate::config::Config;
use crate::state::AcmeRecords;
use crate::xip;

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

    async fn build_response(&self, request: &Message) -> Message {
        let mut response = Message::new();
        response.set_id(request.id());
        response.set_message_type(MessageType::Response);
        response.set_authoritative(true);
        response.set_recursion_available(false);

        if request.queries().is_empty() {
            response.set_response_code(ResponseCode::FormErr);
            return response;
        }

        let query = &request.queries()[0];
        let qname = query.name();
        let qtype = query.query_type();
        // Case-fold for all comparisons (RFC 4343). The response still echoes
        // the original query name via `add_query` below, so 0x20 randomization
        // used by resolvers like Google's is preserved in the question section.
        let qname_str = qname.to_ascii().trim_end_matches('.').to_lowercase();

        tracing::debug!("DNS query: {qname_str} {:?}", qtype);

        response.add_query(query.clone());

        if !xip::is_our_domain(&qname_str, &self.config.domain) {
            response.set_response_code(ResponseCode::NXDomain);
            response.add_name_server(self.soa());
            return response;
        }

        // Always include SOA in authority for in-zone responses.
        response.add_name_server(self.soa());

        match qtype {
            RecordType::SOA => {
                response.add_answer(self.soa());
            }
            RecordType::NS => {
                let zone = Name::from_ascii(&self.config.domain).unwrap();
                let nss = [
                    &self.config.ns_hostname,
                    &self.config.ns_hostname2,
                ];
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
                    response.set_response_code(ResponseCode::NXDomain);
                }
                // else: NODATA — name exists in zone but has no A record.
                // MUST NOT return NXDOMAIN here: RFC 8020 lets validating
                // resolvers (Google, and thus Let's Encrypt) cache the whole
                // name as nonexistent, which then poisons TXT lookups at the
                // same name and breaks DNS-01 validation. SOA is already in
                // the authority section above.
            }
            RecordType::TXT => {
                if let Some(txt) = self.resolve_txt(&qname_str).await {
                    response.add_answer(txt);
                } else if !self.name_exists(&qname_str) {
                    response.set_response_code(ResponseCode::NXDomain);
                }
                // else: NODATA — name exists but no TXT record right now.
                // Per-type negative caching only (not NXDOMAIN-cut), so a
                // subsequent TXT query after the token is set still resolves.
            }
            _ => {
                // NODATA: name exists in zone but record type absent (e.g. CAA).
                // REFUSED would be treated as a DNS failure by CAs.
            }
        }

        response
    }

    fn resolve_a(&self, qname: &str) -> Option<Ipv4Addr> {
        if qname == self.config.ns_hostname || qname == self.config.ns_hostname2 || qname == self.config.domain {
            return self.config.ns_ip.parse().ok();
        }
        if let Some(ip) = xip::parse_xip_ip(qname, &self.config.domain) {
            return Some(ip);
        }
        None
    }

    async fn resolve_txt(&self, qname: &str) -> Option<Record> {
        let acme_name = format!("_acme-challenge.{}", self.config.domain);
        if qname != acme_name {
            return None;
        }

        let values = self.acme.get(&acme_name).await;
        if values.is_empty() {
            return None;
        }
        let name = Name::from_ascii(qname).ok()?;
        Some(Record::from_rdata(
            name,
            self.config.txt_ttl,
            RData::TXT(TXT::new(values)),
        ))
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
        if qname == format!("_acme-challenge.{}", self.config.domain) {
            return true;
        }
        // Any name encoding a valid xip IP octets exists and has an A record.
        if xip::parse_xip_ip(qname, &self.config.domain).is_some() {
            return true;
        }
        false
    }

    fn a_record(&self, name: Name, ip: Ipv4Addr) -> Record {
        Record::from_rdata(name, self.config.default_ttl, RData::A(A::new(
            ip.octets()[0],
            ip.octets()[1],
            ip.octets()[2],
            ip.octets()[3],
        )))
    }

    fn ns_glue(&self) -> Vec<Record> {
        let ip: Ipv4Addr = self.config.ns_ip.parse().unwrap();
        vec![
            self.a_record(Name::from_ascii(&self.config.ns_hostname).unwrap(), ip),
            self.a_record(Name::from_ascii(&self.config.ns_hostname2).unwrap(), ip),
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
            10,  // minimum TTL — governs negative caching (NXDOMAIN + NODATA). Low for fast ACME retry.
        );
        Record::from_rdata(
            Name::from_ascii(&self.config.domain).unwrap(),
            10,  // SOA record TTL — also used for negative caching by some resolvers.
            RData::SOA(soa),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::{Message, MessageType, OpCode, Query};

    fn make_config() -> Config {
        Config {
            domain: "xip.test".to_string(),
            ns_hostname: "ns1.xip.test".to_string(),
            ns_hostname2: "ns2.xip.test".to_string(),
            dns_bind: "0.0.0.0".to_string(),
            dns_port: 53,
            api_bind: "0.0.0.0".to_string(),
            api_port: 8080,
            ns_ip: "127.0.0.1".to_string(),
            api_key: "test-key".to_string(),
            txt_ttl: 60,
            default_ttl: 60,
        }
    }

    fn query_msg(name: &str, qtype: RecordType) -> Vec<u8> {
        let mut msg = Message::new();
        msg.set_id(1);
        msg.set_message_type(MessageType::Query);
        msg.set_op_code(OpCode::Query);
        msg.set_recursion_desired(true);
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
            acme: AcmeRecords::new(),
        };
        let resp = run(&handler, "_acme-challenge.xip.test", RecordType::A).await;
        assert_ne!(
            resp.response_code(),
            ResponseCode::NXDomain,
            "A query on an existing name must not be NXDOMAIN (RFC 8020 NXDOMAIN-cut)"
        );
        assert_eq!(resp.response_code(), ResponseCode::NoError);
        assert!(resp.answers().is_empty(), "expected NODATA (no A record)");
    }

    #[tokio::test]
    async fn acme_challenge_txt_no_token_is_nodata_not_nxdomain() {
        let handler = DnsHandler {
            config: make_config(),
            acme: AcmeRecords::new(),
        };
        let resp = run(&handler, "_acme-challenge.xip.test", RecordType::TXT).await;
        assert_ne!(
            resp.response_code(),
            ResponseCode::NXDomain,
            "TXT with no token must not be NXDOMAIN (would poison the name via NXDOMAIN-cut)"
        );
        assert_eq!(resp.response_code(), ResponseCode::NoError);
        assert!(resp.answers().is_empty());
    }

    #[tokio::test]
    async fn acme_challenge_txt_with_token_returns_answer() {
        let acme = AcmeRecords::new();
        acme.add("_acme-challenge.xip.test".to_string(), "token123".to_string())
            .await;
        let handler = DnsHandler {
            config: make_config(),
            acme,
        };
        let resp = run(&handler, "_acme-challenge.xip.test", RecordType::TXT).await;
        assert_eq!(resp.response_code(), ResponseCode::NoError);
        assert_eq!(resp.answers().len(), 1);
    }

    #[tokio::test]
    async fn nonexistent_in_zone_name_is_nxdomain() {
        let handler = DnsHandler {
            config: make_config(),
            acme: AcmeRecords::new(),
        };
        let resp = run(&handler, "totally-bogus.xip.test", RecordType::A).await;
        assert_eq!(resp.response_code(), ResponseCode::NXDomain);
    }

    #[tokio::test]
    async fn xip_ip_name_has_a_record() {
        let handler = DnsHandler {
            config: make_config(),
            acme: AcmeRecords::new(),
        };
        let resp = run(&handler, "127-0-0-1.xip.test", RecordType::A).await;
        assert_eq!(resp.response_code(), ResponseCode::NoError);
        assert_eq!(resp.answers().len(), 1);
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
            acme: AcmeRecords::new(),
        };
        let resp = run(&handler, "_aCMe-cHAlLeNGe.xIp.tEsT", RecordType::A).await;
        assert_ne!(
            resp.response_code(),
            ResponseCode::NXDomain,
            "mixed-case (0x20) query must be treated as in-zone (RFC 4343)"
        );
        assert_eq!(resp.response_code(), ResponseCode::NoError);
    }

    #[tokio::test]
    async fn acme_challenge_txt_with_0x20_case_and_token_returns_answer() {
        let acme = AcmeRecords::new();
        acme.add("_acme-challenge.xip.test".to_string(), "tok".to_string())
            .await;
        let handler = DnsHandler {
            config: make_config(),
            acme,
        };
        let resp = run(&handler, "_aCMe-cHAlLeNGe.xIp.tEsT", RecordType::TXT).await;
        assert_eq!(resp.response_code(), ResponseCode::NoError);
        assert_eq!(resp.answers().len(), 1);
    }

    #[tokio::test]
    async fn xip_ip_name_with_0x20_case_has_a_record() {
        let handler = DnsHandler {
            config: make_config(),
            acme: AcmeRecords::new(),
        };
        let resp = run(&handler, "127-0-0-1.xIp.TeSt", RecordType::A).await;
        assert_eq!(resp.response_code(), ResponseCode::NoError);
        assert_eq!(resp.answers().len(), 1);
    }
}
