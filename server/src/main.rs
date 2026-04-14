use std::{
    fs,
    mem,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use axum::{
    extract::{ws::{Message, WebSocketUpgrade}, Query, State},
    http::{Method, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use claudulhu_core::{
    build_system_prompt, effective_repo, get_branches_for_repo, init_mcp_pool, init_shell_env,
    load_or_generate_keypair, read_config, resolve_api_key, run_agentic_loop, run_noise_proxy,
    run_startup_prompt, to_base32, write_config, ApiMessage, ChatEvent, Config, ContentBlock, Session,
    DEV_PUBKEY_BASE32, DEV_STATIC_PRIVATE, DEV_STATIC_PUBLIC,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc};
use tower_http::cors::{Any, CorsLayer};

// ── Noise Protocol ────────────────────────────────────────────────────────────

const NOISE_KEY_FILE: &str = "/etc/claudulhu/noise_key.bin";

// ── Wire types ────────────────────────────────────────────────────────────────

/// Frames sent from server to mobile client over WebSocket.
#[derive(serde::Serialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WsFrame {
    /// Full message history, sent once on connect.
    History  { messages: Vec<HistMsg> },
    /// User message was saved; agentic loop is starting.
    Ack,
    /// Tool being invoked (display only).
    Tool     { name: String, input: serde_json::Value },
    /// Agentic turn complete — contains the full assistant response text.
    Done     { text: String, cost_usd: f64 },
    /// Claude is asking the user a question and needs an answer.
    Question { question: String },
    /// Response ended with an error.
    Error    { message: String },
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct HistMsg {
    role: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_input: Option<serde_json::Value>,
}

/// Frames sent from mobile client to server over WebSocket.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMsg {
    Message   { text: String },
    Interrupt,
    Answer    { answer: String },
    Clear,
}

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

/// Convert internal API messages to the wire history format.
/// Text blocks → a single `user`/`assistant` HistMsg with concatenated text.
/// ToolUse blocks in an assistant message → one `tool` HistMsg per block.
/// Pure tool-result user messages (no Text content) are skipped — the
/// tool bubble on the client is driven by the preceding ToolUse block.
fn messages_to_history(messages: &[ApiMessage]) -> Vec<HistMsg> {
    let mut out = Vec::new();
    for m in messages {
        let text: String = m.content.iter()
            .filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None })
            .collect();

        if !text.is_empty() {
            out.push(HistMsg { role: m.role.clone(), text, tool_name: None, tool_input: None });
        }

        if m.role == "assistant" {
            for block in &m.content {
                if let ContentBlock::ToolUse { name, input, .. } = block {
                    out.push(HistMsg {
                        role:       "tool".to_string(),
                        text:       String::new(),
                        tool_name:  Some(name.clone()),
                        tool_input: Some(input.clone()),
                    });
                }
            }
        }
    }
    out
}

// ── App state ─────────────────────────────────────────────────────────────────

struct AppState {
    session:      Arc<Mutex<Session>>,
    loop_running: Arc<AtomicBool>,
    /// Broadcast channel for live events (Tool, Done, Error, Question).
    /// All connected clients receive every event.
    event_tx:     broadcast::Sender<WsFrame>,
}

// ── WebSocket handler ─────────────────────────────────────────────────────────

const MOBILE_SYSTEM_PROMPT_SUFFIX: &str = "\n\n\
You are being accessed from a mobile client where screen space is limited. \
Do not narrate, explain, or comment while you work. \
Perform all tool calls silently. \
Only after all work is complete, provide a single short summary of what was done and the outcome.";

