// src/client/mod.rs
//! Client module for sending analytic user events to a remote client-config service.
//!
//! Responsibilities:
//! - Read client configuration (endpoint and headers) from environment.
//! - Build a JSON event payload from a `crate::common::UserEvent`.
//! - Send the payload via an HTTP POST request and log success or failure.
//!
//! Environment variables:
//! - `CLIENT_CONFIG_SERVICE_ENDPOINT` (required): the service URL to POST events to.
//! - `CLIENT_CONFIG_SERVICE_HEADERS` (optional): JSON object of header name -> value,
//!   e.g. `{"Authorization":"Bearer ...", "X-Custom":"value"}`.
//!
//! Notes:
//! - Headers JSON must parse into a `HashMap<String, String>`. Malformed JSON will result
//!   in `ClientConfigDetails::from_env` panicking due to `unwrap()` on `raw` in this code.
//! - `AnalyticMetricClient::send_event` is async and returns a `reqwest::Error` on transport
//!   failures.

use crate::common;
use crate::common::UserEvent;
use chrono::Utc;
use http::{HeaderMap, HeaderName, HeaderValue};
use log::{error, info};
use reqwest::{header::USER_AGENT, Client, Error};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;

/// Client configuration details used to communicate with the client-config service.
///
/// Fields:
/// - `client_config_endpoint`: full URL of the service to POST events to.
/// - `headers`: additional HTTP headers to include with requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientConfigDetails {
    pub client_config_endpoint: String,
    pub headers: HashMap<String, String>,
}

impl ClientConfigDetails {
    /// Build a `ClientConfigDetails` from environment variables.
    ///
    /// Reads `CLIENT_CONFIG_SERVICE_HEADERS` which should be a JSON object and
    /// `CLIENT_CONFIG_SERVICE_ENDPOINT` for the destination URL.
    ///
    /// # Panics
    ///
    /// - If `CLIENT_CONFIG_SERVICE_ENDPOINT` is not a valid string for the endpoint,
    ///   `.expect("Invalid CLIENT_CONFIG_SERVICE_ENDPOINT")` will panic.
    /// - If `CLIENT_CONFIG_SERVICE_HEADERS` is present but not valid JSON, the
    ///   `from_str(...).unwrap_or_default()` uses `unwrap()` on the `raw` `Option`
    ///   and may panic; consider making parsing fallible in production code.
    pub fn from_env() -> Self {
        let raw = env::var("CLIENT_CONFIG_SERVICE_HEADERS").ok();
        let headers: HashMap<String, String> =
            serde_json::from_str(&raw.unwrap()).unwrap_or_default();
        Self {
            client_config_endpoint: common::get_env("CLIENT_CONFIG_SERVICE_ENDPOINT", "")
                .parse()
                .expect("Invalid CLIENT_CONFIG_SERVICE_ENDPOINT"),
            headers,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct EventPayload {
    name: String,
    clientReferenceId: Option<String>,
    eventTimestamp: u64,
    event: serde_json::Value,
}

/// Client responsible for sending analytic metrics/events.
///
/// Example usage:
/// ```no_run
/// # async fn example(client_cfg: crate::client::ClientConfigDetails, user_event: crate::common::UserEvent) {
/// let client = crate::client::AnalyticMetricClient::new(client_cfg).await;
/// client.send_event("user.action", user_event).await.unwrap();
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct AnalyticMetricClient {
    config: ClientConfigDetails,
}

impl AnalyticMetricClient {
    /// Create a new `AnalyticMetricClient` from a `ClientConfigDetails`.
    ///
    /// This is async to mirror possible future needs for async initialization.
    pub async fn new(client_config: ClientConfigDetails) -> Self {
        Self {
            config: client_config.clone(),
        }
    }

    /// Send an analytic event to the configured endpoint.
    ///
    /// - `event_name`: logical name of the event (e.g. `"meeting.started"`).
    /// - `user_event`: event payload data sourced from `crate::common::UserEvent`.
    ///
    /// The method:
    /// 1. Builds an `EventPayload` including a `clientReferenceId` and `eventTimestamp`.
    /// 2. Converts configured headers into an `http::HeaderMap`.
    /// 3. Issues a POST request with the JSON payload.
    /// 4. Logs success (`info!`) or detailed failure (`error!`) including response body.
    ///
    /// # Errors
    ///
    /// Returns `reqwest::Error` for HTTP/send related failures.
    pub async fn send_event(&self, event_name: &str, user_event: UserEvent) -> Result<(), Error> {
        let client = Client::new();

        let current_time = Utc::now().timestamp_millis();
        // Construct the event payload
        let event_payload = EventPayload {
            name: event_name.clone().to_string(),
            clientReferenceId: Option::from(current_time.clone().to_string()),
            eventTimestamp: current_time.clone() as u64,
            event: serde_json::json!({
                "event_id": user_event.event_id,
                "user_id": user_event.user_id,
                "event_type": user_event.event_type,
                "event_time": user_event.event_time,
                "instance_id": user_event.instance_id,
                "meeting_id": user_event.meeting_id,
            }),
        };

        // Send the POST request
        let url = self.config.client_config_endpoint.clone();
        let headers = self.config.headers.clone();
        let mut header_map = HeaderMap::new();
        for (key, value) in headers {
            header_map.insert(
                HeaderName::from_bytes(key.as_bytes()).unwrap(),
                HeaderValue::from_str(&value).unwrap(),
            );
        }
        // Optional: ensure a User-Agent is present
        header_map.insert(
            USER_AGENT,
            HeaderValue::from_str("analytic-metric-client/1.0").unwrap(),
        );

        let response = client
            .post(&url)
            .headers(header_map)
            .json(&event_payload)
            .send()
            .await?;

        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<Failed to read body>".to_string());

        if status.is_success() {
            info!("Event sent successfully.");
        } else {
            error!(
                "Failed to send event: {} HTTP status: {} url: {} request: {:?}",
                body, status, url, event_payload
            );
        }
        Ok(())
    }
}
