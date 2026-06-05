use adk_core::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;
use tokio::sync::mpsc;

use super::event_source::{EventSource, TriggerEvent};

/// Emits trigger events when HTTP POST requests arrive on the configured path.
///
/// Spawns a minimal `axum` HTTP listener on the configured port. Each POST body
/// becomes a [`TriggerEvent`] payload.
///
/// # Example
///
/// ```rust,ignore
/// use adk_agent::ambient::WebhookTrigger;
///
/// let trigger = WebhookTrigger::new(8080, "/webhook");
/// ```
pub struct WebhookTrigger {
    port: u16,
    path: String,
    name: String,
}

impl WebhookTrigger {
    /// Create a webhook trigger listening on the given port and path.
    ///
    /// The path should start with `/`.
    pub fn new(port: u16, path: &str) -> Self {
        let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };

        Self { port, name: format!("webhook:{path}"), path }
    }
}

#[async_trait]
impl EventSource for WebhookTrigger {
    fn name(&self) -> &str {
        &self.name
    }

    async fn subscribe(&self) -> Result<BoxStream<'static, TriggerEvent>> {
        let (tx, mut rx) = mpsc::channel::<TriggerEvent>(256);
        let source_name = self.name.clone();
        let path = self.path.clone();
        let port = self.port;

        // Spawn the HTTP listener in the background
        tokio::spawn(async move {
            use axum::Router;
            use axum::body::Bytes;
            use axum::routing::post;

            let tx_clone = tx.clone();
            let source_for_handler = source_name.clone();

            let app = Router::new().route(
                &path,
                post(move |body: Bytes| {
                    let tx = tx_clone.clone();
                    let source = source_for_handler.clone();
                    async move {
                        let payload = match serde_json::from_slice::<serde_json::Value>(&body) {
                            Ok(v) => v,
                            Err(_) => {
                                // If not valid JSON, wrap as a string
                                serde_json::Value::String(
                                    String::from_utf8_lossy(&body).to_string(),
                                )
                            }
                        };

                        let event = TriggerEvent { source, payload };

                        if tx.send(event).await.is_err() {
                            tracing::debug!("webhook subscriber dropped, stopping listener");
                        }

                        axum::http::StatusCode::OK
                    }
                }),
            );

            let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
            let listener = match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!("webhook trigger failed to bind on port {port}: {e}");
                    return;
                }
            };

            tracing::info!("webhook trigger listening on {addr}{path}");

            if let Err(e) = axum::serve(listener, app).await {
                tracing::warn!("webhook trigger server error: {e}");
            }
        });

        // Convert the mpsc receiver into a stream
        let stream = async_stream::stream! {
            while let Some(event) = rx.recv().await {
                yield event;
            }
        };

        Ok(Box::pin(stream))
    }
}

impl std::fmt::Debug for WebhookTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookTrigger")
            .field("port", &self.port)
            .field("path", &self.path)
            .finish()
    }
}