async fn chat_ws_handler(
    ws:           WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        let (mut ws_sink, mut ws_stream) = socket.split();
        let (ws_tx, mut ws_rx) = mpsc::channel::<String>(256);

        // Writer task: drain ws_tx → WebSocket.
        tokio::spawn(async move {
            while let Some(json) = ws_rx.recv().await {
                if ws_sink.send(Message::Text(json)).await.is_err() { break; }
            }
        });

        // Subscribe to live events BEFORE reading history so we cannot miss
        // events emitted between the history snapshot and our first recv().
        let mut event_rx = state.event_tx.subscribe();

        // Snapshot history.  When the loop is running, strip the trailing
        // assistant message and its associated tool entries — they will arrive
        // via the live broadcast instead.
        let hist_msgs = {
            let s = state.session.lock().unwrap();
            let loop_running = state.loop_running.load(Ordering::SeqCst);
            let mut msgs = messages_to_history(&s.messages);
            if loop_running {
                while msgs.last().map(|m| m.role == "tool").unwrap_or(false) {
                    msgs.pop();
                }
                if msgs.last().map(|m| m.role == "assistant").unwrap_or(false) {
                    msgs.pop();
                }
            }
            msgs
        };
        ws_tx.send(serde_json::to_string(&WsFrame::History { messages: hist_msgs }).unwrap_or_default()).await.ok();

        // Forward live events (Tool / Done / Error / Question) to this client.
        let fwd_tx = ws_tx.clone();
        let deliver = tokio::spawn(async move {
            while let Ok(frame) = event_rx.recv().await {
                if fwd_tx.send(serde_json::to_string(&frame).unwrap_or_default()).await.is_err() {
                    break;
                }
            }
        });

        // Receive messages from client.
        while let Some(Ok(msg)) = ws_stream.next().await {
            let text = match msg {
                Message::Text(t)  => t,
                Message::Close(_) => break,
                _                 => continue,
            };
            let client_msg: ClientMsg = match serde_json::from_str(&text) {
                Ok(m)  => m,
                Err(_) => continue,
            };

            match client_msg {
                ClientMsg::Message { text } => {
                    let cfg     = read_config();
                    let api_key = match resolve_api_key() {
                        Some(k) => k,
                        None    => {
                            ws_tx.send(serde_json::to_string(&WsFrame::Error {
                                message: "no API key configured".into(),
                            }).unwrap_or_default()).await.ok();
                            continue;
                        }
                    };
                    let model = cfg.model.unwrap_or_else(|| "claude-sonnet-4-6".to_string());

                    {
                        let mut s = state.session.lock().unwrap();
                        s.aborted.store(false, Ordering::Relaxed);
                        s.messages.push(ApiMessage {
                            role:    "user".to_string(),
                            content: vec![ContentBlock::Text { text }],
                        });
                        save_messages(&s.messages);
                    }

                    if !state.loop_running.swap(true, Ordering::SeqCst) {
                        // Ack goes directly to this client only.
                        ws_tx.send(serde_json::to_string(&WsFrame::Ack).unwrap_or_default()).await.ok();

                        let session_c    = state.session.clone();
                        let loop_running = state.loop_running.clone();
                        let event_tx     = state.event_tx.clone();

                        tokio::spawn(async move {
                            let (loop_tx, mut loop_rx) = mpsc::channel::<ChatEvent>(256);
                            tokio::spawn(run_agentic_loop(
                                session_c.clone(), "main".to_string(), api_key, model, loop_tx,
                            ));

                            // Collect text across turns; emit Tool/Done/Error/Question.
                            let mut text_buf = String::new();
                            while let Some(event) = loop_rx.recv().await {
                                match event {
                                    ChatEvent::Text { text } => {
                                        text_buf.push_str(&text);
                                    }
                                    ChatEvent::ToolUse { tool, input } => {
                                        event_tx.send(WsFrame::Tool { name: tool, input }).ok();
                                    }
                                    ChatEvent::Question { question } => {
                                        event_tx.send(WsFrame::Question { question }).ok();
                                        // Don't break — loop resumes after Answer.
                                    }
                                    ChatEvent::Result { cost_usd, .. } => {
                                        event_tx.send(WsFrame::Done { text: mem::take(&mut text_buf), cost_usd }).ok();
                                        break;
                                    }
                                    ChatEvent::Interrupted { cost_usd } => {
                                        event_tx.send(WsFrame::Done { text: mem::take(&mut text_buf), cost_usd }).ok();
                                        break;
                                    }
                                    ChatEvent::Error { message } => {
                                        event_tx.send(WsFrame::Error { message }).ok();
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                            loop_running.store(false, Ordering::SeqCst);
                            save_messages(&session_c.lock().unwrap().messages);
                        });
                    } else {
                        eprintln!("[chat] warning: message received while loop already running");
                        ws_tx.send(serde_json::to_string(&WsFrame::Ack).unwrap_or_default()).await.ok();
                    }
                }

                ClientMsg::Interrupt => {
                    state.session.lock().unwrap().aborted.store(true, Ordering::Relaxed);
                }

                ClientMsg::Answer { answer } => {
                    let pq   = state.session.lock().unwrap().pending_question.clone();
                    let mut slot = pq.lock().await;
                    if let Some(sender) = slot.take() { sender.send(answer).ok(); }
                }

                ClientMsg::Clear => {
                    {
                        let mut s = state.session.lock().unwrap();
                        s.messages.clear();
                        save_messages(&s.messages);
                    }
                    ws_tx.send(serde_json::to_string(&WsFrame::History { messages: vec![] }).unwrap_or_default()).await.ok();
                }
            }
        }

        deliver.abort();
        println!("[chat] WebSocket disconnected");
    })
}

// ── HTTP handlers ─────────────────────────────────────────────────────────────

async fn health_handler() -> impl IntoResponse { (StatusCode::OK, "ok") }

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
    let is_dev = std::env::var("CLAUDULHU_DEV").as_deref() == Ok("1");

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

    // Build session from current config + persisted messages.
    let cfg  = read_config();
    let repo = effective_repo(&cfg);
    let mut system = build_system_prompt(&repo, None, None);
    system.push_str(MOBILE_SYSTEM_PROMPT_SUFFIX);
    let messages = load_messages();
    println!("[claudulhu] loaded {} message(s) from history", messages.len());

    let mcp_pool = init_mcp_pool().await;

    let (event_tx, _) = broadcast::channel(64);

    let state = Arc::new(AppState {
        session: Arc::new(Mutex::new(Session {
            messages,
            system_prompt: system,
            cwd:           repo.clone(),
            aborted:          Arc::new(AtomicBool::new(false)),
            pending_question: Arc::new(tokio::sync::Mutex::new(None)),
            mcp_pool,
        })),
        loop_running: Arc::new(AtomicBool::new(false)),
        event_tx,
    });

    // ── Startup prompt ────────────────────────────────────────────────────────
    if let Ok(prompt) = std::env::var("STARTUP_PROMPT") {
        if !prompt.trim().is_empty() {
            let api_key = resolve_api_key().expect("ANTHROPIC_API_KEY required for STARTUP_PROMPT");
            let model   = std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-opus-4-5".to_string());
            run_startup_prompt(&prompt, state.session.clone(), &api_key, &model).await;
        }
    }

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::PUT, Method::POST, Method::OPTIONS])
        .allow_headers(Any);

    let app = Router::new()
        .route("/health",      get(health_handler))
        .route("/branches",    get(get_branches_handler))
        .route("/completions", get(get_completions_handler))
        .route("/config",      get(get_config_handler).put(update_config_handler))
        .route("/chat",        get(chat_ws_handler))
        .with_state(state)
        .layer(cors);

    let addr = format!("127.0.0.1:{http_port}");
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("failed to bind HTTP port");
    println!("[claudulhu] HTTP/WebSocket on {addr} (Noise proxy on 0.0.0.0:{noise_port}, repo: {repo})");

    axum::serve(listener, app).await.unwrap();
}
