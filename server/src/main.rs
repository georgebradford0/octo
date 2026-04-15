use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::{
    extract::{Query, State},
    http::{Method, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use claudulhu_core::{
    build_system_prompt, effective_repo, get_branches_for_repo, init_shell_env,
    load_or_generate_keypair, read_config, resolve_api_key, run_noise_proxy, send_message,
    to_base32, write_config, ApiMessage, Config, ContentBlock,
    DEV_PUBKEY_BASE32, DEV_STATIC_PRIVATE, DEV_STATIC_PUBLIC,
};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};

const NOISE_KEY_FILE: &str = "/etc/claudulhu/noise_key.bin";

// ── Session persistence ───────────────────────────────────────────────────────

fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("CLAUDULHU_DATA_DIR") {
        PathBuf::from(d)
    } else {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".claudulhu")
    }
}

fn session_dir() -> PathBuf { data_dir().join("session") }

fn save_messages(messages: &[ApiMessage]) {
    let dir = session_dir();
    fs::create_dir_all(&dir).ok();
    if let Ok(json) = serde_json::to_string(messages) {
        fs::write(dir.join("messages.json"), json).ok();
    }
}

fn load_messages() -> Vec<ApiMessage> {
    fs::read_to_string(session_dir().join("messages.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
struct HistMsg {
    role: String,
    text: String,
}

fn messages_to_history(messages: &[ApiMessage]) -> Vec<HistMsg> {
    messages.iter().filter_map(|m| {
        let text: String = m.content.iter()
            .filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None })
            .collect();
        if text.is_empty() { None } else { Some(HistMsg { role: m.role.clone(), text }) }
    }).collect()
}

// ── App state ─────────────────────────────────────────────────────────────────

struct AppState {
    messages: Arc<Mutex<Vec<ApiMessage>>>,
    system:   String,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn health_handler() -> impl IntoResponse { (StatusCode::OK, "ok") }

async fn history_handler(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let msgs = messages_to_history(&state.messages.lock().unwrap());
    Json(serde_json::json!({ "messages": msgs }))
}

#[derive(Deserialize)]
struct PostMessage { text: String }

async fn message_handler(
    State(state): State<Arc<AppState>>,
    Json(body):   Json<PostMessage>,
) -> impl IntoResponse {
    let api_key = match resolve_api_key() {
        Some(k) => k,
        None    => return (StatusCode::INTERNAL_SERVER_ERROR,
                           Json(serde_json::json!({"error": "no API key configured"}))).into_response(),
    };
    let model = read_config().model.unwrap_or_else(|| "claude-sonnet-4-6".to_string());

    {
        let mut msgs = state.messages.lock().unwrap();
        msgs.push(ApiMessage {
            role:    "user".to_string(),
            content: vec![ContentBlock::Text { text: body.text }],
        });
        save_messages(&msgs);
    }

    let messages = state.messages.lock().unwrap().clone();

    match send_message(&messages, &state.system, &model, &api_key).await {
        Ok((text, cost_usd)) => {
            let mut msgs = state.messages.lock().unwrap();
            msgs.push(ApiMessage {
                role:    "assistant".to_string(),
                content: vec![ContentBlock::Text { text: text.clone() }],
            });
            save_messages(&msgs);
            (StatusCode::OK, Json(serde_json::json!({ "text": text, "cost_usd": cost_usd }))).into_response()
        }
        Err(e) => {
            // Roll back the user message so history stays clean on error.
            let mut msgs = state.messages.lock().unwrap();
            msgs.pop();
            save_messages(&msgs);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e }))).into_response()
        }
    }
}

async fn clear_handler(State(state): State<Arc<AppState>>) -> StatusCode {
    let mut msgs = state.messages.lock().unwrap();
    msgs.clear();
    save_messages(&msgs);
    StatusCode::OK
}

#[derive(Deserialize)]
struct CompletionQuery { dir_part: Option<String>, file_part: Option<String> }

