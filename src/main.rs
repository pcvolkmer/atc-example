use askama_axum::{Response, Template};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use csv::{ReaderBuilder, StringRecord};
use itertools::Itertools;
use moka::future::Cache;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;
use strsim::jaro_winkler;
use tower_http::trace::TraceLayer;

static AGS_CSV: &str = include_str!("resources/atc.csv");

static ATC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[ABCDGHJLMNPRSV][0-2][1-9]([A-Z]([A-Z](\\d{2})?)?)?").expect("valid regex")
});

#[derive(Serialize, Deserialize, Clone)]
struct AtcCode {
    code: String,
    name: String,
    similarity: u8,
}

impl AtcCode {
    fn from_record(record: &StringRecord) -> AtcCode {
        Self {
            code: record.get(0).unwrap().to_string(),
            name: record.get(1).unwrap().to_string(),
            similarity: 0,
        }
    }

    fn with_similarity(mut self, similarity: u8) -> AtcCode {
        self.similarity = similarity;
        self
    }
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    query: String,
    entries: Vec<AtcCode>,
}

fn all_codes() -> Vec<AtcCode> {
    ReaderBuilder::new()
        .from_reader(AGS_CSV.as_bytes())
        .records()
        .filter(|record| record.is_ok())
        .map(|record| record.unwrap())
        .map(|record| AtcCode::from_record(&record))
        .collect_vec()
}

fn get_similarity(query: &str, atc_code: &AtcCode) -> u8 {
    if ATC_RE.is_match(&query.to_ascii_uppercase()) {
        if query
            .to_lowercase()
            .starts_with(&atc_code.code.to_lowercase())
        {
            return 100;
        }
        return 0;
    }

    if query.to_lowercase() == atc_code.name.to_lowercase()
        || format!(
            "{} {}",
            atc_code.code.to_lowercase(),
            atc_code.name.to_lowercase()
        )
        .starts_with(&query.to_lowercase())
    {
        return 100;
    }

    let sim_code =
        (100.0 * jaro_winkler(&query.to_lowercase(), &atc_code.code.to_lowercase())) as u8;
    let sim_name =
        (100.0 * jaro_winkler(&query.to_lowercase(), &atc_code.name.to_lowercase())) as u8;

    if sim_code > sim_name {
        return sim_code;
    }

    sim_name
}

async fn find_codes(query: &str, cache: Cache<String, Vec<AtcCode>>) -> Vec<AtcCode> {
    let query = query.trim().to_lowercase();

    if query.is_empty() {
        return vec![];
    }

    if let Some(entries) = cache.get(&query).await {
        return entries;
    }

    let entries = all_codes()
        .into_iter()
        .map(|entry| {
            let similarity = get_similarity(&query, &entry);
            entry.with_similarity(similarity)
        })
        .filter(|entry| entry.similarity >= 90)
        .sorted_by(|e1, e2| e2.similarity.cmp(&e1.similarity))
        .take(25)
        .collect::<Vec<_>>();

    cache.insert(query, entries.clone()).await;

    entries
}

async fn negotiate(
    headers: HeaderMap,
    state_cache: State<Cache<String, Vec<AtcCode>>>,
    query: Query<HashMap<String, String>>,
) -> impl IntoResponse {
    match headers.get(header::ACCEPT) {
        Some(header) => match header.to_str().unwrap_or_default() {
            "application/json" => api_search(state_cache, query).await.into_response(),
            _ => index(state_cache, query).await.into_response(),
        },
        _ => index(state_cache, query).await.into_response(),
    }
}

async fn api_search(
    State(cache): State<Cache<String, Vec<AtcCode>>>,
    query: Query<HashMap<String, String>>,
) -> Response {
    let query = query.get("q").unwrap_or(&String::new()).trim().to_string();
    Json::from(find_codes(&query, cache).await).into_response()
}

async fn index(
    State(cache): State<Cache<String, Vec<AtcCode>>>,
    query: Query<HashMap<String, String>>,
) -> IndexTemplate {
    let query = query.get("q").unwrap_or(&String::new()).to_string();
    IndexTemplate {
        query: query.trim().to_string(),
        entries: find_codes(&query, cache).await,
    }
}

#[tokio::main]
async fn main() {
    #[cfg(debug_assertions)]
    {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();
    }

    let cache: Cache<String, Vec<AtcCode>> = Cache::builder()
        .max_capacity(1000)
        .time_to_live(Duration::from_secs(30 * 60))
        .time_to_idle(Duration::from_secs(5 * 60))
        .build();

    let app = Router::new().route("/", get(negotiate)).with_state(cache);

    #[cfg(debug_assertions)]
    let app = app.layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind("[::]:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap()
}
