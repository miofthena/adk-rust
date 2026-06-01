//! Webhook handlers for external event delivery.
//!
//! This module provides webhook endpoint handlers for receiving
//! signed event notifications from external services.

#[cfg(feature = "openai-webhooks")]
pub mod openai;

#[cfg(feature = "openai-webhooks")]
pub use openai::{OpenAIWebhookConfig, OpenAIWebhookHandler, WebhookEvent};
