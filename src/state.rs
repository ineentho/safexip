use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone, Default)]
pub struct AcmeRecords {
    records: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

impl AcmeRecords {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn add(&self, name: String, value: String) {
        let mut map = self.records.write().await;
        map.entry(name).or_default().push(value);
    }

    pub async fn remove(&self, name: &str, value: &str) {
        let mut map = self.records.write().await;
        if let Some(values) = map.get_mut(name) {
            values.retain(|v| v != value);
            if values.is_empty() {
                map.remove(name);
            }
        }
    }

    pub async fn delete_all(&self, name: &str) {
        self.records.write().await.remove(name);
    }

    pub async fn get(&self, name: &str) -> Vec<String> {
        self.records.read().await.get(name).cloned().unwrap_or_default()
    }
}
