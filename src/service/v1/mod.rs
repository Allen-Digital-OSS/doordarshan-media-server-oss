//! API v1 HTTP handlers for the SFU service.
//!
//! This module exposes axum handlers that wrap the SFU business logic
//! implemented in `mediasoupv1`. Each handler accepts a JSON string payload
//! (extracted by the higher-level router), parses it into the corresponding
//! request payload type and invokes the SFU methods on the shared
//! `ServerState` instance.
//!
//! Conventions:
//! - Handlers return types that implement `axum::response::IntoResponse`.
//! - Request payloads are parsed with `parse_payload<T>()`, which returns a
//!   helpful 400 response on parse failure.
//! - On success handlers usually return a JSON response with `StatusCode::OK`.
//! - On internal errors they log the error and return `500 Internal Server Error`.
//!
//! Key handlers in this module:
//! - `create_meeting` / `replace_meetings` — create meeting/router allocations
//!   based on desired capacity.
//! - `join_meeting` / `get_rtp_capabilities` — helpers used by clients to
//!   obtain router RTP capabilities before joining.
//! - Producer / consumer lifecycle endpoints: `create_producer`,
//!   `create_consumer`, `resume_producer`, `resume_consumer`, `pause_*`,
//!   `close_*`, and transport recreation endpoints.
//!
//! Note: These handlers assume the `ServerState` extension provides initialized
//! SFU instances and a working `sql_client` (MySQL `Pool`). They intentionally
//! do not perform detailed validation beyond JSON deserialization — that is
//! delegated to the service layer where necessary.

use std::collections::HashMap;
use std::fmt::format;
use std::fs::File;
use std::mem::take;
use std::sync::Arc;
use std::time::Duration;
use axum::Extension;
use axum::response::IntoResponse;
use crate::{mediasoupv1, ServerState};
use crate::service::payload::{CloseAllConsumersForProducerRequest, CloseAllConsumersRequest, CloseAllProducersRequest, CloseConsumerRequest, CloseProducerRequest, ConnectTransportRequest, ConsumerDetails, ConsumerResponse, CreateConsumeRequest, CreateMeetingPayload, CreateProduceRequest, CreateTransportRequest, CreateTransportResponse, EndMeetingRequest, GenericResponse, GetParticipantsOfMeetingPayload, GetProducerOfMeetingResponse, GetRTPCapabilitiesRequest, GetRTPCapabilitiesResponse, JoinMeetingPayload, JoinMeetingResponse, LeaveMeetingRequest, MeetingPayload, PauseConsumerRequest, PauseProducerRequest, PreClassDetailsResponse, PreMeetingDetailsRequest, RecreateBulkTransportResponse, RecreateProducerTransportResponse, RecreateTransportRequest, ReplaceMeetingsRequest, RestartIceRequest, RestartIceResponse, ResumeConsumeRequest, ResumeProducerRequest, TransportOptions};
use http::StatusCode;
use axum::Json;
use log::{error, info};
use mediasoup::prelude::{DtlsParameters, IceCandidate, IceParameters, ProducerId, TransportId};
use pprof::ProfilerGuardBuilder;
use serde::{de, Serialize};
use serde_json::json;
use tokio::fs::read_to_string;
use tokio::time::sleep;
use uuid::Uuid;
use crate::dao::ParticipantMeeting;
use crate::db::{MySql, MySqlContext};
use crate::mediasoupv1::{Kind, ProducerConsumerCapacity};

pub(crate) mod openapi;

/// Parse a JSON string payload into the requested type `T`.
///
/// On success returns `Ok(T)`. If deserialization fails the function
/// immediately constructs a `400 Bad Request` JSON response describing the
/// parsing error and returns it wrapped in `Err(...)` so caller can return
/// early.
pub fn parse_payload<T: for<'a> de::Deserialize<'a>>(
    request_payload: String,
) -> Result<T, impl IntoResponse> {
    Ok(match serde_json::from_str(request_payload.as_str()) {
        Ok(value) => value,
        Err(error) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": error.to_string()})),
            )
                .into_response())
        }
    })
}

