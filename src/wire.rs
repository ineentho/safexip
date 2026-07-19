use hickory_proto::op::{Edns, Message, OpCode, Query};
use hickory_proto::rr::rdata::{SOA, TXT};
use hickory_proto::rr::{Name, RData, Record, RecordType};

use crate::config::Config;

const MAX_DNS_RECORDS: usize = u16::MAX as usize;

#[derive(Clone)]
pub struct AcmeWireCapacity {
    acme_name: Name,
    zone: Name,
    ns_hostname: Name,
    txt_ttl: u32,
}

impl AcmeWireCapacity {
    pub fn from_config(config: &Config) -> Self {
        Self {
            acme_name: Name::from_ascii(config.acme_name()).expect("validated ACME name"),
            zone: Name::from_ascii(&config.domain).expect("validated zone name"),
            ns_hostname: Name::from_ascii(&config.ns_hostname).expect("validated nameserver name"),
            txt_ttl: config.txt_ttl,
        }
    }

    pub fn fits(&self, values: &[String]) -> bool {
        self.response(values.iter().map(String::as_str))
            .is_complete(values.len())
    }

    pub fn maximum_token_count(&self) -> usize {
        let mut low = 0;
        let mut high = 1;
        while high < MAX_DNS_RECORDS && self.fits_count(high) {
            low = high;
            high = (high * 2).min(MAX_DNS_RECORDS);
        }

        while low + 1 < high {
            let middle = low + (high - low) / 2;
            if self.fits_count(middle) {
                low = middle;
            } else {
                high = middle;
            }
        }
        if high == MAX_DNS_RECORDS && self.fits_count(high) {
            high
        } else {
            low
        }
    }

    fn fits_count(&self, count: usize) -> bool {
        self.response(std::iter::repeat_n("x", count))
            .is_complete(count)
    }

    fn response<'a>(&self, values: impl IntoIterator<Item = &'a str>) -> Message {
        let mut response = Message::response(0, OpCode::Query);
        response.metadata.authoritative = true;
        response.add_query(Query::query(self.acme_name.clone(), RecordType::TXT));
        for value in values {
            response.add_answer(txt_record(
                self.acme_name.clone(),
                self.txt_ttl,
                value.to_owned(),
            ));
        }
        response.add_authority(soa_record(&self.zone, &self.ns_hostname, 0));
        let mut response_edns = Edns::new();
        response_edns.set_max_payload(1232);
        response.set_edns(response_edns);
        response
    }
}

trait CompleteMessage {
    fn is_complete(&self, expected_answers: usize) -> bool;
}

impl CompleteMessage for Message {
    fn is_complete(&self, expected_answers: usize) -> bool {
        let Ok(encoded) = self.to_vec() else {
            return false;
        };
        let Ok(decoded) = Message::from_vec(&encoded) else {
            return false;
        };
        !decoded.metadata.truncation && decoded.answers.len() == expected_answers
    }
}

pub fn txt_record(name: Name, ttl: u32, value: String) -> Record {
    Record::from_rdata(name, ttl, RData::TXT(TXT::new(vec![value])))
}

pub fn soa_record(zone: &Name, ns_hostname: &Name, serial: u32) -> Record {
    let rname = Name::from_ascii(format!("admin.{}", zone.to_ascii()))
        .expect("validated SOA responsible name");
    let soa = SOA::new(ns_hostname.clone(), rname, serial, 3600, 900, 86400, 10);
    Record::from_rdata(zone.clone(), 10, RData::SOA(soa))
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    fn config() -> Config {
        Config {
            domain: "xip.test".into(),
            ns_hostname: "ns1.xip.test".into(),
            ns_hostname2: "ns2.xip.test".into(),
            dns_bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
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
    fn includes_record_and_message_overhead_at_the_boundary() {
        let capacity = AcmeWireCapacity::from_config(&config());
        let mut values = Vec::new();
        while capacity.fits(&values) {
            values.push(format!("{:04}{}", values.len(), "x".repeat(251)));
        }
        let rejected = values.pop().unwrap();

        assert!(capacity.fits(&values));
        values.push(rejected);
        assert!(!capacity.fits(&values));
    }
}
