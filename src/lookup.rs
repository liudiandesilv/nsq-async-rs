use crate::error::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct LookupResponse {
    producers: Vec<Producer>,
}

#[derive(Debug, Deserialize)]
struct Producer {
    broadcast_address: String,
    tcp_port: u16,
}

pub async fn lookup_nodes(lookupd_url: &str, topic: &str) -> Result<Vec<String>> {
    let url = format!("{}/lookup?topic={}", lookupd_url, topic);

    let response = reqwest::get(&url).await?;
    let data: LookupResponse = response.json().await?;

    Ok(data
        .producers
        .into_iter()
        .map(|p| {
            let host = if p.broadcast_address == "nsqd" {
                "127.0.0.1".to_string()
            } else {
                p.broadcast_address
            };
            format!("{}:{}", host, p.tcp_port)
        })
        .collect())
}
