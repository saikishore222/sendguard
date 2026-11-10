use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::time::Duration;

const BATCH: usize = 8;
const ATTEMPTS: u64 = 10;

#[derive(Clone)]
pub struct Rpc {
    http: reqwest::Client,
    url: String,
}

impl Rpc {
    pub fn new(url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("http client");
        Self { http, url }
    }

    async fn post(&self, body: &Value) -> Result<Value> {
        let response = self.http.post(&self.url).json(body).send().await?;
        if response.status().as_u16() == 429 {
            bail!("rate limited");
        }
        Ok(response.error_for_status()?.json().await?)
    }

    pub async fn signatures(&self, address: &str, limit: usize) -> Result<Vec<(String, bool)>> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "getSignaturesForAddress", "params": [address, {"limit": limit}]});
        let mut last = anyhow!("no attempt");
        for attempt in 1..=ATTEMPTS {
            match self.post(&body).await {
                Ok(reply) if reply["result"].is_array() => {
                    return Ok(reply["result"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|row| {
                            Some((row["signature"].as_str()?.to_string(), row["err"].is_null()))
                        })
                        .collect());
                }
                Ok(reply) => last = anyhow!("rpc error: {}", reply["error"]["message"]),
                Err(error) => last = error,
            }
            tokio::time::sleep(Duration::from_millis(700 * attempt)).await;
        }
        Err(last)
    }

    pub async fn transactions(&self, signatures: &[String]) -> Result<Vec<(String, Value)>> {
        let mut found = Vec::with_capacity(signatures.len());
        for chunk in signatures.chunks(BATCH) {
            let mut waiting: Vec<&String> = chunk.iter().collect();
            for attempt in 1..=ATTEMPTS {
                let body: Vec<Value> = waiting
                    .iter()
                    .enumerate()
                    .map(|(id, signature)| json!({"jsonrpc": "2.0", "id": id, "method": "getTransaction", "params": [signature, {"encoding": "jsonParsed", "maxSupportedTransactionVersion": 0}]}))
                    .collect();
                let retry: Vec<&String> = match self.post(&Value::Array(body)).await {
                    Ok(Value::Array(replies)) => {
                        let mut answered = vec![false; waiting.len()];
                        for reply in replies {
                            let Some(id) = reply["id"]
                                .as_u64()
                                .map(|id| id as usize)
                                .filter(|id| *id < waiting.len())
                            else {
                                continue;
                            };
                            if reply["error"].is_null() {
                                answered[id] = true;
                                if !reply["result"].is_null() {
                                    found.push((waiting[id].clone(), reply["result"].clone()));
                                }
                            }
                        }
                        waiting
                            .iter()
                            .zip(&answered)
                            .filter(|(_, done)| !**done)
                            .map(|(s, _)| *s)
                            .collect()
                    }
                    _ => waiting.clone(),
                };
                if retry.is_empty() {
                    break;
                }
                if attempt == ATTEMPTS {
                    bail!("the RPC kept rate limiting, try again in a minute");
                }
                waiting = retry;
                tokio::time::sleep(Duration::from_millis((1500 * attempt).min(11_000))).await;
            }
        }
        Ok(found)
    }
}
