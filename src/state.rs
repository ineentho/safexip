use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::wire::AcmeWireCapacity;

#[derive(Clone)]
pub struct AcmeRecords {
    records: Arc<RwLock<HashMap<String, Vec<Token>>>>,
    lifetime: Duration,
    max_tokens: usize,
    wire_capacity: AcmeWireCapacity,
}

#[derive(Clone)]
struct Token {
    value: String,
    expires_at: Instant,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AddError {
    TokenLimitReached,
    DnsWireCapacityReached,
}

impl AcmeRecords {
    pub fn new(lifetime: Duration, max_tokens: usize, wire_capacity: AcmeWireCapacity) -> Self {
        Self {
            records: Arc::new(RwLock::new(HashMap::new())),
            lifetime,
            max_tokens,
            wire_capacity,
        }
    }

    pub async fn add(&self, name: String, value: String) -> Result<(), AddError> {
        let mut map = self.records.write().await;
        prune(&mut map);

        if let Some(existing) = map
            .get_mut(&name)
            .and_then(|tokens| tokens.iter_mut().find(|token| token.value == value))
        {
            existing.expires_at = Instant::now() + self.lifetime;
            return Ok(());
        }

        let token_count: usize = map.values().map(Vec::len).sum();
        if token_count >= self.max_tokens {
            return Err(AddError::TokenLimitReached);
        }

        let mut prospective_values: Vec<String> = map
            .values()
            .flatten()
            .map(|token| token.value.clone())
            .collect();
        prospective_values.push(value.clone());
        if !self.wire_capacity.fits(&prospective_values) {
            return Err(AddError::DnsWireCapacityReached);
        }

        map.entry(name).or_default().push(Token {
            value,
            expires_at: Instant::now() + self.lifetime,
        });
        Ok(())
    }

    pub async fn remove(&self, name: &str, value: &str) {
        let mut map = self.records.write().await;
        if let Some(values) = map.get_mut(name) {
            values.retain(|token| token.value != value);
            if values.is_empty() {
                map.remove(name);
            }
        }
        prune(&mut map);
    }

    pub async fn get(&self, name: &str) -> Vec<String> {
        let mut map = self.records.write().await;
        prune(&mut map);
        map.get(name)
            .into_iter()
            .flatten()
            .map(|token| token.value.clone())
            .collect()
    }
}

fn prune(records: &mut HashMap<String, Vec<Token>>) {
    let now = Instant::now();
    records.retain(|_, tokens| {
        tokens.retain(|token| token.expires_at > now);
        !tokens.is_empty()
    });
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;
    use crate::config::Config;

    fn capacity() -> AcmeWireCapacity {
        AcmeWireCapacity::from_config(&Config {
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
        })
    }

    fn records(lifetime: Duration, max_tokens: usize) -> AcmeRecords {
        AcmeRecords::new(lifetime, max_tokens, capacity())
    }

    #[tokio::test]
    async fn deduplicates_and_refreshes_tokens() {
        let records = records(Duration::from_secs(60), 1);
        records.add("name".into(), "value".into()).await.unwrap();
        records.add("name".into(), "value".into()).await.unwrap();
        assert_eq!(records.get("name").await, ["value"]);
    }

    #[tokio::test]
    async fn enforces_capacity() {
        let records = records(Duration::from_secs(60), 1);
        records.add("name".into(), "one".into()).await.unwrap();
        assert_eq!(
            records.add("name".into(), "two".into()).await,
            Err(AddError::TokenLimitReached)
        );
    }

    #[tokio::test]
    async fn expires_tokens() {
        let records = records(Duration::from_millis(10), 1);
        records.add("name".into(), "value".into()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(records.get("name").await.is_empty());
    }

    #[tokio::test]
    async fn rejects_wire_overflow_without_changing_existing_records() {
        let records = records(Duration::from_secs(60), usize::MAX);
        let name = "_acme-challenge.xip.test";
        let mut accepted = Vec::new();
        loop {
            let value = format!("{:04}{}", accepted.len(), "x".repeat(251));
            match records.add(name.into(), value.clone()).await {
                Ok(()) => accepted.push(value),
                Err(AddError::DnsWireCapacityReached) => break,
                Err(error) => panic!("unexpected error: {error:?}"),
            }
        }

        assert!(!accepted.is_empty());
        assert_eq!(records.get(name).await, accepted);
    }

    #[tokio::test]
    async fn concurrent_additions_cannot_exceed_wire_capacity() {
        let records = records(Duration::from_secs(60), usize::MAX);
        let mut tasks = Vec::new();
        for index in 0..400 {
            let records = records.clone();
            tasks.push(tokio::spawn(async move {
                let value = format!("{index:04}{}", "x".repeat(251));
                records.add("_acme-challenge.xip.test".into(), value).await
            }));
        }
        for task in tasks {
            let _ = task.await.unwrap();
        }

        let values = records.get("_acme-challenge.xip.test").await;
        assert!(capacity().fits(&values));
        let mut overflow = values.clone();
        overflow.push("z".repeat(255));
        assert!(!capacity().fits(&overflow));
    }
}
