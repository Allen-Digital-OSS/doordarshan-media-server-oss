use crate::client::{AnalyticMetricClient, ClientConfigDetails};
use crate::common::{get_request_id, set_request_id};
use crate::data::MySqlUrl;
use crate::gstreamer::Gstreamer;
use crate::mediasoup::SFUConfig;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use log::{error, info, LevelFilter};
use mysql::{OptsBuilder, Pool};
use once_cell::sync::Lazy;
use opentelemetry::metrics::{Meter, MetricsError};
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::{HttpExporterBuilder, Protocol, WithExportConfig};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::{runtime, Resource};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::error::Error;
use std::fmt::Debug;
use std::fs::File;
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, fs};
use tokio::signal as tokio_signal;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tower_http::add_extension::AddExtensionLayer;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use utoipa_swagger_ui::SwaggerUi;
use crate::service::v1;
use utoipa::OpenApi;

mod client;
mod common;
mod dao;
mod data;
mod db;
mod gstreamer;
mod mediasoup;
mod mediasoupv1;
mod service;

const ENVIRONMENT_VARIABLE_KEY_ENV: &'static str = "ENV";
const OTEL_ENDPOINT: &'static str = "OTEL_ENDPOINT";
const X_REQUEST_ID: &'static str = "X-Request-ID";

/// Environment enum to represent the environment.
/*#[derive(Debug, Clone, PartialEq)]
pub enum Environment {
    Test,
    Local,
    Development,
    Stage,
    Production,
    StageMN,
    StageLT,
    ProductionMN,
    ProductionLT,
}*/

/// Server state to hold the current environment and other necessary states.
#[derive(Clone)]
pub struct ServerState {
    // pub instance_external_ip: Option<String>,
    // pub instance_internal_ip: Option<String>,
    // pub instance_port: Option<u16>,
    // pub current_environment: Option<Environment>,
    pub sql_client: Option<Pool>,
    pub sfu_v1: Option<mediasoupv1::SFU>,
    pub boot_timestamp: Option<u128>,
    pub meter: Meter,
}

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

