use std::collections::HashMap;
use mediasoup::prelude::{DtlsParameters, IceCandidate, IceParameters, Producer, RtpCapabilities, RtpCapabilitiesFinalized, RtpParameters, TransportId};
use mediasoup::router::RouterId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use crate::mediasoupv1::{Kind, ProducerConsumerCapacity};
/// Data structure containing all the necessary information about transport options required
/// from the server to establish transport connection on the client.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportOptions {
    pub id: TransportId,
    pub dtls_parameters: DtlsParameters,
    pub ice_candidates: Vec<IceCandidate>,
    pub ice_parameters: IceParameters,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecreateProducerTransportResponse {
    pub transport_options: TransportOptions,
    pub closed_producer_details: Vec<ProducerDetail>
}

#[derive(Serialize, Deserialize, Clone, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouterDetails {
    pub router_capacity: i32,
    pub worker_id : String,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceMeetingsRequest {
    pub replace_meeting_requests: Vec<ReplaceMeetingPayload>
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceMeetingPayload {
    pub producer_router_details : Option<Vec<RouterDetails>>,
    pub consumer_router_details : Option<Vec<RouterDetails>>,
    pub routers : Option<RouterDetails>,
    pub meeting_id: String,
    pub old_container_id: String,
    pub new_container_id: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateMeetingPayload {
    pub producer_router_details : Option<Vec<RouterDetails>>,
    pub consumer_router_details : Option<Vec<RouterDetails>>,
    pub routers : Option<RouterDetails>,
    pub meeting_id: String,
    pub container_id: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JoinMeetingPayload {
    pub producer_router_id : String,
    pub consumer_router_id : String,
    pub participant_id : String,
    pub instance_id: String,
    pub meeting_id : String
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinMeetingResponse {
    pub producer_rtp_capabilities : RtpCapabilitiesFinalized,
    pub consumer_rtp_capabilities : RtpCapabilitiesFinalized
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GetParticipantsOfMeetingPayload {
    pub meeting_id : String
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProducerOfMeetingResponse {
    pub participant_id : String,
    pub producer_id : String
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTransportRequest {
    pub meeting_id: String,
    pub participant_id : String,
    pub instance_id: String,
    pub router_id : String,
    pub old_producer_transport_id: Option<String>,
    pub old_consumer_transport_id: Option<String>,
    pub old_producer_ids: Option<Vec<String>>,
    pub old_consumer_ids: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecreateTransportRequest{
    pub meeting_id: String,
    pub participant_id : String,
    pub instance_id: String,
    pub producer_router_id : String,
    pub consumer_router_id : String
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecreateBulkTransportResponse {
    pub producer_transport_options: TransportOptions,
    pub consumer_transport_options: TransportOptions,
    pub closed_producer_details: Vec<ProducerDetail>
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTransportResponse {
    pub transport_options: TransportOptions,
    pub producer_details: Vec<ProducerDetail>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestartIceResponse {
    pub result : HashMap<String, IceParameters>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestartIceRequest {
    pub participant_id : String,
    pub meeting_id: String,
    pub transport_ids : Vec<String>
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateProduceRequest {
    pub participant_id : String,
    pub rtp_parameters : RtpParameters,
    pub meeting_id: String,
    pub kind: String,
    pub producer_router_id: String,
    pub consumer_router_ids: Option<Vec<String>>,
    pub producer_transport_id : String,
    pub start_recording: bool,
    pub instance_id: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProducerDetail {
    pub producer_id: String,
    pub participant_id: String,
    pub kind : String
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProducerResponse {
    pub producer_details: Vec<ProducerDetail>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectTransportRequest {
    pub participant_id: String,
    pub transport_id: String,
    pub dtls_parameters: DtlsParameters,
    pub meeting_id: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateConsumeRequest {
    pub participant_id : String,
    pub rtp_capabilities : RtpCapabilities,
    pub meeting_id: String,
    pub transport_id : String,
    pub target_participants: HashMap<String, HashMap<String, String>>,
    pub instance_id: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConsumerResponse {
    pub consumer_details: Vec<ConsumerDetails>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConsumerDetails {
    pub consumer_id : String,
    pub rtp_parameters: RtpParameters,
    pub producer_id: String,
    pub target_participant_id: String,
    pub kind : String
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GenericResponse {
    pub response : String,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResumeConsumeRequest {
    pub participant_id : String,
    pub meeting_id: String,
    pub consumer_ids: Vec<String>
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResumeProducerRequest {
    pub participant_id : String,
    pub meeting_id: String,
    pub producer_kind_map: HashMap<String, String>
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PauseProducerRequest {
    pub meeting_id: String,
    pub participant_id : String,
    pub producer_kind_map: HashMap<String, String>
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PauseConsumerRequest {
    pub participant_id : String,
    pub meeting_id: String,
    pub consumer_ids : Vec<String>
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CloseConsumerRequest {
    pub participant_id : String,
    pub meeting_id: String,
    pub consumer_ids : Vec<String>
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CloseProducerRequest {
    pub participant_id : String,
    pub meeting_id: String,
    pub producer_kind_map: HashMap<String, String>
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CloseAllProducersRequest {
    pub participant_id : String,
    pub meeting_id: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CloseAllConsumersForProducerRequest {
    pub participant_id : String,
    pub meeting_id: String,
    pub target_participant_id: String
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CloseAllConsumersRequest {
    pub participant_id : String,
    pub meeting_id: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LeaveMeetingRequest {
    pub participant_id : String,
    pub instance_id: String,
    pub meeting_id : String,
    pub producer_transport_id: Option<String>,
    pub consumer_transport_id: Option<String>,
    pub producer_ids : Option<Vec<String>>,
    pub consumer_ids : Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EndMeetingRequest {
    pub meeting_id : String,
    pub router_ids: Vec<String>,
    pub transport_ids : Vec<String>,
    pub producer_ids : Vec<String>,
    pub consumer_ids : Vec<String>,
    pub participant_ids: Vec<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GetRTPCapabilitiesRequest {
    pub meeting_id : String,
    pub producer_router_id : String,
    pub consumer_router_id : String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetRTPCapabilitiesResponse {
    pub producer_rtp_capabilities : RtpCapabilitiesFinalized,
    pub consumer_rtp_capabilities : RtpCapabilitiesFinalized
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreMeetingDetailsRequest {
    pub meeting_id : String,
    pub producer_router_id : String,
    pub consumer_router_id : String,
    pub participant_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreClassDetailsResponse {
    pub producer_rtp_capabilities : RtpCapabilitiesFinalized,
    pub consumer_rtp_capabilities : RtpCapabilitiesFinalized,
    pub producer_details: Vec<ProducerDetail>
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LeaveMeetingResponse {
    pub response : String,
    pub send_signal: bool
}


pub struct MeetingPayload {
    pub worker_payload : HashMap<String, ProducerConsumerCapacity>,
    pub old_container_id: String,
    pub new_container_id: String,
}
