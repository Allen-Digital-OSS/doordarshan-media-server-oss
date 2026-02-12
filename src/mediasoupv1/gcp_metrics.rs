/*use std::fmt::format;
use std::sync::Arc;
use std::time::{Duration, Instant};
use chrono::Utc;
use log::{debug, error, info};
use opentelemetry::Value;
use reqwest::Client;
use serde::{Deserialize, Serialize, Serializer};
use serde::ser::SerializeMap;
use tokio::sync::Mutex;
use crate::InstanceMeta;

const METADATA_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";

#[derive(Debug, Deserialize)]
struct GcpTokenResponse {
    pub access_token: String,
    pub expires_in: u64,
}

#[derive(Debug, Serialize)]
struct GcpMetricRequest {
    pub time_series: Vec<TimeSeries>,
}

#[derive(Debug, Serialize)]
struct TimeSeries {
    pub metric: Metric,
    pub resource: Resource,
    pub points: Vec<Point>,
}

#[derive(Debug, Serialize)]
struct Metric {
    #[serde(rename = "type")]
    pub metric_type: String,
    pub labels: MetricLabels,
}

#[derive(Debug, Serialize)]
struct Resource {
    #[serde(rename = "type")]
    pub resource_type: String,
    pub labels: ResourceLabels,
}

#[derive(Debug, Serialize)]
struct ResourceLabels {
    pub instance_id: String,
    pub zone: String,
}

#[derive(Debug, Serialize)]
struct MetricLabels {
    pub instance_id: String,
    pub zone: String,
    pub instance_group: String,
}

#[derive(Serialize, Debug)]
struct PointValue {
    double_value: f64,
}

#[derive(Debug, Serialize)]
struct Point {
    pub interval: Interval,
    pub value: PointValue,
}

#[derive(Debug, Serialize)]
struct Interval {
    pub end_time: String,
}

pub struct TokenCache {
    pub token: String,
    pub expiry: Instant,
}
pub struct GcpMetrics {}

impl GcpMetrics {
    pub async fn fetch_gcp_token(client: &Client, cache: Arc<Mutex<Option<TokenCache>>>) -> Result<String, String> {
        let mut cache_lock = cache.lock().await;
        debug!("Fetching GCP token...");
        // Check if token is still valid
        if let Some(cache) = cache_lock.as_ref() {
            if Instant::now() < cache.expiry {
                return Ok(cache.token.clone());
            }
        }
        info!("Token expired or not found, fetching a new one...");
        // Fetch new token
        let response = match client
            .get(METADATA_URL)
            .header("Metadata-Flavor", "Google")
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(err) => {
                error!("Failed to fetch token: {}", err);
                return Err("Failed to fetch token".to_string());
            }
        };
        info!("Response from GCP: {:?}", response);
        let token_response: GcpTokenResponse = match response.json().await {
            Ok(data) => data,
            Err(err) => {
                error!("Failed to parse token response: {}", err);
                return Err("Failed to parse token response".to_string());
            }
        };
        info!("Token response: {:?}", token_response);
        let new_token = token_response.access_token.clone();
        let expiry_time = Instant::now() + Duration::from_secs(token_response.expires_in);

        // Store new token in cache
        *cache_lock = Some(TokenCache {
            token: new_token.clone(),
            expiry: expiry_time,
        });

        Ok(new_token)
    }

    pub async fn send_metric(client: &Client, cache: Arc<Mutex<Option<TokenCache>>>, metric_value: f64,
                             instance_meta: InstanceMeta) {
        let token = match Self::fetch_gcp_token(client, cache.clone()).await {
            Ok(token) => token,
            Err(err) => {
                error!("Failed to fetch token: {:?}", err);
                return;
            }
        };
        debug!("Token fetched successfully: {}", token);
        let metric = GcpMetricRequest {
            time_series: vec![TimeSeries {
                metric: Metric {
                    metric_type: "custom.googleapis.com/doordarshan/current_capacity".to_string(),
                    labels: MetricLabels {
                        instance_id: instance_meta.instance_id.clone(),
                        zone: instance_meta.zone.clone(),
                        instance_group: instance_meta.managed_instance_group_name.clone()
                    },
                },
                resource: Resource {
                    resource_type: "gce_instance".to_string(),
                    labels: ResourceLabels {
                        instance_id: instance_meta.instance_id.clone(),
                        zone: instance_meta.zone.clone()
                    },
                },
                points: vec![Point {
                    interval: Interval {
                        end_time: Utc::now().to_rfc3339(),
                    },
                    value: PointValue {
                        double_value: metric_value,
                    },
                }],
            }],
        };
        debug!("Metric to be sent: {:?}", metric);
        debug!("Metric Payload {:?}", serde_json::to_string(&metric));
        let res = client
            .post(format!("https://monitoring.googleapis.com/v3/projects/{}/timeSeries", instance_meta.gcp_project_id))
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .json(&metric)
            .send()
            .await;
        debug!("Response from GCP: {:?}", res);
        match res {
            Ok(resp) => {
                //info!("Metric sent: {:?}", resp.status())
            },
            Err(err) => error!("Failed to send metric: {:?}", err),
        }
    }
}*/