#[derive(Debug, Clone)]
pub struct InstanceMeta {
    pub external_ip: String,
    pub internal_ip: String,
}
/// Main function to start the server.
#[tokio::main]
async fn main() {
    env::set_var("TOKIO_CONSOLE_BIND", "0.0.0.0:6669");
    // console_subscriber::init();
    let current_environment = load_env();
    load_config(current_environment.clone());
    let instance_meta = get_instance_meta(current_environment.clone()).await;
    init_logger(current_environment.clone());
    log_panics::init();
    log_panics::Config::new()
        .backtrace_mode(log_panics::BacktraceMode::Unresolved)
        .install_panic_hook();

    info!(
        "
    ____                           __                    __
   / __ \\ ____   ____   _____ ____/ /____ _ _____ _____ / /_   ____ _ ____
  / / / // __ \\ / __ \\ / ___// __  // __ `// ___// ___// __ \\ / __ `// __ \\
 / /_/ // /_/ // /_/ // /   / /_/ // /_/ // /   (__  )/ / / // /_/ // / / /
/_____/ \\____/ \\____//_/    \\__,_/ \\__,_//_/   /____//_/ /_/ \\__,_//_/ /_/
    __  ___           __ _           _____
   /  |/  /___   ____/ /(_)____ _   / ___/ ___   _____ _   __ ___   _____
  / /|_/ // _ \\ / __  // // __ `/   \\__ \\ / _ \\ / ___/| | / // _ \\ / ___/
 / /  / //  __// /_/ // // /_/ /   ___/ //  __// /    | |/ //  __// /
/_/  /_/ \\___/ \\__,_//_/ \\__,_/   /____/ \\___//_/     |___/ \\___//_/
   "
    );
    info!("Starting Doordarshan MediaServer");
    info!("Environment: {:?}", current_environment);
    info!("Elastic IP: {}", instance_meta.external_ip);
    info!("Private IP: {}", instance_meta.internal_ip);

    let server_port: u16 = common::get_env("SERVER_PORT", "3000")
        .parse()
        .expect("Invalid SERVER_PORT");
    let mysql_creds_path = common::get_env("MYSQL_CREDS_PATH", "");
    if mysql_creds_path != "" {
        let file_content: String = match fs::read_to_string(mysql_creds_path) {
            Ok(content) => content,
            Err(e) => {
                error!("Error while reading MySQL creds file: {}", e);
                panic!("Error while reading MySQL creds file: {}", e);
            }
        };
        let credentials: Credentials = match serde_json::from_str(&file_content) {
            Ok(creds) => creds,
            Err(e) => {
                error!("Error while parsing MySQL creds file: {}", e);
                panic!("Error while parsing MySQL creds file: {}", e);
            }
        };
        env::set_var("MYSQL_USER", credentials.username);
        env::set_var("MYSQL_PASSWORD", credentials.password);
    }
    let db_url = MySqlUrl::new();
    let pool = match Pool::new(db_url.clone().as_str()) {
        Ok(pool) => pool,
        Err(e) => {
            panic!(
                "Error while creating MySQL pool: {} with string {}",
                e, db_url
            );
        }
    };
    let boot_timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let result = init_metrics();
    assert!(
        result.is_ok(),
        "Init metrics failed with error: {:?}",
        result.err()
    );
    let meter_provider = result.unwrap();
    global::set_meter_provider(meter_provider.clone());
    let common_scope_attributes = vec![KeyValue::new("scope-key", "scope-value")];
    let meter = global::meter_with_version(
        "basic",
        Some("v1.0"),
        Some("schema_url"),
        Some(common_scope_attributes.clone()),
    );
    let tenant: String = common::get_env("TENANT", "NA")
        .parse()
        .expect("Invalid Tenant");
    if tenant == "NA" {
        error!("Invalid Tenant");
        panic!("Invalid Tenant");
    }

    let analytic_client = client::AnalyticMetricClient::new(ClientConfigDetails::from_env()).await;

    let sfu_v1 = mediasoupv1::SFU::new(
        SFUConfig::from_env(),
        instance_meta,
        server_port.clone(),
        Some(pool.clone()),
        boot_timestamp,
        tenant,
        meter.clone(),
        analytic_client,
    )
    .await;

    let gstreamer = Gstreamer::new();

    let server_state = ServerState {
        // instance_external_ip: Some(instance_external_ip),
        // instance_internal_ip: Some(instance_private_ip),
        // instance_port: Some(server_port),
        // current_environment: Some(current_environment),
        sql_client: Some(pool.clone()),
        sfu_v1: Some(sfu_v1),
        boot_timestamp: Some(boot_timestamp),
        meter: meter.clone(),
    };

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/health", get(health_handler))
        .merge(service::setup_routes())
        .merge(SwaggerUi::new("/docs").url("/openapi.json", v1::openapi::ApiDoc::openapi()))
        .layer(AddExtensionLayer::new(server_state.clone()))
        .layer(CorsLayer::permissive())
        // .layer(TraceLayer::new_for_http().on_request(on_request).on_response(on_response))
        .layer(middleware::from_fn(header_middleware))
        .layer(middleware::from_fn(logging_middleware));

    let addr = SocketAddr::from(([0, 0, 0, 0], server_port));

    info!("Media Server started!");
    info!("Listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(server_state))
    .await
    .unwrap();
}

async fn header_middleware(req: Request<Body>, next: Next) -> impl IntoResponse {
    let headers = req.headers().clone();
    if !headers.contains_key(X_REQUEST_ID) {
        let request_id = uuid::Uuid::new_v4().to_string();
        let mut req = req;
        req.headers_mut()
            .insert(X_REQUEST_ID, request_id.parse().unwrap());
        set_request_id(request_id.parse().unwrap());
        return next.run(req).await;
    }
    next.run(req).await
}

async fn logging_middleware(
    req: Request<Body>,
    next: Next,
) -> Result<impl IntoResponse, (http::StatusCode, String)> {
    let start_time = Instant::now();
    let path = req.uri().clone();
    let method = req.method().clone();
    info!("Received request: {} {}", path, method);
    let response = next.run(req).await;
    let latency = start_time.elapsed();
    info!(
        "Response: {} {} {} | Latency: {:?}",
        method,
        path,
        response.status(),
        latency
    );
    // info!("Response: {} | Latency: {:?}", response.status(), latency);
    Ok(response)
}

/// Health check handler.
async fn health_handler() -> impl IntoResponse {
    //let response = (StatusCode::OK, Json(json!({"status": "ok"})));
    return (StatusCode::OK, Json(json!({"status": "ok"})));
}

/// Index handler.
async fn index_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({"message": "Welcome to Doordarshan Media Server!"})),
    )
}

