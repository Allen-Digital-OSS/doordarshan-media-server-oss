//! Common code for the service like setting up routes, etc.
pub(crate) mod payload;
pub mod v1;

use axum::routing::method_routing::{get, post};
use axum::Router;

/// Set up the routes for the server.
pub fn setup_routes() -> Router {
    let v1_routes = Router::new()
        .route(
            "/createMeeting",
            post(v1::create_meeting),
        )
        .route(
            "/replaceMeetings",
            post(v1::replace_meetings),
        )
        .route(
            "/joinMeeting",
            post(v1::join_meeting),
        )
        .route(
            "/getProducersOfMeeting",
            post(v1::get_producers_of_meeting),
        )
        .route(
            "/connectTransport",
            post(v1::connect_transport),
        )
        .route(
            "/restartIce",
            post(v1::restart_ice),
        )
        .route(
            "/recreateProducerTransport",
            post(v1::recreate_producer_transport),
        )
        .route(
            "/recreateConsumerTransport",
            post(v1::recreate_consumer_transport),
        )
        /*.route(
            "/recreateTransports",
            post(v1::recreate_transports),
        )*/
        .route(
            "/createConsumer",
            post(v1::create_consumer),
        )
        .route(
            "/createProducer",
            post(v1::create_producer),
        )
        .route(
            "/resumeConsumer",
            post(v1::resume_consumer),
        )
        .route(
            "/resumeProducer",
            post(v1::resume_producer),
        )
        .route(
            "/pauseProducer",
            post(v1::pause_producer),
        )
        .route(
            "/pauseConsumer",
            post(v1::pause_consumer),
        )
        .route(
            "/closeProducer",
            post(v1::close_producer),
        )
        .route(
            "/closeConsumer",
            post(v1::close_consumer),
        )
        .route(
            "/leaveMeeting",
            post(v1::leave_meeting),
        )
        .route(
            "/endMeeting",
            post(v1::end_meeting),
        )
        .route("/getRTPCapabilities",
               post(v1::get_rtp_capabilities)
        )
        .route("/preMeetingDetails",
               post(v1::pre_meeting_details)
        )
        .route("/getParticipantProducers", post(v1::get_participant_producers))
        .route("/getParticipantConsumers", post(v1::get_participant_consumers))
        .route("/pprofStart", get(v1::pprof_start))
        ;

    Router::new()
        .nest("/v1", v1_routes)
}
