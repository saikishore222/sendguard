mod rpc;
mod scan;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};
use tower_http::cors::CorsLayer;

const CACHE_FOR: Duration = Duration::from_secs(600);

struct App {
    rpc: rpc::Rpc,
    max_scan: usize,
    default_scan: usize,
    cluster: String,
    scans: Semaphore,
    cache: Mutex<HashMap<(String, usize), (Instant, Value)>>,
}

#[derive(Deserialize)]
struct Options {
    limit: Option<usize>,
}

type Failure = (StatusCode, Json<Value>);

fn failure(status: StatusCode, message: impl Into<String>) -> Failure {
    (status, Json(json!({ "error": message.into() })))
}

async fn check(
    State(app): State<Arc<App>>,
    Path(address): Path<String>,
    Query(options): Query<Options>,
) -> Result<Json<Value>, Failure> {
    let valid = bs58::decode(&address)
        .into_vec()
        .is_ok_and(|bytes| bytes.len() == 32);
    if !valid {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "that isn't a valid Solana address",
        ));
    }
    let limit = options
        .limit
        .unwrap_or(app.default_scan)
        .clamp(1, app.max_scan);
    let key = (address.clone(), limit);
    if let Some((at, report)) = app.cache.lock().await.get(&key) {
        if at.elapsed() < CACHE_FOR {
            return Ok(Json(report.clone()));
        }
    }
    let Ok(_slot) = app.scans.try_acquire() else {
        return Err(failure(
            StatusCode::TOO_MANY_REQUESTS,
            "busy with other scans, try again in a minute",
        ));
    };

    let upstream = |error: anyhow::Error| failure(StatusCode::BAD_GATEWAY, error.to_string());
    let signatures = app
        .rpc
        .signatures(&address, limit)
        .await
        .map_err(upstream)?;
    let failed = signatures.iter().filter(|(_, ok)| !ok).count();
    let wanted: Vec<String> = signatures
        .into_iter()
        .filter(|(_, ok)| *ok)
        .map(|(s, _)| s)
        .collect();
    let raw = app.rpc.transactions(&wanted).await.map_err(upstream)?;
    let txs: Vec<scan::Tx> = raw
        .iter()
        .filter_map(|(signature, tx)| scan::parse(&address, signature, tx))
        .collect();
    let mut report =
        serde_json::to_value(scan::scan(&address, &txs, failed)).expect("report serializes");
    report["cluster"] = json!(app.cluster);

    let mut cache = app.cache.lock().await;
    cache.retain(|_, (at, _)| at.elapsed() < CACHE_FOR);
    cache.insert(key, (Instant::now(), report.clone()));
    Ok(Json(report))
}

#[tokio::main]
async fn main() {
    let env =
        |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_string());
    let app = Arc::new(App {
        rpc: rpc::Rpc::new(env("RPC_URL", "https://api.mainnet-beta.solana.com")),
        default_scan: env("DEFAULT_SCAN", "100")
            .parse()
            .expect("DEFAULT_SCAN is a number"),
        cluster: env("CLUSTER", "mainnet-beta"),
        max_scan: env("MAX_SCAN", "300")
            .parse()
            .expect("MAX_SCAN is a number"),
        scans: Semaphore::new(
            env("MAX_CONCURRENT_SCANS", "2")
                .parse()
                .expect("MAX_CONCURRENT_SCANS is a number"),
        ),
        cache: Mutex::new(HashMap::new()),
    });
    let router = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/check/{address}", get(check))
        .layer(CorsLayer::permissive())
        .with_state(app);
    let address = format!("0.0.0.0:{}", env("PORT", "8787"));
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .expect("port is free");
    println!("sendguard-api listening on {address}");
    axum::serve(listener, router).await.expect("server runs");
}