/// Load the environment based on the environment variable.
fn load_env() -> String {
    let env_var = env::var(ENVIRONMENT_VARIABLE_KEY_ENV);
    if env_var.is_err() {
        return "local".to_string();
    } else {
        return env_var.unwrap();
    }
}
static RESOURCE: Lazy<Resource> =
    Lazy::new(|| Resource::new(vec![KeyValue::new("doordarshan", "otel-metrics")]));
fn http_exporter() -> HttpExporterBuilder {
    let exporter = opentelemetry_otlp::new_exporter().http();
    #[cfg(feature = "hyper")]
    let exporter = exporter.with_http_client(hyper::HyperClient::default());
    exporter
}
fn init_metrics() -> Result<SdkMeterProvider, MetricsError> {
    let otel_endpoint = format!(
        "http://{}/v1/metrics",
        common::get_env(OTEL_ENDPOINT, "localhost:4318")
    );
    opentelemetry_otlp::new_pipeline()
        .metrics(runtime::Tokio)
        .with_exporter(
            http_exporter()
                .with_protocol(Protocol::HttpBinary) //can be changed to `Protocol::HttpJson` to export in JSON format
                .with_endpoint(otel_endpoint),
        )
        .with_resource(RESOURCE.clone())
        .build()
}

async fn get_instance_meta(current_environment: String) -> InstanceMeta {
    if current_environment != "local" {
        let external_ip = env::var("PUBLIC_IP");
        if external_ip.is_err() {
            error!("Error while fetching the external IP");
            panic!("Error while fetching the external IP");
        }
        let internal_ip = env::var("PRIVATE_IP");
        if internal_ip.is_err() {
            error!("Error while fetching the internal IP");
            panic!("Error while fetching the internal IP");
        }
        InstanceMeta {
            external_ip: external_ip.unwrap(),
            internal_ip: internal_ip.unwrap(),
        }
    } else {
        InstanceMeta {
            external_ip: Ipv4Addr::LOCALHOST.to_string(),
            internal_ip: Ipv4Addr::LOCALHOST.to_string(),
        }
    }
}

/// Load the configuration based on the environment.
fn load_config(current_environment: String) {
    dotenvy::from_filename("config/".to_string() + &current_environment + ".env")
        .unwrap_or_default();
}

/// Initialize logger based on the environment.
fn init_logger(current_environment: String) {
    if current_environment == "local" {
        env_logger::Builder::from_env(
            env_logger::Env::default().default_filter_or(common::get_env("LOG_LEVEL", "error")),
        )
        .format(|buf, record| {
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let request_id = if get_request_id().is_some() {
                get_request_id().unwrap().to_string()
            } else {
                "None".to_string()
            };
            writeln!(
                buf,
                "[{}] [{}] [{}] {}",
                timestamp,
                request_id,
                record.level(),
                record.args()
            )
        })
        .init();
        info!("Using local environment, logging to console");
    } else {
        /*let log_file: String = common::get_env("LOG_PATH", "doordarshan-media-server.log")
            .parse()
            .expect("Invalid LOG_PATH");
        let log_output = Box::new(File::create(log_file).expect("Can't create log file"));*/
        env_logger::Builder::from_env(
            env_logger::Env::default().default_filter_or(common::get_env("LOG_LEVEL", "info")),
        )
        .format(|buf, record| {
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let request_id = if get_request_id().is_some() {
                get_request_id().unwrap().to_string()
            } else {
                "None".to_string()
            };
            writeln!(
                buf,
                "[{}] [{}] [{}] {}",
                timestamp,
                request_id,
                record.level(),
                record.args()
            )
        })
        // .target(env_logger::Target::Pipe(log_output))
        .init();

        info!(
            "Using {:?} environment, logging to console",
            current_environment
        );
    }
}

/// Hook into the shutdown signal.
/// https://github.com/tokio-rs/axum/blob/main/examples/graceful-shutdown/src/main.rs.
async fn shutdown_signal(_server_state: ServerState) {
    let ctrl_c = async {
        tokio_signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio_signal::unix::signal(tokio_signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {
            info!("Ctrl-C signal received");
        },
        () = terminate => {
            info!("Terminate signal received");
        },
    }
    shutdown_handler(_server_state).await;
    info!("Shutdown signal received");
}

/// Shutdown handler for the media server.
async fn shutdown_handler(_server_state: ServerState) {
    info!("Shutting down the media server...");
    // Add shutdown logic here
}
