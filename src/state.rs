use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

#[derive(Clone)]
pub struct AcmeRecords {
    records: Arc<RwLock<HashMap<String, Vec<Token>>>>,
    lifetime: Duration,
    max_tokens: usize,
}

#[derive(Clone)]
struct Token {
    value: String,
    expires_at: Instant,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AddError {
    CapacityReached,
}

impl AcmeRecords {
    pub fn new(lifetime: Duration, max_tokens: usize) -> Self {
        Self {
            records: Arc::new(RwLock::new(HashMap::new())),
            lifetime,
            max_tokens,
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
            return Err(AddError::CapacityReached);
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
    use super::*;

    #[tokio::test]
    async fn deduplicates_and_refreshes_tokens() {
        let records = AcmeRecords::new(Duration::from_secs(60), 1);
        records.add("name".into(), "value".into()).await.unwrap();
        records.add("name".into(), "value".into()).await.unwrap();
        assert_eq!(records.get("name").await, ["value"]);
    }

    #[tokio::test]
    async fn enforces_capacity() {
        let records = AcmeRecords::new(Duration::from_secs(60), 1);
        records.add("name".into(), "one".into()).await.unwrap();
        assert_eq!(
            records.add("name".into(), "two".into()).await,
            Err(AddError::CapacityReached)
        );
    }

    #[tokio::test]
    async fn expires_tokens() {
        let records = AcmeRecords::new(Duration::from_millis(10), 1);
        records.add("name".into(), "value".into()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(records.get("name").await.is_empty());
    }
}
