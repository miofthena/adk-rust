//! Web server for the live-translation app — a **server-side bridge** (same shape
//! as the "Mindfulness with Mia" example, but for speech translation).
//!
//! ```text
//!   browser ──mic PCM16 (base64 over WS)──▶  Rust /ws handler ──▶ provider
//!   browser ◀─translated PCM16 + transcripts─  (OpenAI translate | Gemini translate)
//! ```
//!
//! The browser is a thin audio device: it streams microphone PCM up and plays the
//! translated PCM the server streams back, while showing the source and target
//! transcripts. The provider + target language are chosen per session
//! (`/ws?provider=openai|gemini&target=es`). Audio rates differ per provider
//! (OpenAI 24 kHz in/out; Gemini 16 kHz in / 24 kHz out), negotiated to the
//! browser in a `ready` message before any audio flows.

use std::collections::HashMap;

use axum::{
    Router,
    extract::Query,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::{Html, IntoResponse},
    routing::get,
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;
use tracing::info;

use crate::translate::{self, Provider, XlatEvent};

/// Run the Axum web server.
pub async fn run_server(port: u16) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/", get(serve_index))
        .route("/ws", get(ws_handler))
        .layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    info!("listening on 0.0.0.0:{port}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn serve_index() -> impl IntoResponse {
    Html(include_str!("../assets/index.html"))
}

/// Messages the browser sends up the WebSocket.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClientMsg {
    /// A chunk of microphone audio (base64-encoded PCM16 mono).
    #[serde(rename = "input_audio")]
    InputAudio { audio: String },
    /// The user ended the session.
    #[serde(rename = "hangup")]
    Hangup,
}

/// Upgrade `/ws?provider=openai|gemini&target=<lang>` to a translation bridge.
async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let provider = params.get("provider").map(|p| Provider::parse(p)).unwrap_or(Provider::OpenAI);
    let target = params.get("target").cloned().unwrap_or_else(|| "es".to_string());
    ws.on_upgrade(move |socket| handle_ws(socket, provider, target))
}

/// Bridge one browser session to the chosen translation provider.
async fn handle_ws(socket: WebSocket, provider: Provider, target: String) {
    let session_id = uuid::Uuid::new_v4().to_string();
    info!(session_id = %session_id, provider = provider.name(), target = %target, "translation session starting");

    let (mut sender, mut receiver) = socket.split();

    // Resolve credentials/model before doing anything else.
    let Some(api_key) = translate::api_key(provider) else {
        let var = if provider == Provider::OpenAI { "OPENAI_API_KEY" } else { "GEMINI_API_KEY" };
        let _ = sender
            .send(Message::Text(
                json!({ "type": "error", "message": format!("{var} is not set") })
                    .to_string()
                    .into(),
            ))
            .await;
        return;
    };
    let model = translate::model_for(provider);

    // Tell the browser which sample rates to use before any audio flows.
    let (input_rate, output_rate) = provider.audio_rates();
    let ready = json!({
        "type": "ready",
        "provider": provider.name(),
        "model": model,
        "target": target,
        "input_rate": input_rate,
        "output_rate": output_rate,
    });
    if sender.send(Message::Text(ready.to_string().into())).await.is_err() {
        return;
    }

    // Channels: browser mic frames → provider; provider events → browser.
    let (audio_tx, audio_rx) = mpsc::channel::<String>(256);
    let (evt_tx, mut evt_rx) = mpsc::channel::<XlatEvent>(256);

    // The provider translation client runs as its own task.
    let provider_task =
        tokio::spawn(translate::run(provider, api_key, model, target, audio_rx, evt_tx));

    // Outbound: provider events → browser JSON. Owns the WS sink.
    let outbound = async move {
        while let Some(event) = evt_rx.recv().await {
            let payload = match event {
                XlatEvent::Audio(audio) => json!({ "type": "audio", "audio": audio }),
                XlatEvent::Source(delta) => json!({ "type": "source", "delta": delta }),
                XlatEvent::Target(delta) => json!({ "type": "target", "delta": delta }),
                XlatEvent::Info(message) => json!({ "type": "info", "message": message }),
                XlatEvent::Error(message) => json!({ "type": "error", "message": message }),
            };
            if sender.send(Message::Text(payload.to_string().into())).await.is_err() {
                break; // browser went away
            }
        }
    };

    // Inbound: browser mic audio → provider.
    let inbound = async move {
        while let Some(Ok(frame)) = receiver.next().await {
            match frame {
                Message::Text(text) => match serde_json::from_str::<ClientMsg>(&text) {
                    Ok(ClientMsg::InputAudio { audio }) => {
                        if audio_tx.send(audio).await.is_err() {
                            break; // provider task ended
                        }
                    }
                    Ok(ClientMsg::Hangup) => break,
                    Err(_) => {}
                },
                Message::Close(_) => break,
                _ => {}
            }
        }
        // Dropping audio_tx signals the provider client to flush and close.
    };

    tokio::select! {
        _ = outbound => info!(session_id = %session_id, "provider stream ended"),
        _ = inbound => info!(session_id = %session_id, "browser disconnected"),
    }

    provider_task.abort();
    info!(session_id = %session_id, "translation session closed");
}
