use std::net::Ipv4Addr;

use hickory_proto::op::{Edns, Message, MessageType, OpCode, ResponseCode};
use hickory_proto::rr::rdata::{A, NS};
use hickory_proto::rr::{DNSClass, Name, RData, Record, RecordType};

use crate::config::Config;
use crate::state::AcmeRecords;
use crate::wire::{self, txt_record};
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
        if request.metadata.message_type != MessageType::Query {
            return Vec::new();
        }

        let response = self.build_response(&request).await;
        self.encode_complete_or_servfail(&request, &response)
    }

    pub async fn handle_udp(&self, raw: &[u8]) -> Vec<u8> {
        let request = match Message::from_vec(raw) {
            Ok(msg) => msg,
            Err(_) => return Vec::new(),
        };
        if request.metadata.message_type != MessageType::Query {
            return Vec::new();
        }
        let max_payload = usize::from(request.max_payload().min(MAX_UDP_PAYLOAD));
        let response = self.build_response(&request).await;
        let encoded = self.encode_complete_or_servfail(&request, &response);
        if encoded.len() <= max_payload {
            encoded
        } else {
            match response.truncate().to_vec() {
                Ok(encoded) => encoded,
                Err(error) => {
                    tracing::error!("failed to serialize truncated UDP DNS response: {error}");
                    self.servfail(&request)
                }
            }
        }
    }

    fn encode_complete_or_servfail(&self, request: &Message, response: &Message) -> Vec<u8> {
        match response.to_vec() {
            Ok(encoded) => match Message::from_vec(&encoded) {
                Ok(decoded) if !decoded.metadata.truncation => encoded,
                Ok(_) => {
                    tracing::error!("DNS response serialization dropped records");
                    self.servfail(request)
                }
                Err(error) => {
                    tracing::error!("failed to verify serialized DNS response: {error}");
                    self.servfail(request)
                }
            },
            Err(error) => {
                tracing::error!("failed to serialize DNS response: {error}");
                self.servfail(request)
            }
        }
    }

    fn servfail(&self, request: &Message) -> Vec<u8> {
        let mut response = Message::response(request.metadata.id, request.metadata.op_code);
        response.metadata.response_code = ResponseCode::ServFail;
        response.metadata.recursion_desired = request.metadata.recursion_desired;
        response.add_queries(request.queries.iter().cloned());
        match response.to_vec() {
            Ok(encoded) => encoded,
            Err(error) => {
                tracing::error!("failed to serialize SERVFAIL DNS response: {error}");
                Vec::new()
            }
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
            response.metadata.authoritative = false;
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

        // safexip serves Internet-class data only. QCLASS ANY may select that
        // data, but answering CH/HS questions with IN records is a protocol
        // violation and can confuse diagnostic clients.
        if !matches!(query.query_class(), DNSClass::IN | DNSClass::ANY) {
            response.metadata.authoritative = false;
            response.metadata.response_code = ResponseCode::NotImp;
            return response;
        }

        if !xip::is_our_domain(&qname_str, &self.config.domain) {
            response.metadata.authoritative = false;
            response.metadata.response_code = ResponseCode::Refused;
            return response;
        }

        // Read the serial and any dynamic TXT values from one state version so
        // an answer can never advertise a different zone version from its data.
        let acme_name = self.config.acme_name();
        let (serial, txt_values) = if qtype == RecordType::TXT && qname_str == acme_name {
            let (values, serial) = self.acme.get_with_serial(&qname_str).await;
            (serial, values)
        } else {
            (self.acme.serial().await, Vec::new())
        };
        let soa = self.soa(serial);

        // Always include SOA in authority for in-zone responses.
        response.add_authority(soa.clone());

        match qtype {
            RecordType::SOA => {
                if qname_str == self.config.domain {
                    response.add_answer(soa);
                } else if !self.name_exists(&qname_str) {
                    response.metadata.response_code = ResponseCode::NXDomain;
                }
            }
            RecordType::NS => {
                if qname_str == self.config.domain {
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
                } else if !self.name_exists(&qname_str) {
                    response.metadata.response_code = ResponseCode::NXDomain;
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
                let records = self.txt_records(&qname_str, txt_values);
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

    fn txt_records(&self, qname: &str, values: Vec<String>) -> Vec<Record> {
        if qname != self.config.acme_name() {
            return Vec::new();
        }

        let Ok(name) = Name::from_ascii(qname) else {
            return Vec::new();
        };
        values
            .into_iter()
            .map(|value| txt_record(name.clone(), self.config.txt_ttl, value))
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

    fn soa(&self, serial: u32) -> Record {
        wire::soa_record(
            &Name::from_ascii(&self.config.domain).unwrap(),
            &Name::from_ascii(&self.config.ns_hostname).unwrap(),
            serial,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Duration;

    use super::*;
    use crate::state::AddError;
    use crate::wire::AcmeWireCapacity;
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
        let config = make_config();
        AcmeRecords::new(
            Duration::from_secs(60),
            100,
            AcmeWireCapacity::from_config(&config),
        )
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

    fn soa_serial(records: &[Record]) -> u32 {
        records
            .iter()
            .find_map(|record| match &record.data {
                RData::SOA(soa) => Some(soa.serial),
                _ => None,
            })
            .expect("SOA record")
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
        let config = make_config();
        let acme = AcmeRecords::new(
            Duration::from_secs(60),
            100,
            AcmeWireCapacity::from_config(&config),
        );
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
        assert_eq!(response.queries.len(), 1);
        assert!(response.to_vec().unwrap().len() <= 512);
    }

    #[tokio::test]
    async fn large_rrset_is_complete_without_udp_limit() {
        let config = make_config();
        let acme = AcmeRecords::new(
            Duration::from_secs(60),
            100,
            AcmeWireCapacity::from_config(&config),
        );
        for index in 0..20 {
            acme.add(
                "_acme-challenge.xip.test".into(),
                format!("{index:02}{}", "x".repeat(198)),
            )
            .await
            .unwrap();
        }
        let handler = DnsHandler { config, acme };
        let raw = query_msg("_acme-challenge.xip.test", RecordType::TXT);
        let response = Message::from_vec(&handler.handle(&raw).await).unwrap();
        assert!(!response.metadata.truncation);
        assert_eq!(response.answers.len(), 20);
        assert!(response.answers.iter().all(|record| {
            record.name.to_ascii() == "_acme-challenge.xip.test."
                && record.dns_class == DNSClass::IN
                && record.record_type() == RecordType::TXT
        }));
    }

    #[tokio::test]
    async fn soa_and_ns_follow_authoritative_owner_semantics() {
        let handler = DnsHandler {
            config: make_config(),
            acme: AcmeRecords::with_serial(
                Duration::from_secs(60),
                100,
                AcmeWireCapacity::from_config(&make_config()),
                123,
            ),
        };
        for qtype in [RecordType::SOA, RecordType::NS] {
            let apex = run(&handler, "XiP.TeSt", qtype).await;
            assert_eq!(apex.metadata.response_code, ResponseCode::NoError);
            assert!(apex.metadata.authoritative);
            assert!(!apex.answers.is_empty());
            assert!(apex.answers.iter().all(|record| {
                record.name.to_ascii() == "xip.test."
                    && record.dns_class == DNSClass::IN
                    && record.record_type() == qtype
            }));
            assert_eq!(apex.authorities.len(), 1);
            assert_eq!(apex.authorities[0].name.to_ascii(), "xip.test.");
            assert_eq!(soa_serial(&apex.authorities), 123);
            if qtype == RecordType::SOA {
                assert_eq!(apex.answers.len(), 1);
                assert_eq!(soa_serial(&apex.answers), 123);
                assert!(apex.additionals.is_empty());
            } else {
                assert_eq!(apex.answers.len(), 2);
                assert_eq!(apex.additionals.len(), 2);
                assert!(apex.additionals.iter().all(|record| {
                    record.record_type() == RecordType::A
                        && matches!(
                            record.name.to_ascii().as_str(),
                            "ns1.xip.test." | "ns2.xip.test."
                        )
                }));
            }

            let existing = run(&handler, "127-0-0-1.xIp.TeSt", qtype).await;
            assert_eq!(existing.metadata.response_code, ResponseCode::NoError);
            assert!(existing.metadata.authoritative);
            assert!(existing.answers.is_empty());
            assert_eq!(existing.authorities.len(), 1);
            assert_eq!(existing.authorities[0].name.to_ascii(), "xip.test.");
            assert_eq!(soa_serial(&existing.authorities), 123);
            assert!(existing.additionals.is_empty());

            let nonexistent = run(&handler, "missing.xip.test", qtype).await;
            assert_eq!(nonexistent.metadata.response_code, ResponseCode::NXDomain);
            assert!(nonexistent.metadata.authoritative);
            assert!(nonexistent.answers.is_empty());
            assert_eq!(nonexistent.authorities.len(), 1);
            assert_eq!(nonexistent.authorities[0].name.to_ascii(), "xip.test.");
            assert_eq!(soa_serial(&nonexistent.authorities), 123);
            assert!(nonexistent.additionals.is_empty());

            let outside = run(&handler, "example.com", qtype).await;
            assert_eq!(outside.metadata.response_code, ResponseCode::Refused);
            assert!(!outside.metadata.authoritative);
            assert!(outside.answers.is_empty());
            assert!(outside.authorities.is_empty());
            assert!(outside.additionals.is_empty());
        }
    }

    #[tokio::test]
    async fn soa_serial_is_stable_and_tracks_dynamic_zone_changes() {
        let acme = AcmeRecords::with_serial(
            Duration::from_secs(60),
            100,
            AcmeWireCapacity::from_config(&make_config()),
            500,
        );
        let handler = DnsHandler {
            config: make_config(),
            acme: acme.clone(),
        };

        for _ in 0..2 {
            let response = run(&handler, "xip.test", RecordType::SOA).await;
            assert_eq!(soa_serial(&response.answers), 500);
            assert_eq!(soa_serial(&response.authorities), 500);
        }

        acme.add("_acme-challenge.xip.test".into(), "token".into())
            .await
            .unwrap();
        let txt = run(&handler, "_acme-challenge.xip.test", RecordType::TXT).await;
        assert_eq!(txt.answers.len(), 1);
        assert_eq!(soa_serial(&txt.authorities), 501);

        acme.remove("_acme-challenge.xip.test", "token").await;
        let response = run(&handler, "xip.test", RecordType::SOA).await;
        assert_eq!(soa_serial(&response.answers), 502);
        assert_eq!(soa_serial(&response.authorities), 502);
    }

    #[tokio::test]
    async fn unsupported_query_class_never_receives_in_records() {
        let handler = DnsHandler {
            config: make_config(),
            acme: acme(),
        };
        let mut message = Message::new(7, MessageType::Query, OpCode::Query);
        let mut query = Query::query(Name::from_ascii("xip.test").unwrap(), RecordType::SOA);
        query.set_query_class(DNSClass::CH);
        message.add_query(query);
        let response =
            Message::from_vec(&handler.handle(&message.to_vec().unwrap()).await).unwrap();
        assert_eq!(response.metadata.response_code, ResponseCode::NotImp);
        assert!(!response.metadata.authoritative);
        assert!(response.answers.is_empty());
        assert_eq!(response.queries[0].query_class(), DNSClass::CH);
    }

    #[tokio::test]
    async fn semantically_malformed_messages_return_formerr() {
        let handler = DnsHandler {
            config: make_config(),
            acme: acme(),
        };
        let request = Message::new(8, MessageType::Query, OpCode::Query);
        let response =
            Message::from_vec(&handler.handle(&request.to_vec().unwrap()).await).unwrap();
        assert_eq!(response.metadata.response_code, ResponseCode::FormErr);
        assert!(!response.metadata.authoritative);

        let response = Message::new(9, MessageType::Response, OpCode::Query);
        assert!(handler.handle(&response.to_vec().unwrap()).await.is_empty());
        assert!(handler.handle_udp(&[0, 1, 2]).await.is_empty());
    }

    #[tokio::test]
    async fn edns_payload_is_echoed_and_capped() {
        let handler = DnsHandler {
            config: make_config(),
            acme: acme(),
        };
        let mut request = Message::new(10, MessageType::Query, OpCode::Query);
        request.add_query(Query::query(
            Name::from_ascii("xip.test").unwrap(),
            RecordType::SOA,
        ));
        let mut edns = Edns::new();
        edns.set_max_payload(4096).set_dnssec_ok(true);
        request.set_edns(edns);
        let response =
            Message::from_vec(&handler.handle_udp(&request.to_vec().unwrap()).await).unwrap();
        let edns = response.edns.unwrap();
        assert_eq!(edns.max_payload(), MAX_UDP_PAYLOAD);
        assert!(edns.flags().dnssec_ok);
    }

    #[tokio::test]
    async fn udp_truncation_falls_back_to_a_complete_tcp_answer_at_capacity() {
        let config = make_config();
        let acme = AcmeRecords::new(
            Duration::from_secs(60),
            usize::MAX,
            AcmeWireCapacity::from_config(&config),
        );
        let name = "_acme-challenge.xip.test";
        let mut accepted = 0;
        loop {
            let value = format!("{accepted:04}{}", "x".repeat(251));
            match acme.add(name.into(), value).await {
                Ok(()) => accepted += 1,
                Err(AddError::DnsWireCapacityReached) => break,
                Err(error) => panic!("unexpected error: {error:?}"),
            }
        }
        let handler = DnsHandler { config, acme };
        let raw = query_msg(name, RecordType::TXT);

        let udp = Message::from_vec(&handler.handle_udp(&raw).await).unwrap();
        assert!(udp.metadata.truncation);

        let tcp_bytes = handler.handle(&raw).await;
        assert!(tcp_bytes.len() <= u16::MAX as usize);
        let tcp = Message::from_vec(&tcp_bytes).unwrap();
        assert!(!tcp.metadata.truncation);
        assert_eq!(tcp.metadata.response_code, ResponseCode::NoError);
        assert_eq!(tcp.answers.len(), accepted);
    }

    #[tokio::test]
    async fn unexpected_serialization_truncation_returns_servfail() {
        let handler = DnsHandler {
            config: make_config(),
            acme: acme(),
        };
        let raw = query_msg("_acme-challenge.xip.test", RecordType::TXT);
        let request = Message::from_vec(&raw).unwrap();
        let mut response = Message::response(request.metadata.id, OpCode::Query);
        response.add_query(request.queries[0].clone());
        let name = Name::from_ascii("_acme-challenge.xip.test").unwrap();
        for _ in 0..400 {
            response.add_answer(txt_record(name.clone(), 60, "x".repeat(255)));
        }

        let encoded = handler.encode_complete_or_servfail(&request, &response);
        let response = Message::from_vec(&encoded).unwrap();
        assert_eq!(response.metadata.response_code, ResponseCode::ServFail);
        assert!(!response.metadata.truncation);
        assert!(response.answers.is_empty());
    }

    #[tokio::test]
    async fn serialization_error_returns_servfail() {
        let handler = DnsHandler {
            config: make_config(),
            acme: acme(),
        };
        let raw = query_msg("_acme-challenge.xip.test", RecordType::TXT);
        let request = Message::from_vec(&raw).unwrap();
        let mut response = Message::response(request.metadata.id, OpCode::Query);
        response.add_query(request.queries[0].clone());
        response.add_answer(txt_record(
            Name::from_ascii("_acme-challenge.xip.test").unwrap(),
            60,
            "x".repeat(256),
        ));

        let encoded = handler.encode_complete_or_servfail(&request, &response);
        let response = Message::from_vec(&encoded).unwrap();
        assert_eq!(response.metadata.response_code, ResponseCode::ServFail);
        assert!(!response.metadata.truncation);
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