async fn get_completions_handler(Query(p): Query<CompletionQuery>) -> Json<Vec<String>> {
    let cfg       = read_config();
    let repo      = effective_repo(&cfg);
    let dir_part  = p.dir_part.unwrap_or_default();
    let file_part = p.file_part.unwrap_or_default();
    let mut seen    = std::collections::HashSet::new();
    let mut results = Vec::new();
    let search_dir  = PathBuf::from(&repo).join(&dir_part);
    if let Ok(entries) = fs::read_dir(&search_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') && !file_part.starts_with('.') { continue; }
            if !name.to_lowercase().starts_with(&file_part.to_lowercase()) { continue; }
            let is_dir     = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let completion = if is_dir { format!("{dir_part}{name}/") } else { format!("{dir_part}{name}") };
            if seen.insert(completion.clone()) { results.push(completion); }
        }
    }
    results.sort();
    Json(results)
}

async fn get_branches_handler() -> impl IntoResponse {
    let cfg  = read_config();
    let repo = effective_repo(&cfg);
    match get_branches_for_repo(&repo) {
        Ok(b)  => Json(b).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn get_config_handler() -> Json<Config> { Json(read_config()) }

async fn update_config_handler(Json(patch): Json<Config>) -> StatusCode {
    let mut cfg = read_config();
    if patch.repo.is_some()    { cfg.repo    = patch.repo; }
    if patch.api_key.is_some() { cfg.api_key = patch.api_key; }
    if patch.model.is_some()   { cfg.model   = patch.model; }
    write_config(&cfg);
    StatusCode::OK
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    init_shell_env();

    let args: Vec<String> = std::env::args().collect();
    let is_dev   = std::env::var("CLAUDULHU_DEV").as_deref() == Ok("1");
    let key_file = std::env::var("NOISE_KEY_FILE").unwrap_or_else(|_| NOISE_KEY_FILE.to_string());

    if args.get(1).map(|s| s.as_str()) == Some("--print-pubkey") {
        let pubkey = if is_dev {
            DEV_PUBKEY_BASE32.to_string()
        } else {
            let (_, public) = load_or_generate_keypair(&key_file);
            to_base32(&public)
        };
        println!("{pubkey}");
        return;
    }

    let (static_private, static_public) = if is_dev {
        println!("[claudulhu] !! DEV MODE: using fixed dev keypair (CLAUDULHU_DEV=1)");
        (DEV_STATIC_PRIVATE.to_vec(), DEV_STATIC_PUBLIC.to_vec())
    } else {
        load_or_generate_keypair(&key_file)
    };

    let noise_port: u16 = std::env::var("NOISE_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(9000);
    let http_port:  u16 = 8000;
    println!("[claudulhu] Noise public key: {}", to_base32(&static_public));

    tokio::spawn(run_noise_proxy(static_private, noise_port, http_port));

    let cfg      = read_config();
    let repo     = effective_repo(&cfg);
    let system   = build_system_prompt(&repo, None, None);
    let messages = load_messages();
    println!("[claudulhu] loaded {} message(s) from history", messages.len());

    let state = Arc::new(AppState {
        messages: Arc::new(Mutex::new(messages)),
        system,
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::PUT, Method::POST, Method::OPTIONS])
        .allow_headers(Any);

    let app = Router::new()
        .route("/health",      get(health_handler))
        .route("/history",     get(history_handler))
        .route("/message",     post(message_handler))
        .route("/clear",       post(clear_handler))
        .route("/branches",    get(get_branches_handler))
        .route("/completions", get(get_completions_handler))
        .route("/config",      get(get_config_handler).put(update_config_handler))
        .with_state(state)
        .layer(cors);

    let addr = format!("127.0.0.1:{http_port}");
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("failed to bind HTTP port");
    println!("[claudulhu] HTTP on {addr} (Noise proxy on 0.0.0.0:{noise_port}, repo: {repo})");

    axum::serve(listener, app).await.unwrap();
}