/// Create a new meeting and allocate routers on workers according to payload.
///
/// The handler expects a `CreateMeetingPayload` JSON body. It converts the
/// requested router/producers/consumers specifications into a map of
/// `ProducerConsumerCapacity` per worker and calls the SFU's
/// `create_meeting_and_routers_on_workers` implementation. On success returns
/// a `GenericResponse` with a message.

#[utoipa::path(
    post,
    path = "/create_meeting",
    request_body = CreateMeetingPayload,
    responses(
        (status = 200, description = "Created"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
#[axum_macros::debug_handler]
pub async fn create_meeting(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                            -> impl IntoResponse {
    let payload = match parse_payload::<CreateMeetingPayload>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut worker_router_capacity: HashMap<String, Vec<ProducerConsumerCapacity>> = HashMap::default();
    //payload validation
    if payload.routers.is_none() &&
        (payload.producer_router_details.is_none() && payload.consumer_router_details.is_some()) &&
        (payload.producer_router_details.is_some() && payload.consumer_router_details.is_none()) {
        return (
            StatusCode::BAD_REQUEST,
            Json("Neither single router details are present nor producer and consumer details are absent")
        ).into_response();
    }
    if payload.routers.is_some() {
        let producer_router_detail = payload.routers.unwrap();
        let temp = ProducerConsumerCapacity {
            producer_capacity: None,
            consumer_capacity: None,
            capacity: Some(producer_router_detail.router_capacity),
        };
        let temp_vector = vec![temp];
        worker_router_capacity.insert(producer_router_detail.worker_id.clone(), temp_vector);
    } else {
        let producer_router_details = payload.producer_router_details.unwrap();
        let consumer_router_details = payload.consumer_router_details.unwrap();
        for producer_router_detail in producer_router_details {
            let temp = ProducerConsumerCapacity {
                producer_capacity: Some(producer_router_detail.router_capacity),
                consumer_capacity: None,
                capacity: None,
            };
            let temp_vector = vec![temp];
            worker_router_capacity.insert(producer_router_detail.worker_id.clone(), temp_vector);
        }
        for consumer_router_detail in consumer_router_details {
            let temp = ProducerConsumerCapacity {
                producer_capacity: None,
                consumer_capacity: Some(consumer_router_detail.router_capacity),
                capacity: None,
            };
            let temp_vector = vec![temp];
            worker_router_capacity.insert(consumer_router_detail.worker_id.clone(), temp_vector);
        }
    }

    match _server_state.sfu_v1.unwrap().create_meeting_and_routers_on_workers(
        worker_router_capacity,
        payload.meeting_id,
        payload.container_id,
        _server_state.sql_client.unwrap()).await
    {
        Ok(o) => (
            StatusCode::OK,
            Json(GenericResponse { response: o })
        ).into_response(),
        Err(e) => {
            error!("Error in creation of router {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR,
             Json(format!("Error in creation of meeting {}", e)),
            ).into_response()
        }
    }
}

#[utoipa::path(
    post,
    path = "/replace_meetings",
    request_body = ReplaceMeetingsRequest,
    responses(
        (status = 200, description = "Replaced"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
#[axum_macros::debug_handler]
pub async fn replace_meetings(Extension
                              (mut _server_state): Extension<ServerState>, request_payload: String) -> impl IntoResponse {
    let payload = match parse_payload::<ReplaceMeetingsRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut batch_router_payload: HashMap<String, MeetingPayload> = HashMap::default();

    for payload in payload.replace_meeting_requests {
        let mut worker_payload: HashMap<String, ProducerConsumerCapacity> = HashMap::default();
        if let Some(routers) = payload.routers.clone() {
            let router_details = routers;
            let producer_consumer_capacity = ProducerConsumerCapacity {
                producer_capacity: None,
                consumer_capacity: None,
                capacity: Some(router_details.router_capacity),
            };
            worker_payload.insert(router_details.worker_id, producer_consumer_capacity);
            let meeting_payload = MeetingPayload {
                worker_payload,
                old_container_id: payload.old_container_id.clone(),
                new_container_id: payload.new_container_id.clone(),
            };
            batch_router_payload.insert(payload.meeting_id.clone(), meeting_payload);
        } else {
            let producer_router_details = payload.producer_router_details.unwrap();
            let consumer_router_details = payload.consumer_router_details.unwrap();
            for producer_router_detail in producer_router_details {
                let temp = ProducerConsumerCapacity {
                    producer_capacity: Some(producer_router_detail.router_capacity),
                    consumer_capacity: None,
                    capacity: None,
                };
                worker_payload.insert(producer_router_detail.worker_id.clone(), temp);
            }
            for consumer_router_detail in consumer_router_details {
                let temp = ProducerConsumerCapacity {
                    producer_capacity: None,
                    consumer_capacity: Some(consumer_router_detail.router_capacity),
                    capacity: None,
                };
                worker_payload.insert(consumer_router_detail.worker_id.clone(), temp);
            }
            let meeting_payload = MeetingPayload {
                worker_payload,
                old_container_id: payload.old_container_id.clone(),
                new_container_id: payload.new_container_id.clone(),
            };
            batch_router_payload.insert(payload.meeting_id.clone(), meeting_payload);
        }
    }
    match _server_state.sfu_v1.unwrap().
        batch_create_router_on_worker(batch_router_payload, _server_state.sql_client.unwrap()).await {
        Ok(o) => (
            StatusCode::OK,
            Json(o)
        ).into_response(),
        Err(e) => {
            error!("Error in creation of router {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR,
             Json(format!("Error in creation of meeting {}", e)),
            ).into_response()
        }
    }
}

#[utoipa::path(
    post,
    path = "/join_meeting",
    request_body = JoinMeetingPayload,
    responses(
        (status = 200, description = "Get RTP capabilities"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
/// Join meeting: return router RTP capabilities the client should use.
///
/// The handler returns both producer and consumer router RTP capabilities so
/// clients can configure their transports accordingly before creating
/// producers/consumers.
pub async fn join_meeting(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                          -> impl IntoResponse {
    let payload = match parse_payload::<JoinMeetingPayload>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    let producer_router_id = payload.producer_router_id;
    let consumer_router_id = payload.consumer_router_id;
    let producer_rtp_capabilities = sfu_v1.get_router_capabilities(producer_router_id.clone()).await;
    let consumer_rtp_capabilities = sfu_v1.get_router_capabilities(consumer_router_id.clone()).await;
    (
        StatusCode::OK,
        Json(json!(JoinMeetingResponse{
            producer_rtp_capabilities,
            consumer_rtp_capabilities,
        }))
    ).into_response()
}

/// Return list of producers for a meeting.
///
/// This handler queries the SFU for all producers associated with the
/// specified meeting and returns them as `ProducerResponse`.
#[utoipa::path(
    post,
    path = "/get_producers_of_meeting",
    request_body = GetParticipantsOfMeetingPayload,
    responses(
        (status = 200, description = "Get Producers of Meeting"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn get_producers_of_meeting(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                                      -> impl IntoResponse {
    let payload = match parse_payload::<GetParticipantsOfMeetingPayload>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    let meeting_id = payload.meeting_id;
    match sfu_v1.get_producers_of_meeting(meeting_id.clone(),
                                          _server_state.sql_client.unwrap()).await {
        Ok(o) => (
            StatusCode::OK,
            Json(o)
        ).into_response(),
        Err(e) => "Error in getting participants of meeting".into_response()
    }
}

/// Connect/upsert DTLS parameters for a transport.
///
/// Accepts a `ConnectTransportRequest` and calls `connect_transport` on the
/// SFU. Returns a simple `GenericResponse` on success.
#[utoipa::path(
    post,
    path = "/connect_transport",
    request_body = ConnectTransportRequest,
    responses(
        (status = 200, description = "Connect Transport"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn connect_transport(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                               -> impl IntoResponse {
    let payload = match parse_payload::<ConnectTransportRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    match sfu_v1.connect_transport(payload.participant_id, payload.transport_id, payload.dtls_parameters).await
    {
        Ok(o) => (
            StatusCode::OK,
            Json(GenericResponse { response: o })
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json("Error in connecting producer transport")
        ).into_response()
    }
}

/// Restart ICE for a set of transports belonging to a participant.
///
/// Accepts `RestartIceRequest` and forwards to SFU's `restart_ice`.
#[utoipa::path(
    post,
    path = "/restart_ice",
    request_body = RestartIceRequest,
    responses(
        (status = 200, description = "Restart ICE"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn restart_ice(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                         -> impl IntoResponse {
    let payload = match parse_payload::<RestartIceRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    match sfu_v1.restart_ice(payload.participant_id, payload.transport_ids).await {
        Ok(o) => (
            StatusCode::OK,
            Json(json!(o))
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json("Error in restarting ice")
        ).into_response()
    }
}

/// Recreate (replace) a producer transport for a participant.
///
/// This is used when a transport dies and needs to be replaced; the handler
/// accepts a `CreateTransportRequest` and delegates to the SFU helper which
/// performs DB updates and transport recreation. Returns `RecreateProducerTransportResponse`.
#[utoipa::path(
    post,
    path = "/recreate_producer_transport",
    request_body = CreateTransportRequest,
    responses(
        (status = 200, description = "Recreate Producer Transport"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn recreate_producer_transport(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                                         -> impl IntoResponse {
    let payload = match parse_payload::<CreateTransportRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    let participant_id = payload.participant_id;
    let meeting_id = payload.meeting_id;
    let router_id = payload.router_id;
    let instance_id = payload.instance_id;
    let pool = _server_state.sql_client.unwrap();

    match sfu_v1.recreate_producer_transport_helper(pool.clone(),
                                                    participant_id.clone(),
                                                    meeting_id.clone(),
                                                    instance_id.clone(),
                                                    router_id,
                                                    None,
                                                    _server_state.meter,
                                                    payload.old_producer_transport_id,
                                                    payload.old_consumer_transport_id).await {
        Ok(o) => (
            StatusCode::OK,
            Json(o)
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json("Error in creating producer transport")
        ).into_response()
    }
}

/// Recreate (replace) a consumer transport for a participant.
///
/// Similar to the producer transport helper but for consumer-side plain/
/// webrtc transports. Returns a `CreateTransportResponse` on success.
#[utoipa::path(
    post,
    path = "/recreate_consumer_transport",
    request_body = CreateTransportRequest,
    responses(
        (status = 200, description = "Recreate Consumer Transport"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn recreate_consumer_transport(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                                         -> impl IntoResponse {
    let payload = match parse_payload::<CreateTransportRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    let participant_id = payload.participant_id;
    let meeting_id = payload.meeting_id;
    let router_id = payload.router_id;
    let instance_id = payload.instance_id;
    let pool = _server_state.sql_client.unwrap();

    match sfu_v1.recreate_consumer_transport_helper(pool.clone(),
                                                    participant_id.clone(),
                                                    meeting_id.clone(),
                                                    instance_id.clone(),
                                                    router_id,
                                                    None,
                                                    _server_state.meter,
                                                    payload.old_producer_transport_id,
                                                    payload.old_consumer_transport_id).await {
        Ok(o) => (
            StatusCode::OK,
            Json(o)
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!("Failed to fetch producers of meeting"))
        ).into_response()
    }
}

/*
  Handlers for producer/consumer lifecycle: `create_producer`,
  `create_consumer`, `resume_consumer`, `resume_producer`, `pause_*`,
  `close_*`, `leave_meeting`, `end_meeting` are documented below. Each
  handler follows the same pattern: parse payload -> call SFU -> return
  JSON response or 500 on error.
*/

/// Create a producer for a participant.
#[axum_macros::debug_handler]
#[utoipa::path(
    post,
    path = "/create_producer",
    request_body = CreateProduceRequest,
    responses(
        (status = 200, description = "Create Producer"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn create_producer(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                             -> impl IntoResponse {
    let payload = match parse_payload::<CreateProduceRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    match sfu_v1.
        create_producer(
            payload.participant_id,
            payload.rtp_parameters,
            payload.kind,
            payload.meeting_id,
            payload.producer_router_id,
            payload.consumer_router_ids,
            payload.producer_transport_id,
            payload.start_recording,
            payload.instance_id,
            _server_state.meter.clone(),
            _server_state.sql_client.unwrap()).await
    {
        Ok(p) => (
            StatusCode::OK,
            Json(p)
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json("Error in creating producer")
        ).into_response()
    }
}

/// Create consumers for a participant.
#[axum_macros::debug_handler]
#[utoipa::path(
    post,
    path = "/create_consumer",
    request_body = CreateConsumeRequest,
    responses(
        (status = 200, description = "Create Consumer"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn create_consumer(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                             -> impl IntoResponse {
    let payload = match parse_payload::<CreateConsumeRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    match sfu_v1.create_consumer(payload.participant_id, payload.transport_id,
                                 payload.target_participants,
                                 payload.rtp_capabilities, payload.meeting_id,
                                 payload.instance_id,
                                 _server_state.sql_client.unwrap()).await
    {
        Ok(c) => {
            let mut response: Vec<ConsumerDetails> = Vec::new();
            for entry in c {
                let target_participant_id = entry.0;
                let kind_consumer = entry.1;
                for consumer in kind_consumer {
                    let consumer_id = consumer.1.clone().id().to_string();
                    let producer_id = consumer.1.clone().producer_id().to_string();
                    let rtp_parameters = consumer.1.clone().rtp_parameters().clone();
                    response.push(ConsumerDetails {
                        consumer_id,
                        producer_id,
                        rtp_parameters,
                        target_participant_id: target_participant_id.clone(),
                        kind: consumer.0.clone(),
                    });
                }
            }
            (StatusCode::OK,
             Json(json!(ConsumerResponse{
                 consumer_details: response
             }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!(format!("Error in creating consumer {}", e)))
        ).into_response()
    }
}

/// Resume a set of consumers for a participant.
#[utoipa::path(
    post,
    path = "/resume_consumer",
    request_body = ResumeConsumeRequest,
    responses(
        (status = 200, description = "Resume Consumer"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn resume_consumer(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                             -> impl IntoResponse {
    let payload = match parse_payload::<ResumeConsumeRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    match sfu_v1.resume_consumer(payload.participant_id, payload.consumer_ids, payload.meeting_id).await {
        Ok(o) => (
            StatusCode::OK,
            Json(json!(GenericResponse{response: "Successfully resumed".to_string()}))
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!(format!("Error in resuming consumer {}", e)))
        ).into_response()
    }
}

/// Resume a set of producers for a participant.
#[utoipa::path(
    post,
    path = "/resume_producer",
    request_body = ResumeProducerRequest,
    responses(
        (status = 200, description = "Resume Producer"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn resume_producer(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                             -> impl IntoResponse {
    let payload = match parse_payload::<ResumeProducerRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    match sfu_v1.resume_producer(payload.participant_id, payload.producer_kind_map).await {
        Ok(o) => (
            StatusCode::OK,
            Json(json!(o))
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!(format!("Error in resuming producer {}", e)))
        ).into_response()
    }
}

/// Pause producers for a participant.
#[axum_macros::debug_handler]
#[utoipa::path(
    post,
    path = "/pause_producer",
    request_body = PauseProducerRequest,
    responses(
        (status = 200, description = "Pause Producer"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn pause_producer(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                            -> impl IntoResponse {
    let payload = match parse_payload::<PauseProducerRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    match sfu_v1.pause_producer(payload.participant_id, payload.producer_kind_map).await {
        Ok(o) => (
            StatusCode::OK,
            Json(json!(o)),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!(format!("Error in creating consumer {}", e)))
        ).into_response()
    }
}

/// Pause consumers for a participant.
#[axum_macros::debug_handler]
#[utoipa::path(
    post,
    path = "/pause_consumer",
    request_body = PauseConsumerRequest,
    responses(
        (status = 200, description = "Pause Consumer"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn pause_consumer(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                            -> impl IntoResponse {
    let payload = match parse_payload::<PauseConsumerRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    match sfu_v1.pause_consumer(payload.participant_id, payload.consumer_ids, payload.meeting_id).await {
        Ok(o) => (
            StatusCode::OK,
            Json(json!(GenericResponse{response: "Successfully paused".to_string()})),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!(format!("Error in creating consumer {}", e)))
        ).into_response()
    }
}

/// Close a producer (and associated resources).
#[utoipa::path(
    post,
    path = "/close_producer",
    request_body = CloseProducerRequest,
    responses(
        (status = 200, description = "Close Producer"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn close_producer(Extension(mut _server_state): Extension<ServerState>, request_payload: String) -> impl IntoResponse {
    let payload = match parse_payload::<CloseProducerRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    match sfu_v1.close_producer(payload.participant_id,
                                payload.meeting_id,
                                payload.producer_kind_map,
                                _server_state.sql_client.unwrap()).await {
        Ok(o) => (
            StatusCode::OK,
            Json(json!(o))
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!(format!("Error in closing producer {}", e)))
        ).into_response()
    }
}

/// Close consumers for a participant.
#[utoipa::path(
    post,
    path = "/close_consumer",
    request_body = CloseConsumerRequest,
    responses(
        (status = 200, description = "Close Consumer"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn close_consumer(Extension(mut _server_state): Extension<ServerState>, request_payload: String) -> impl IntoResponse {
    let payload = match parse_payload::<CloseConsumerRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    match sfu_v1.close_consumer(payload.participant_id,
                                payload.consumer_ids,
                                payload.meeting_id,
                                _server_state.sql_client.unwrap()).await {
        Ok(o) => (
            StatusCode::OK,
            Json(json!(GenericResponse{response: "Successfully closed consumer".to_string()})),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!(format!("Error in closing consumer {}", e)))
        ).into_response()
    }
}

/// Participant leaves meeting: cleanup and notify analytics.
#[utoipa::path(
    post,
    path = "/leave_meeting",
    request_body = LeaveMeetingRequest,
    responses(
        (status = 200, description = "Leave Meeting"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn leave_meeting(Extension(mut _server_state): Extension<ServerState>, request_payload: String) -> impl IntoResponse {
    let payload = match parse_payload::<LeaveMeetingRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    match sfu_v1.leave_meeting(payload.participant_id,
                               payload.meeting_id,
                               payload.instance_id,
                               payload.producer_transport_id,
                               payload.consumer_transport_id,
                               _server_state.sql_client.unwrap()).await {
        Ok(o) => (
            StatusCode::OK,
            Json(json!(o)),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!(format!("Error in leaving meeting {}", e)))
        ).into_response()
    }
}

/// End a meeting and clean up all resources.
#[utoipa::path(
    post,
    path = "/end_meeting",
    request_body = EndMeetingRequest,
    responses(
        (status = 200, description = "End Meeting"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn end_meeting(Extension(mut _server_state): Extension<ServerState>, request_payload: String) -> impl IntoResponse {
    let payload = match parse_payload::<EndMeetingRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    match sfu_v1.end_meeting(payload.meeting_id,
                             payload.router_ids,
                             payload.transport_ids,
                             payload.participant_ids,
                             _server_state.sql_client.unwrap()).await {
        Ok(o) => (
            StatusCode::OK,
            Json(json!(GenericResponse{response: "Successfully ended meeting".to_string()})),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!(format!("Error in ending meeting {}", e)))
        ).into_response()
    }
}

/// Retrieve RTP capabilities for the given routers (producer & consumer).
#[utoipa::path(
    post,
    path = "/get_rtp_capabilities",
    request_body = GetRTPCapabilitiesRequest,
    responses(
        (status = 200, description = "Get RTP Capabilities"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn get_rtp_capabilities(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                                  -> impl IntoResponse {
    let payload = match parse_payload::<GetRTPCapabilitiesRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    let producer_router_id = payload.producer_router_id;
    let consumer_router_id = payload.consumer_router_id;
    let producer_rtp_capabilities = sfu_v1.get_router_capabilities(producer_router_id.clone()).await;
    let consumer_rtp_capabilities = sfu_v1.get_router_capabilities(consumer_router_id.clone()).await;
    (
        StatusCode::OK,
        Json(json!(GetRTPCapabilitiesResponse{
            producer_rtp_capabilities,
            consumer_rtp_capabilities,
        }))
    ).into_response()
}

/// Pre-meeting details (capabilities + current producers) used for setup UI.
#[utoipa::path(
    post,
    path = "/pre_meeting_details",
    request_body = PreMeetingDetailsRequest,
    responses(
        (status = 200, description = "Pre Meeting Details"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn pre_meeting_details(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                                 -> impl IntoResponse {
    let payload = match parse_payload::<PreMeetingDetailsRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    let producer_router_id = payload.producer_router_id;
    let consumer_router_id = payload.consumer_router_id;
    let meeting_id = payload.meeting_id;
    let producer_rtp_capabilities = sfu_v1.get_router_capabilities(producer_router_id.clone()).await;
    let consumer_rtp_capabilities = sfu_v1.get_router_capabilities(consumer_router_id.clone()).await;
    match sfu_v1.get_producers_of_meeting(meeting_id.clone(), _server_state.sql_client.unwrap()).await
    {
        Ok(o) => (
            StatusCode::OK,
            Json(json!(PreClassDetailsResponse{
                producer_rtp_capabilities,
                consumer_rtp_capabilities,
                producer_details: o.producer_details,
            }))
        ).into_response(),
        Err(e) => "Error in getting participants of meeting".into_response()
    }
}

/// Retrieve participant producers (in-memory).
#[utoipa::path(
    post,
    path = "/get_participant_producers",
    request_body = RestartIceRequest,
    responses(
        (status = 200, description = "Get Participant Producers"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn get_participant_producers(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                                       -> impl IntoResponse {
    let payload = match parse_payload::<RestartIceRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    let participant_id = payload.participant_id;
    let producers =  sfu_v1.get_participant_producers(participant_id.clone()).await;
    (
        StatusCode::OK,
        Json(producers)
    ).into_response()
}

/// Retrieve participant consumers (in-memory shard).
#[utoipa::path(
    post,
    path = "/get_participant_consumers",
    request_body = RestartIceRequest,
    responses(
        (status = 200, description = "Get Participant Consumers"),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Unknown error"),
    )
)]
pub async fn get_participant_consumers(Extension(mut _server_state): Extension<ServerState>, request_payload: String)
                                       -> impl IntoResponse {
    let payload = match parse_payload::<RestartIceRequest>(request_payload) {
        Ok(v) => v,
        Err(v) => return v.into_response(),
    };
    let mut sfu_v1 = _server_state.sfu_v1.unwrap();
    let participant_id = payload.participant_id;
    let meeting_id = payload.meeting_id;
    let consumers =  sfu_v1.get_participant_consumers(participant_id.clone(), meeting_id.clone()).await;
    (
        StatusCode::OK,
        Json(consumers)
    ).into_response()
}

/// Start a pprof profiling run and save a flamegraph to disk.
pub async fn pprof_start() -> impl IntoResponse {
    // Placeholder for pprof start functionality
    let guard = ProfilerGuardBuilder::default()
        .frequency(1000)
        .blocklist(&["libc", "libgcc", "pthread", "vdso"])
        .build()
        .unwrap();

    // Profile for 10 seconds (or profile workload during this time)
    sleep(Duration::from_secs(30)).await;
    if let Ok(report) = guard.report().build() {
        // Create a file to save the SVG flamegraph
        let mut file = File::create("flamegraph.svg").expect("Failed to create file");

        let mut options = pprof::flamegraph::Options::default();
        options.image_width = Some(1600);

        // Write flamegraph SVG data directly into the file
        report.flamegraph_with_options(&mut file, &mut options)
            .expect("Failed to write flamegraph");

        info!("Flamegraph saved to flamegraph.svg");
    }
    (
        StatusCode::OK,
        Json(json!({"status": "pprof started"}))
    ).into_response()
}
