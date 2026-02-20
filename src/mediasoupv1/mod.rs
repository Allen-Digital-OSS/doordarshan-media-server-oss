//! SFU (Selective Forwarding Unit) implementation and runtime helpers.
//!
//! This module implements an SFU built on top of `mediasoup` and provides the
//! runtime glue to manage workers, routers, transports, producers and
//! consumers. It also contains in-memory representations for producers,
//! consumers and transports used by higher-level application code.
//!
//! Responsibilities in this module:
//! - Create and configure mediasoup `Worker`s and `Router`s.
//! - Provide lifecycle operations to create/destroy transports, producers and
//!   consumers.
//! - Maintain in-memory maps to track active entities and provide helper
//!   functions for capacity, sharding and recording integration.
//!
//! Concurrency notes:
//! - Internal shared maps use `Arc`, `Mutex`, and `RwLock` to allow safe
//!   concurrent access from async tasks and sync callbacks. Care is taken to
//!   limit lock contention; callers should avoid holding locks across await
//!   points where possible.
//!
//! Example usage:
//! ```no_run
//! let sfu = SFU::new(config, instance_meta, port, pool, boot_ts, tenant, meter, client).await;
//! ```

mod metrics;

use std::option::Option;
use std::collections::HashMap;
use std::{env, fmt, fs, io, thread};
use std::any::Any;
use std::cell::Cell;
use std::collections::hash_map::Entry;
use std::fmt::{Display, format, Formatter};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr};
use std::num::{NonZeroU32, NonZeroU8};
use std::ops::RangeInclusive;
use std::ptr::null;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use anyhow::__private::kind::TraitKind;
use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::primitives::event_stream::Message;
use chrono::Utc;
use clap::builder::Str;
use event_listener_primitives::{Bag, BagOnce};
use gstreamer::{Clock, MessageView, Pipeline, State};
use gstreamer::prelude::{ElementExt, ElementExtManual, GObjectExtManualGst, GstBinExt, GstBinExtManual, GstObjectExt, ObjectExt, PipelineExt};
use gstreamer::State::{Null, Paused, Playing};
use local_ip_address::local_ip;
use log::{debug, error, info, warn};
use log::__private_api::loc;
use mediasoup::consumer::{Consumer, ConsumerId, ConsumerOptions};
use mediasoup::data_structures::{ListenInfo, Protocol};
use mediasoup::plain_transport::PlainTransportRemoteParameters;
use mediasoup::prelude::{ConsumeError, DtlsParameters, IceParameters, MediaKind, MimeTypeAudio, MimeTypeVideo, PipeProducerToRouterPair, PlainTransport, PlainTransportOptions, ProduceError, Producer, ProducerId, ProducerOptions, Router, RouterOptions, RtcpFeedback, RtpCapabilities, RtpCodecCapability, RtpCodecParametersParameters, RtpParameters, TransportId, WebRtcTransportListenInfos, WebRtcTransportOptions, WebRtcTransportRemoteParameters, Worker, WorkerManager, WorkerSettings};
use mediasoup::producer::PipedProducer;
use mediasoup::router::{PipeToRouterOptions, RouterId};
use mediasoup::rtp_parameters::{RtpCapabilitiesFinalized, RtpCodecCapabilityFinalized, RtpCodecParametersParametersValue};
use mediasoup::transport::Transport;
use mediasoup::webrtc_transport::{WebRtcTransport, WebRtcTransportStat};
use mediasoup::worker::{RequestError, WorkerId, WorkerLogLevel, WorkerLogTag};
use mediasoup::router;
use mysql::{from_row, AccessMode, IsolationLevel, Pool, Row, TxOpts};
use mysql::prelude::Queryable;
use opentelemetry::metrics::{Counter, Meter};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, UdpSocket};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;
use crate::dao::{ContainerHeartbeat, Dao, MeetingStatus, MutationType, ParticipantConsumerRouter, ParticipantConsumerTransport, ParticipantConsumers, ParticipantMeeting, ParticipantProducerRouter, ParticipantProducerTransport, ParticipantProducers, RouterMeeting, RouterWorker, WorkerContainer};
use crate::{client, dao, db, InstanceMeta};
use crate::client::AnalyticMetricClient;
use crate::dao::MutationType::{CustomQuery1, Delete, Insert, Update};
use crate::db::{DBError, MySql, MySqlContext};
use crate::gstreamer::{Gstreamer};
use crate::mediasoup::SFUConfig;
use crate::common::{UserEvent};
use crate::service::payload::{GetProducerOfMeetingResponse, RecreateBulkTransportResponse, RecreateProducerTransportResponse, TransportOptions, ProducerDetail, ProducerResponse, MeetingPayload, LeaveMeetingResponse, RestartIceResponse, CreateTransportRequest, CreateTransportResponse};

/// Top-level SFU instance that owns shared state and configuration.
///
/// The `SFU` struct contains an `id`, an `Inner` shared state container, the
/// active `SFUConfig`, and metadata about the server instance. Construct an
/// instance with `SFU::new(...)` which sets up workers, internal runtimes
/// and background tasks.
#[derive(Debug, Clone)]
pub struct SFU {
    id: Uuid,
    inner: Arc<Inner>,
    config: SFUConfig,
    server_port: u16,
    instance_meta: InstanceMeta
}

/// In-memory representation of a producer belonging to a participant.
///
/// This type stores the `ProducerId`, the media `Kind` (video/audio/screenshare),
/// optional plain transport identifiers and a pipeline reference used for
/// recording or direct processing.
#[derive(Debug, Clone)]
pub struct ParticipantProducerInMemory {
    // pub producer: Producer,
    pub media_kind: MediaKind,
    pub kind: Kind,
    pub producer_id: ProducerId,
    pub plain_transport_id: Option<TransportId>,
    pub plain_transport_consumer_id: Option<ConsumerId>,
    pub pipeline: Option<Pipeline>,
}

/// In-memory representation of a consumer associated with a participant.
///
/// Holds the `ConsumerId` and the producer participant id so the system can
/// track who is consuming whose stream.
#[derive(Debug, Clone)]
pub struct ParticipantConsumerInMemory {
    // pub consumer: Consumer,
    pub producer_participant_id: String,
    pub kind: Kind,
    pub consumer_id: ConsumerId,
}

/// Lightweight container for active transports stored in memory.
#[derive(Debug, Clone)]
pub struct TransportInMemory {
    pub transport: WebRtcTransport,
    pub meeting_id: String,
    pub participant_id: String,
    pub kind: String,
}

/// Simple wrapper around a `Producer` object stored in memory.
#[derive(Debug, Clone)]
pub struct ProducerInMemory {
    pub producer: Producer,
    pub meeting_id: String,
    pub participant_id: String,
    pub kind: String,
    pub instance_id: String,
}

/// Simple wrapper around a `Consumer` object stored in memory.
#[derive(Debug, Clone)]
pub struct ConsumerInMemory {
    pub consumer: Consumer,
    pub meeting_id: String,
    pub participant_id: String,
    pub target_participant_id: String,
    pub kind: String,
    pub instance_id: String,
    pub producer_id: String,
}

/// In-memory router representation and capacity hint.
#[derive(Debug, Clone)]
pub struct RouterInMemory {
    pub router: Router,
    pub capacity: i32,
}

/// Handlers container used to store cleanup callbacks.
///
/// `Handlers` aggregates one-shot closure callbacks used to cleanup resources
/// when transports or other objects close unexpectedly.
pub struct Handlers {
    transport_close:
        BagOnce<Box<dyn FnOnce() + Send>>,
}

/// Channel used to send/receive `UserEvent`s within the SFU.
///
/// `UserChannel` is a small helper that provides a bounded synchronous sender
/// and an async-friendly `Receiver` wrapped in an `Arc<Mutex<...>>` for use in
/// async tasks. The `new` constructor takes a `buffer_size` to size the
/// internal sync channel.
pub struct UserChannel {
    pub tx: SyncSender<UserEvent>,
    pub rx: Arc<Mutex<Receiver<UserEvent>>>,
}

impl UserChannel {
    /// Create a new `UserChannel` with a bounded buffer.
    pub fn new(buffer_size: usize) -> Self {
        let (tx, rx) = mpsc::sync_channel(buffer_size);
        UserChannel { tx, rx: Arc::new(Mutex::new(rx)) }
    }
}

/// Shared internal state used by the `SFU` instance.
///
/// `Inner` stores the maps of workers, routers, transports, producers and
/// consumers as well as background runtimes for metrics and recordings. The
/// fields intentionally use `Arc`, `Mutex` and `RwLock` so they can be safely
/// shared across async tasks and synchronous callbacks emitted by the
/// mediasoup/gstreamer layers.
pub struct Inner {
    pub workers_map: Mutex<HashMap<String, Worker>>,
    pub routers_map: Arc<RwLock<HashMap<String, RouterInMemory>>>,
    pub transport_map: Arc<RwLock<HashMap<String, TransportInMemory>>>,
    pub producer_map: Arc<RwLock<HashMap<String, ProducerInMemory>>>,
    pub consumer_map_shards: Vec<Arc<RwLock<HashMap<String, ConsumerInMemory>>>>,
    pub plain_transport_map: Arc<RwLock<HashMap<String, PlainTransport>>>,
    pub plain_transport_consumer_map: Arc<RwLock<HashMap<String, Consumer>>>,
    pub pipeline_map: Arc<RwLock<HashMap<String, Option<Gstreamer>>>>,
    pub lock_map: Arc<RwLock<HashMap<String, Arc<Mutex<()>>>>>,
    pub boot_timestamp: u128,
    pub instance_meta: InstanceMeta,
    pub metrics_pool: Runtime,
    pub recordings_pool: Runtime,
    pub shared_clock: Clock,
    pub user_channel: UserChannel,
    pub client: Arc<AnalyticMetricClient>,
}

/// Kind of media a producer/consumer represents.
///
/// Parsing from string is supported via `FromStr` and `Display` is implemented
/// for easy logging. Values: `Video`, `Audio`, `ScreenShare`.
#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub enum Kind {
    Video,
    Audio,
    ScreenShare,
}

impl FromStr for Kind {
    type Err = String;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.to_lowercase().as_str() {
            "video" => Ok(Kind::Video),
            "audio" => Ok(Kind::Audio),
            "screenshare" => Ok(Kind::ScreenShare),
            _ => Err(format!("'{}' is not a valid color", input)),
        }
    }
}

impl Display for Kind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Kind::Video => write!(f, "video"),
            Kind::Audio => write!(f, "audio"),
            Kind::ScreenShare => write!(f, "screenshare"),
        }
    }
}

/// Helper describing producer/consumer capacity hints used when allocating routers.
pub struct ProducerConsumerCapacity {
    pub producer_capacity: Option<i32>,
    pub consumer_capacity: Option<i32>,
    pub capacity: Option<i32>,
}

impl fmt::Debug for Inner {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Inner")
            .field("workers_map", &self.workers_map)
            .field("routers", &self.routers_map)
            .field("boot_timestamp", &self.boot_timestamp)
            .finish()
    }
}

impl SFU {
    /// Create and initialize a new `SFU` instance.
    ///
    /// This async constructor initializes internal runtimes, creates the
    /// `Inner` shared state and spawns background tasks for metrics and
    /// processing user events. It will panic if critical initialization
    /// (database or worker creation) fails.
    pub async fn new(
        sfu_config: SFUConfig,
        instance_meta: InstanceMeta,
        server_port: u16,
        pool: Option<Pool>,
        boot_timestamp: u128,
        tenant: String,
        meter: Meter,
        client: client::AnalyticMetricClient,
    ) -> Self {
        let inner = match Self::create_inner(sfu_config.clone(), pool.unwrap(), boot_timestamp,
                                             instance_meta.clone(), tenant, client).await {
            Ok(o) => o,
            Err(e) => panic!("Error in writing to database {}", e)
        };
        // inner.metrics_pool.spawn(metrics::Metrics::push_metrics_for_transports(Arc::clone(&inner.transport_map), meter.clone(), inner.instance_meta.internal_ip.clone()));
        // inner.metrics_pool.spawn(metrics::Metrics::push_metrics_for_producers(Arc::clone(&inner.producer_map), meter.clone(), inner.instance_meta.internal_ip.clone()));
        // inner.metrics_pool.spawn(metrics::Metrics::push_metrics_for_consumers(Arc::clone(&inner.consumer_map), meter.clone(), inner.instance_meta.internal_ip.clone()));
        let num_cores = num_cpus::get() as i32;
        /*inner.metrics_pool.spawn(metrics::Metrics::push_auto_scale_metrics((num_cores-1)*500, Arc::clone(&inner.routers_map),
                                                                           inner.instance_meta.clone()));*/
        // inner.metrics_pool.spawn(metrics::Metrics::push_metrics_for_pipeline(Arc::clone(&inner.pipeline_map), meter.clone(), inner.instance_meta.internal_ip.clone()));
        inner.metrics_pool.spawn(metrics::Metrics::receive_and_send_user_event(Arc::clone(&inner.user_channel.rx), Arc::clone(&inner.client)));
        Self {
            id: Uuid::new_v4(),
            inner: Arc::new(inner),
            config: sfu_config,
            instance_meta,
            server_port
        }
    }
    /// Build `WorkerSettings` for mediasoup based on `SFUConfig`.
    pub fn worker_settings(sfu_config: SFUConfig) -> WorkerSettings {
        let mut settings = WorkerSettings::default();
        settings.rtc_port_range = sfu_config.rtc_port_range_start..=sfu_config.rtc_port_range_end;
        Self::worker_log_settings(&mut settings);
        settings
    }

    /// Configure worker logging options used for created workers.
    pub fn worker_log_settings(settings: &mut WorkerSettings) -> &mut WorkerSettings {
        settings.log_level = WorkerLogLevel::Error;
        settings.log_tags = vec![
            WorkerLogTag::Info,
            WorkerLogTag::Ice,
            WorkerLogTag::Dtls,
            WorkerLogTag::Rtp,
            WorkerLogTag::Srtp,
            WorkerLogTag::Rtcp,
            WorkerLogTag::Rtx,
            WorkerLogTag::Bwe,
            WorkerLogTag::Score,
            WorkerLogTag::Simulcast,
            WorkerLogTag::Svc,
            WorkerLogTag::Sctp,
            WorkerLogTag::Message,
        ];
        settings.enable_liburing = false;
        settings
    }

    /// Return the configured listening IP address for RTC.
    fn listen_ip_address(&self) -> IpAddr {
        return self.config.rtc_listen_ip;
    }

    /// Return the announced address (if any) used by peers to reach this SFU.
    fn announced_address(&self) -> Option<String> {
        if !self.instance_meta.external_ip.is_empty() {
            Some(self.instance_meta.external_ip.clone())
        } else {
            None
        }
    }

    /// Options used to create a new participant transport.
    ///
    /// This configures the transport to use the UDP protocol, sets the IP
    /// address to listen on, and specifies a port range derived from the
    /// configured RTC port range. Both UDP and TCP are enabled and preferred
    /// for transport negotiation flexibility.
    pub fn participant_transport_options(&self) -> WebRtcTransportOptions {
        let port_range = std::ops::RangeInclusive::new(self.config.rtc_port_range_start, self.config.rtc_port_range_end);
        let mut options =
            WebRtcTransportOptions::new(WebRtcTransportListenInfos::new(ListenInfo {
                protocol: Protocol::Udp,
                ip: self.listen_ip_address(),
                announced_address: self.announced_address(),
                port: None,
                port_range: Some(port_range),
                flags: None,
                send_buffer_size: None,
                recv_buffer_size: None,
            }));
        options.enable_udp = true;
        options.prefer_udp = true;
        options.enable_tcp = true;
        options.prefer_tcp = true;
        options
    }

    /// Options used to create a new plain transport for producers.
    ///
    /// This sets up a transport configuration with UDP protocol, localhost IP
    /// address, and disables RTCP mux and comedia options. The port range is
    /// left unspecified, as it will be determined by the system.
    pub fn producer_plain_transport_options(&self, port_range: Option<RangeInclusive<u16>>) -> PlainTransportOptions {
        // let port_range = self.config.rtc_port_range_start..self.config.rtc_port_range_end;
        let mut options =
            PlainTransportOptions::new(ListenInfo {
                protocol: Protocol::Udp,
                // ip: self.listen_ip_address(),
                ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                announced_address: None,
                port: None,
                port_range: None,
                flags: None,
                send_buffer_size: None,
                recv_buffer_size: None,
            });
        options.rtcp_mux = false;
        options.comedia = false;
        options
    }

    /// Supported media codecs for producers in this SFU.
    ///
    /// Currently configured with Opus audio codec and VP8 video
    /// codecs. Each codec configuration includes parameters like payload type,
    /// clock rate, and optional RTCP feedback types.
    pub fn media_codecs() -> Vec<RtpCodecCapability> {
        vec![
            RtpCodecCapability::Audio {
                mime_type: MimeTypeAudio::Opus,
                preferred_payload_type: Some(111),
                clock_rate: NonZeroU32::new(48000).unwrap(),
                channels: NonZeroU8::new(2).unwrap(),
                parameters: RtpCodecParametersParameters::from([(
                    "useinbandfec",
                    RtpCodecParametersParametersValue::Number(1u32),
                )]),
                rtcp_feedback: vec![RtcpFeedback::TransportCc],
            },
            /*RtpCodecCapability::Video {
                mime_type: MimeTypeVideo::H264,
                preferred_payload_type: Some(125),
                clock_rate: NonZeroU32::new(90000).unwrap(),
                parameters: RtpCodecParametersParameters::from([(
                    "level-asymmetry-allowed",
                    1_u32.into(),
                ), (
                    "profile-level-id",
                    "42e01f".to_string().into(),
                )]),
                rtcp_feedback: vec![
                    RtcpFeedback::Nack,
                    RtcpFeedback::NackPli,
                    RtcpFeedback::CcmFir,
                    RtcpFeedback::GoogRemb,
                    RtcpFeedback::TransportCc,
                ],
            },*/
            RtpCodecCapability::Video {
                mime_type: MimeTypeVideo::Vp8,
                preferred_payload_type: Some(96),
                clock_rate: NonZeroU32::new(90000).unwrap(),
                parameters: RtpCodecParametersParameters::default(),
                rtcp_feedback: vec![
                    RtcpFeedback::Nack,
                    RtcpFeedback::NackPli,
                    RtcpFeedback::CcmFir,
                    RtcpFeedback::GoogRemb,
                    RtcpFeedback::TransportCc,
                ],
            },
            /*RtpCodecCapability::Video {
                mime_type: MimeTypeVideo::Vp9,
                preferred_payload_type: Some(98),
                clock_rate: NonZeroU32::new(90000).unwrap(),
                parameters: RtpCodecParametersParameters::default(),
                rtcp_feedback: vec![
                    RtcpFeedback::Nack,
                    RtcpFeedback::NackPli,
                    RtcpFeedback::CcmFir,
                    RtcpFeedback::GoogRemb,
                    RtcpFeedback::TransportCc,
                ],
            },
            RtpCodecCapability::Video {
                mime_type: MimeTypeVideo::H264Svc,
                preferred_payload_type: Some(125),
                clock_rate: NonZeroU32::new(90000).unwrap(),
                parameters: RtpCodecParametersParameters::from([(
                    "level-asymmetry-allowed",
                    1_u32.into(),
                )]),
                rtcp_feedback: vec![
                    RtcpFeedback::Nack,
                    RtcpFeedback::NackPli,
                    RtcpFeedback::CcmFir,
                    RtcpFeedback::GoogRemb,
                    RtcpFeedback::TransportCc,
                ],
            },
            RtpCodecCapability::Video {
                mime_type: MimeTypeVideo::H265,
                preferred_payload_type: Some(126),
                clock_rate: NonZeroU32::new(90000).unwrap(),
                parameters: RtpCodecParametersParameters::from([(
                    "level-asymmetry-allowed",
                    1_u32.into(),
                )]),
                rtcp_feedback: vec![
                    RtcpFeedback::Nack,
                    RtcpFeedback::NackPli,
                    RtcpFeedback::CcmFir,
                    RtcpFeedback::GoogRemb,
                    RtcpFeedback::TransportCc,
                ],
            },*/
        ]
    }
    /// Create the inner shared state container for the SFU.
    ///
    /// This sets up the worker manager, creates the initial workers, writes
    /// the container and worker heartbeat records to the database, and spawns
    /// the periodic container heartbeat updater thread.
    async fn create_inner(sfu_config: SFUConfig, pool: Pool, boot_timestamp: u128,
                              instance_meta: InstanceMeta, tenant: String, client: AnalyticMetricClient) -> Result<Inner, String> {
        let num_cores = num_cpus::get() as i32;
        let worker_manager = WorkerManager::new();
        let mut workers_map = HashMap::default();
        // let machine_uid = machine_uid::get().unwrap().to_string();
        let machine_uid = instance_meta.internal_ip.clone();
        let local_ip = instance_meta.internal_ip.clone();
        info!("Initialising Mediasoup SFU");
        info!(
            "Instance configuration and computed settings | Instance Cores: {} ",
            num_cores
        );
        //need to check if we need to have some free cores
        for i in 1..=2 {
            info!("Creating worker # {}", i);
            let worker_instance = match worker_manager
                .create_worker(Self::worker_settings(sfu_config.clone()))
                .await
            {
                Ok(w) => w,
                Err(e) => panic!("Failed to create worker: {}.", e),
            };
            workers_map.insert(worker_instance.id().clone().to_string(), worker_instance.clone());
        }
        //TODO: Write an interface to store the data, for now hard coding write to MySql
        let mut entities: Vec<Box<dyn dao::Dao>> = Vec::new();
        let container_heartbeat = ContainerHeartbeat {
            mutation_type: Insert,
            container_id: machine_uid.clone() + "-" + boot_timestamp.to_string().as_str(),
            heartbeat: Some(boot_timestamp),
            host: Some(local_ip.to_string()),
            tenant: Some(tenant),
        };
        entities.push(Box::new(container_heartbeat));
        for worker in workers_map.clone() {
            let worker_container = WorkerContainer {
                mutation_type: Insert,
                worker_id: worker.0.clone().to_string(),
                container_id: machine_uid.clone() + "-" + boot_timestamp.to_string().as_str(),
            };
            entities.push(Box::new(worker_container));
        }
        let mysql_context = Arc::new(MySqlContext {
            pool: pool.clone(),
            rollback_on_duplicate: true,
            tx_opts: TxOpts::default(),
        });
        let mysql = MySql {};

        match mysql.mutate(entities, mysql_context.clone()) {
            Ok(o) => info!("Success in writing to DB"),
            Err(e) => {
                error!("Error in writing to DB {}", e);
                return Err(format!("Error in writing to DB {}", e));
            }
        }
        let mysql_context_arc_clone = Arc::clone(&mysql_context);
        thread::spawn(move || {
            loop {
                let current_timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
                let container_heartbeat = ContainerHeartbeat {
                    mutation_type: Update,
                    container_id: machine_uid.clone() + "-" + boot_timestamp.to_string().as_str(),
                    heartbeat: Some(current_timestamp),
                    host: None,
                    tenant: None,
                };
                let entities: Vec<Box<dyn dao::Dao>> = vec![Box::new(container_heartbeat)];
                match mysql.update(entities, mysql_context_arc_clone.clone()) {
                    Ok(o) => {
                        //info!("Success in writing to DB")
                    }
                    Err(e) => {
                        error!("Error in writing to DB {}", e);
                    }
                }
                thread::sleep(Duration::from_secs(5));
            }
        });
        let metrics_pool = Builder::new_multi_thread()
            .worker_threads(10) // Define the number of worker threads
            .enable_all() // Enable all Tokio features (like timers, I/O, etc.)
            .build()
            .unwrap();
        let recordings_pool = Builder::new_multi_thread()
            .worker_threads(10) // Define the number of worker threads
            .enable_all() // Enable all Tokio features (like timers, I/O, etc.)
            .build()
            .unwrap();
        gstreamer::init().expect("Failed to initialize GStreamer");
        let shared_clock = gstreamer::SystemClock::obtain();

        let user_channel = UserChannel::new(sfu_config.user_event_channel_count);
        let mut shards = Vec::with_capacity(sfu_config.consumer_shard_count);
        for _ in 0..sfu_config.consumer_shard_count {
            shards.push(Arc::new(RwLock::new(HashMap::new())));
        }


        Ok(Inner {
            workers_map: Mutex::new(workers_map),
            routers_map: Arc::new(RwLock::new(HashMap::default())),
            transport_map: Arc::new(RwLock::new(HashMap::default())),
            producer_map: Arc::new(RwLock::new(HashMap::default())),
            consumer_map_shards: shards,
            plain_transport_map: Arc::new(RwLock::new(HashMap::default())),
            plain_transport_consumer_map: Arc::new(RwLock::new(HashMap::default())),
            pipeline_map: Arc::new(RwLock::new(HashMap::default())),
            lock_map: Arc::new(RwLock::new(HashMap::default())),
            boot_timestamp,
            instance_meta,
            metrics_pool,
            recordings_pool,
            shared_clock,
            user_channel,
            client: Arc::new(client),
        })
    }

    /// Acquire a free UDP port for video recording.
    ///
    /// Scans the configured plain transport video port range and attempts to
    /// bind a UDP socket to each port until a free one is found or the range
    /// is exhausted.
    pub async fn get_free_video_port(&mut self) -> Result<u16, String> {
        for port in (self.config.plain_transport_video_port_range_start..self.config.plain_transport_video_port_range_end).step_by(2) {
            if UdpSocket::bind(("127.0.0.1", port)).await.is_ok() {
                return Ok(port);
            }
        }
        Err("No free ports available for recording".to_string())
    }

    /// Acquire a free UDP port for audio recording.
    ///
    /// Scans the configured plain transport audio port range and attempts to
    /// bind a UDP socket to each port until a free one is found or the range
    /// is exhausted.
    pub async fn get_free_audio_port(&mut self) -> Result<u16, String> {
        for port in (self.config.plain_transport_audio_port_range_start..self.config.plain_transport_audio_port_range_end).step_by(2) {
            if UdpSocket::bind(("127.0.0.1", port)).await.is_ok() {
                return Ok(port);
            }
        }
        Err("No free ports available for recording".to_string())
    }
    /// Create routers on available workers for a batch of meetings.
    ///
    /// This will delete existing router-meeting associations and create new
    /// ones based on the provided `meeting_map`. It requires a writable DB
    /// connection pool to persist the changes.
    pub async fn batch_create_router_on_worker(
        &mut self,
        meeting_map: HashMap<String, MeetingPayload>,
        pool: Pool,
    ) -> Result<String, String>
    {
        let worker_map = self.inner.workers_map.lock().await.clone();
        let mut router_worker_entities: Vec<Box<RouterWorker>> = Vec::new();
        let mut meeting_status_entities: Vec<Box<MeetingStatus>> = Vec::new();
        let mut router_meeting_entities: Vec<Box<RouterMeeting>> = Vec::new();
        let mut router_map_temp: HashMap<String, RouterInMemory> = HashMap::default();
        for (key, value) in meeting_map.iter() {
            let meeting_status_clear = MeetingStatus {
                mutation_type: Delete,
                meeting_id: key.clone(),
                enabled: None,
            };
            info!("delete string for meeting_status {}", meeting_status_clear.get_delete_string().unwrap());
            meeting_status_entities.push(Box::new(meeting_status_clear));
            let meeting_status = MeetingStatus {
                mutation_type: Insert,
                meeting_id: key.clone(),
                enabled: Some(true),
            };
            meeting_status_entities.push(Box::new(meeting_status));
            for (worker_id, worker_payload) in value.worker_payload.iter() {
                let worker = worker_map.get(worker_id).unwrap();
                let router = match worker.create_router(RouterOptions::new(Self::media_codecs())).await
                {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Failed to create router: {}", e);
                        return Err(format!("Failed to create router {}", e));
                    }
                };
                if worker_payload.capacity.is_some() {
                    let router_in_memory = RouterInMemory {
                        router: router.clone(),
                        capacity: worker_payload.capacity.unwrap(),
                    };
                    router_map_temp.insert(router.id().clone().to_string(), router_in_memory);
                } else if worker_payload.producer_capacity.is_some() {
                    let router_in_memory = RouterInMemory {
                        router: router.clone(),
                        capacity: worker_payload.producer_capacity.unwrap(),
                    };
                    router_map_temp.insert(router.id().clone().to_string(), router_in_memory);
                } else if worker_payload.consumer_capacity.is_some() {
                    let router_in_memory = RouterInMemory {
                        router: router.clone(),
                        capacity: worker_payload.consumer_capacity.unwrap(),
                    };
                    router_map_temp.insert(router.id().clone().to_string(), router_in_memory);
                }
                let router_worker = RouterWorker {
                    mutation_type: Insert,
                    worker_id: Some(worker_id.to_string()),
                    router_id: router.id().clone().to_string(),
                };
                info!("insert string for router_worker {}", router_worker.get_insert_string().unwrap());
                router_worker_entities.push(Box::new(router_worker));
                let router_meeting = RouterMeeting {
                    mutation_type: Insert,
                    router_id: Some(router.id().to_string()),
                    meeting_id: key.clone(),
                    producer_capacity: worker_payload.producer_capacity,
                    consumer_capacity: worker_payload.consumer_capacity,
                    capacity: worker_payload.capacity,
                    container_id: Some(value.new_container_id.clone()),
                };
                info!("insert string for router_meeting {}", router_meeting.get_insert_string().unwrap());
                router_meeting_entities.push(Box::new(router_meeting));
            }
        }
        let mysql = MySql {};
        let mut entities: Vec<Box<dyn dao::Dao>> = Vec::new();
        for t in meeting_status_entities {
            entities.push(t);
        }
        for t in router_worker_entities {
            entities.push(t);
        }
        for t in router_meeting_entities {
            entities.push(t);
        }

        let tx_opts = TxOpts::default()
            .set_isolation_level(Some(IsolationLevel::Serializable))
            .set_with_consistent_snapshot(false)
            .set_access_mode(Some(AccessMode::ReadWrite));

        let mysql_context = MySqlContext {
            pool,
            rollback_on_duplicate: true,
            tx_opts: tx_opts.clone(),
        };

        info!("Updating {} queries", entities.len());
        match mysql.mutate(entities, Arc::from(mysql_context)) {
            Ok(o) => {
                info!("Success in writing to DB");
                self.inner.routers_map.write().await.extend(router_map_temp);
                Ok("created the required routers".to_string())
            }
            Err(e) => {
                // Not persisting the routers in the map will lead to drop in the creation of routers
                error!("Error in writing to DB {}", e);
                return Err(format!("Error in writing to DB {}", e));
            }
        }
    }
    /// Create a new meeting and allocate routers on workers based on desired
    /// capacity.
    ///
    /// This will create a new router for each worker and link them to the
    /// meeting. The router capacity is adjusted based on the provided
    /// `worker_router_capacity` map. Persistent storage is updated with the
    /// new meeting and router associations.
    pub async fn create_meeting_and_routers_on_workers(
        &mut self,
        worker_router_capacity: HashMap<String, Vec<ProducerConsumerCapacity>>,
        meeting_id: String,
        container_id: String,
        pool: Pool,
    ) -> Result<String, String> {
        let worker_map = self.inner.workers_map.lock().await.clone();

        let mut router_worker_entities: Vec<Box<RouterWorker>> = Vec::new();
        let mut router_meeting_entities: Vec<Box<RouterMeeting>> = Vec::new();
        let mut router_map_temp: HashMap<String, RouterInMemory> = HashMap::default();
        let mut producer_routers: Vec<String> = Vec::new();
        let mut consumer_routers: Vec<String> = Vec::new();
        let meeting_status = MeetingStatus {
            mutation_type: Insert,
            meeting_id: meeting_id.clone(),
            enabled: Some(true),
        };
        for worker_router_entry in worker_router_capacity.iter() {
            let worker_id = worker_router_entry.0;
            let worker = worker_map.get(worker_id).unwrap();
            for entity in worker_router_entry.1
            {
                let router = match worker
                    .create_router(RouterOptions::new(Self::media_codecs())).await
                {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Failed to create router: {}", e);
                        return Err(format!("Failed to create router {}", e));
                    }
                };
                let router_worker = RouterWorker {
                    mutation_type: Insert,
                    worker_id: Some(worker_id.to_string()),
                    router_id: router.id().clone().to_string(),
                };
                let router_meeting = RouterMeeting {
                    mutation_type: Insert,
                    router_id: Some(router.id().to_string()),
                    meeting_id: meeting_id.clone(),
                    producer_capacity: entity.producer_capacity,
                    consumer_capacity: entity.consumer_capacity,
                    capacity: entity.capacity,
                    container_id: Some(container_id.clone()),
                };
                if entity.capacity.is_some() {
                    router_map_temp.insert(router.id().clone().to_string(), RouterInMemory {
                        router: router.clone(),
                        capacity: entity.capacity.unwrap(),
                    });
                } else if entity.producer_capacity.is_some() {
                    router_map_temp.insert(router.id().clone().to_string(), RouterInMemory {
                        router: router.clone(),
                        capacity: entity.producer_capacity.unwrap(),
                    });
                } else if entity.consumer_capacity.is_some() {
                    router_map_temp.insert(router.id().clone().to_string(), RouterInMemory {
                        router: router.clone(),
                        capacity: entity.consumer_capacity.unwrap(),
                    });
                }
                router_worker_entities.push(Box::new(router_worker));
                router_meeting_entities.push(Box::new(router_meeting));
                if entity.producer_capacity.is_some() {
                    producer_routers.push(router.id().clone().to_string());
                } else if entity.consumer_capacity.is_some() {
                    consumer_routers.push(router.id().clone().to_string());
                }
            }
        }
        let mysql = MySql {};
        let mut entities: Vec<Box<dyn dao::Dao>> = Vec::new();
        entities.push(Box::new(meeting_status));
        for t in router_worker_entities {
            entities.push(t);
        }
        for t in router_meeting_entities {
            entities.push(t);
        }
        let mysql_context = MySqlContext {
            pool,
            rollback_on_duplicate: true,
            tx_opts: TxOpts::default(),
        };
        match mysql.mutate(entities, Arc::from(mysql_context)) {
            Ok(o) => {
                info!("Success in writing to DB");
                {
                    let mut lock = self.inner.routers_map.write().await;
                    lock.extend(router_map_temp);
                }
                Ok("created the required routers".to_string())
            }
            Err(e) => {
                match e {
                    DBError::MySqlError(e) => {
                        if e.code == 1062 {
                            Ok("Meeting already exists".to_string())
                        } else {
                            error!("Error in writing to DB {}", e);
                            Err(format!("Error in writing to DB {}", e))
                        }
                    }
                    ref DBError => {
                        error!("Error in writing to DB {}", e);
                        Err(format!("Error in writing to DB {}", e))
                    }
                }
            }
        }
    }

    /// Retrieve the finalized RTP capabilities for a given router.
    ///
    /// This queries the in-memory router map and returns the RTP capabilities
    /// which describe the supported codecs and formats for media negotiation.
    pub async fn get_router_capabilities(
        &mut self,
        router_id: String,
    ) -> RtpCapabilitiesFinalized {
        let router_map = self.inner.routers_map.read().await.clone();
        router_map.get(router_id.as_str()).unwrap().router.rtp_capabilities().clone()
    }

    /// Recreate the producer transport and associated resources for a participant.
    ///
    /// This will remove the existing transport and create a new one, updating
    /// the database and in-memory state. It can optionally preserve the existing
    /// consumer transport if `new_user` is true.
    pub async fn recreate_producer_transport_helper(&mut self, pool: Pool,
                                                    participant_id: String,
                                                    meeting_id: String,
                                                    instance_id: String,
                                                    router_id: String,
                                                    consumer_router_id: Option<String>,
                                                    meter: Meter,
                                                    old_producer_transport_id: Option<String>,
                                                    old_consumer_transport_id: Option<String>,
    ) -> Result<RecreateProducerTransportResponse, String> {

        let key_mutex = {
            let mut lock_map = self.inner.lock_map.write().await;
            lock_map.entry(participant_id.clone()).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
        };

        let guard = key_mutex.lock().await;

        let new_user: bool;
        match self.remove_and_add_participant_to_meeting(pool.clone(),
                                                           participant_id.clone(),
                                                           meeting_id.clone(),
                                                           instance_id.clone(),
                                                           Some(router_id.clone()),
                                                           None).await {
            Ok(o) => (
                new_user = o.1,
            ),
            Err(e) => return Err(format!("Failed to add participant to meeting {}", e))
        };

        let mut producer_transport_id = None;
        let mut consumer_transport_id = None;
        if new_user {
            producer_transport_id = old_producer_transport_id;
            consumer_transport_id = old_consumer_transport_id;
        }

        self.recreate_producer_transport(participant_id.clone(),
                                         meeting_id.clone(),
                                         producer_transport_id,
                                         consumer_transport_id,
                                         router_id.clone(),
                                         instance_id.clone(),
                                         new_user,
                                         pool.clone(),
                                         meter).await

    }

    /// Recreate the consumer transport and associated resources for a participant.
    ///
    /// This will remove the existing transport and create a new one, updating
    /// the database and in-memory state. It can optionally preserve the existing
    /// producer transport if `new_user` is true.
    pub async fn recreate_consumer_transport_helper(&mut self, pool: Pool,
                                                    participant_id: String,
                                                    meeting_id: String,
                                                    instance_id: String,
                                                    router_id: String,
                                                    consumer_router_id: Option<String>,
                                                    meter: Meter,
                                                    old_producer_transport_id: Option<String>,
                                                    old_consumer_transport_id: Option<String>,
    ) -> Result<CreateTransportResponse, String> {
        let key_mutex = {
            let mut lock_map = self.inner.lock_map.write().await;
            lock_map.entry(participant_id.clone()).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
        };

        let guard = key_mutex.lock().await;

        let new_user: bool;
        match self.remove_and_add_participant_to_meeting(pool.clone(),
                                                         participant_id.clone(),
                                                         meeting_id.clone(),
                                                         instance_id.clone(),
                                                         None, Some(router_id.clone())).await {
            Ok(o) => (
                new_user = o.1,
            ),
            Err(e) => return Err(format!("Failed to add participant to meeting {}", e))
        };
        let mut producer_transport_id = None;
        let mut consumer_transport_id = None;
        if new_user {
            producer_transport_id = old_producer_transport_id;
            consumer_transport_id = old_consumer_transport_id;
        }

        let consumer_transport_response = match self.recreate_consumer_transport(participant_id.clone(),
                                                                                   router_id.clone(),
                                                                                   meeting_id.clone(),
                                                                                   instance_id.clone(),
                                                                                   producer_transport_id,
                                                                                   consumer_transport_id,
                                                                                   new_user,
                                                                                   pool.clone(),
                                                                                   meter).await {
            Ok(o) => {
                o
            },
            Err(e) => return Err(format!("Failed to recreate consumer transport {}", e))
        };
        match self.get_producers_of_meeting_excluding_participant(meeting_id.clone(), participant_id.clone(),
                                                              pool.clone()).await {
            Ok(o) => Ok(CreateTransportResponse {
                transport_options: TransportOptions {
                    id: consumer_transport_response.id.clone(),
                    dtls_parameters: consumer_transport_response.dtls_parameters.clone(),
                    ice_candidates: consumer_transport_response.ice_candidates.clone(),
                    ice_parameters: consumer_transport_response.ice_parameters.clone(),
                },
                producer_details: o.producer_details,
            }),
            Err(e) => Err(format!("Failed to get producers of meeting {}", e))
        }
    }

    /// Remove a participant from a meeting and add them back, updating their
    /// router associations.
    ///
    /// This is used to change a participant's router or recover from errors.
    /// It deletes and recreates the participant-meeting association in the DB.
    /// Returns whether the participant is new to the meeting.
    pub async fn remove_and_add_participant_to_meeting(&mut self, pool: Pool,
                                                       participant_id: String,
                                                       meeting_id: String,
                                                       instance_id: String,
                                                       producer_router_id: Option<String>,
                                                       consumer_router_id: Option<String>) -> Result<(String, bool), String> {
        let mut entities: Vec<Box<dyn Dao>> = Vec::new();
        let participant_meeting_delete_custom = ParticipantMeeting {
            mutation_type: CustomQuery1,
            meeting_id: meeting_id.clone(),
            participant_id: Some(participant_id.clone()),
            instance_id: Some(instance_id.clone()),
        };
        entities.push(Box::new(participant_meeting_delete_custom));
        let participant_meeting_update = ParticipantMeeting {
            mutation_type: Insert,
            meeting_id: meeting_id.clone(),
            participant_id: Some(participant_id.clone()),
            instance_id: Some(instance_id.clone()),
        };
        entities.push(Box::new(participant_meeting_update));
        if producer_router_id.is_some() {
            let participant_producer_router = ParticipantProducerRouter {
                mutation_type: Insert,
                router_id: producer_router_id.clone(),
                participant_id: participant_id.clone(),
            };
            entities.push(Box::new(participant_producer_router));
        }
        if consumer_router_id.is_some() {
            let participant_consumer_router = ParticipantConsumerRouter {
                mutation_type: Insert,
                router_id: consumer_router_id.clone(),
                participant_id: participant_id.clone(),
            };
            entities.push(Box::new(participant_consumer_router));
        }
        let mysql = MySql {};
        let mysql_context = MySqlContext {
            pool,
            rollback_on_duplicate: false,
            tx_opts: TxOpts::default(),
        };
        match mysql.mutate(entities, Arc::from(mysql_context)) {
            Ok(o) => {
                self.push_user_session_event(participant_id.clone(), meeting_id.clone(), "JOIN".to_string(), instance_id.clone());
                info!("Success in writing to DB");
                Ok(("Success in writing to DB".to_string(), !o.1))
            }
            Err(e) => {
                error!("Error in writing to DB {}", e);
                Err(format!("Error in writing to DB {}", e))
            }
        }
    }

    /// Send a user session event (JOIN/LEAVE) to the analytic client.
    ///
    /// This publishes an event indicating that a user has joined or left a
    /// meeting. It includes metadata like `participant_id`, `meeting_id`,
    /// `event_type` and `instance_id`.
    pub fn push_user_session_event(&mut self,
                                   participant_id: String,
                                   meeting_id: String,
                                   event_type: String,
                                   instance_id: String) {

        // Send Leave Event
        let current_time = Utc::now().timestamp_millis();
        match self.inner.user_channel.tx.send(UserEvent {
            event_id: Uuid::new_v4(),
            user_id: participant_id.clone(),
            event_type: event_type.clone(),
            event_time: current_time.clone(),
            instance_id: instance_id.clone(),
            meeting_id: meeting_id.clone(),
        }) {
            Ok(_) => {
                info!("User event sent successfully for user: {} meetingID: {}", participant_id.clone(), meeting_id.clone());
            }
            Err(e) => {
                error!("Failed to send user event: for user: {} meetingID: {} err {}", participant_id.clone(), meeting_id.clone(), e);
            }
        }
    }

    /// Query the database for all producers belonging to a meeting.
    ///
    /// This will return the current set of producers (audio/video) active in
    /// the specified meeting, including their participant associations.
    pub async fn get_producers_of_meeting(&mut self, meeting_id: String, pool: Pool) -> Result<ProducerResponse, String> {
        let participant_meeting = ParticipantProducers {
            mutation_type: MutationType::Query,
            participant_id: None,
            producers: None,
            kind: None,
            producer_id: None,
            meeting_id: Some(meeting_id.clone()),
            transport_id: None,
        };
        let mysql = MySql {};
        let mysql_context = MySqlContext {
            pool,
            rollback_on_duplicate: true,
            tx_opts: TxOpts::default(),
        };
        match mysql.query(Box::new(participant_meeting), Arc::from(mysql_context)) {
            Ok(rows) => {
                let mut response: Vec<ProducerDetail> = Vec::new();
                for row in rows {
                    let producer_id = row.get(1).unwrap();
                    let participant_id = row.get(0).unwrap();
                    let kind = row.get(2).unwrap();
                    response.push(ProducerDetail {
                        participant_id,
                        producer_id,
                        kind,
                    });
                }
                Ok(ProducerResponse { producer_details: response })
            }
            Err(e) => {
                error!("Error in querying from DB {}", e);
                Err(format!("Error in writing from DB {}", e))
            }
        }
    }

    /// Query the database for all producers in a meeting, excluding a specific
    /// participant.
    ///
    /// This is useful to determine what media streams a participant is not
    /// currently receiving, so that new transports can be created with the
    /// correct producer associations.
    pub async fn get_producers_of_meeting_excluding_participant(&mut self, meeting_id: String, participant_id: String, pool: Pool) -> Result<ProducerResponse, String> {
        let participant_meeting = ParticipantProducers {
            mutation_type: MutationType::CustomQuery2,
            participant_id: Some(participant_id.clone()),
            producers: None,
            kind: None,
            producer_id: None,
            meeting_id: Some(meeting_id.clone()),
            transport_id: None,
        };
        let mysql = MySql {};
        let mysql_context = MySqlContext {
            pool,
            rollback_on_duplicate: false,
            tx_opts: TxOpts::default(),
        };
        match mysql.query(Box::new(participant_meeting), Arc::from(mysql_context)) {
            Ok(rows) => {
                let mut response: Vec<ProducerDetail> = Vec::new();
                for row in rows {
                    let producer_id = row.get(1).unwrap();
                    let participant_id = row.get(0).unwrap();
                    let kind = row.get(2).unwrap();
                    response.push(ProducerDetail {
                        participant_id,
                        producer_id,
                        kind,
                    });
                }
                Ok(ProducerResponse { producer_details: response })
            }
            Err(e) => {
                error!("Error in querying from DB {}", e);
                Err(format!("Error in writing from DB {}", e))
            }
        }
    }

    /// Create a new WebRTC transport for a participant.
    ///
    /// This sets up a transport using the configured participant transport options
    /// and the router's listening IP/port. The transport is used for sending
    /// and receiving media packets.
    pub async fn create_transport(&mut self, router_id: String) -> Result<WebRtcTransport, String> {
        let router_map = self.inner.routers_map.read().await.clone();
        let router = router_map.get(router_id.as_str()).unwrap().router.clone();
        let transport_options = self.participant_transport_options();
        match router
            .create_webrtc_transport(transport_options.clone())
            .await
        {
            Ok(transport) => {
                Ok(transport)
            }
            Err(e) => {
                error!("Failed to create producer transport: {}", e);
                return Err(format!("Failed to create producer transport: {}", e));
            }
        }
    }

    pub async fn create_producer_transport(
        &mut self,
        router_id: String,
        participant_id: String,
        meeting_id: String,
        old_producer_transport_id_option: Option<String>,
        old_consumer_transport_id_option: Option<String>,
        pool: Pool,
    ) -> Result<TransportOptions, String> {
        let producer_transport = match self.create_transport(router_id).await
        {
            Ok(transport) => transport,
            Err(e) => {
                error!("Failed to create producer transport: {}", e);
                return Err(format!("Failed to create producer transport: {}", e));
            }
        };
        info!(
            "Created a producer transport: {}",
            producer_transport.id(),
        );
        let _ = producer_transport
            .set_max_incoming_bitrate(self.config.max_incoming_bitrate.clone())
            .await
            .map_err(|e| {
                error!(
                    "Failed to set max incoming bitrate on producer transport: {}, error: {}",
                    producer_transport.id(),
                    e
                )
            });
        let _ = producer_transport
            .set_min_outgoing_bitrate(self.config.min_outgoing_bitrate.clone())
            .await
            .map_err(|e| {
                error!(
                    "Failed to set min outgoing bitrate on producer transport: {}, error: {}",
                    producer_transport.id(),
                    e
                )
            });
        let _ = producer_transport
            .set_max_outgoing_bitrate(self.config.max_outgoing_bitrate.clone())
            .await
            .map_err(|e| {
                error!(
                    "Failed to set max outgoing bitrate on producer transport: {}, error: {}",
                    producer_transport.id(),
                    e
                )
            });
        let producer_transport_clone = producer_transport.clone();
        let transport_in_memory = TransportInMemory {
            transport: producer_transport.clone(),
            meeting_id: meeting_id.clone(),
            participant_id: participant_id.clone(),
            kind: "producer".to_string(),
        };
        let mut mysql_entities: Vec<Box<dyn dao::Dao>> = Vec::new();
        let new_participant_producer_transport = ParticipantProducerTransport {
            mutation_type: Insert,
            participant_id: participant_id.clone(),
            transport_id: Some(producer_transport.id().to_string()),
        };
        mysql_entities.push(Box::new(new_participant_producer_transport));
        let mysql = MySql {};
        let mysql_context = MySqlContext {
            pool,
            rollback_on_duplicate: true,
            tx_opts: TxOpts::default(),
        };
        match mysql.mutate(mysql_entities, Arc::from(mysql_context)) {
            Ok(o) => {
                info!("Success in writing to DB");
                {
                    let mut mutable_transport_map = self.inner.transport_map.write().await;
                    if let Some(old_transport_id) = old_producer_transport_id_option {
                        mutable_transport_map.remove(old_transport_id.as_str());
                    }
                    if let Some(old_transport_id) = old_consumer_transport_id_option {
                        mutable_transport_map.remove(old_transport_id.as_str());
                    }
                    mutable_transport_map.insert(producer_transport.id().to_string(), transport_in_memory);
                }
                Ok(TransportOptions {
                    id: producer_transport_clone.id(),
                    dtls_parameters: producer_transport_clone.dtls_parameters().clone(),
                    ice_candidates: producer_transport_clone.ice_candidates().clone(),
                    ice_parameters: producer_transport_clone.ice_parameters().clone(),
                })
            }
            Err(e) => {
                // handle duplicate entry error
                if let DBError::MySqlError(ref mysql_error) = e {
                    if mysql_error.code == 1062 {
                        info!("Duplicate entry for participant transport, ignoring");
                        let transport_map = self.inner.transport_map.read().await;
                        let transport = transport_map.values().filter_map(|t| {
                            if t.participant_id == participant_id && t.meeting_id == meeting_id && t.kind == "producer" {
                                Some(t.transport.clone())
                            } else {
                                None
                            }
                        }).next();
                        if transport.is_none() {
                            error!("No transport found for participant {} in meeting {}, Unhandled case", participant_id, meeting_id);
                            return Err(format!("No transport found for participant {} in meeting {}", participant_id, meeting_id));
                        }
                        Ok(TransportOptions {
                            id: transport.clone().unwrap().id(),
                            dtls_parameters: transport.clone().unwrap().dtls_parameters().clone(),
                            ice_candidates: transport.clone().unwrap().ice_candidates().clone(),
                            ice_parameters: transport.clone().unwrap().ice_parameters().clone(),
                        })
                    } else {
                        error!("Error in writing to DB {}", e);
                        Err(format!("Error in writing to DB {}", e))
                    }
                } else {
                    error!("Error in writing to DB {}", e);
                    Err(format!("Error in writing to DB {}", e))
                }
            }
        }
    }

    pub async fn create_consumer_transport(
        &mut self,
        router_id: String,
        participant_id: String,
        meeting_id: String,
        old_producer_transport_id_option: Option<String>,
        old_consumer_transport_id_option: Option<String>,
        pool: Pool,
    ) -> Result<TransportOptions, String> {
        let consumer_transport = match self.create_transport(router_id).await
        {
            Ok(transport) => transport,
            Err(e) => {
                error!("Failed to create producer transport: {}", e);
                return Err(format!("Failed to create producer transport: {}", e));
            }
        };
        let _ = consumer_transport
            .set_max_incoming_bitrate(self.config.max_incoming_bitrate.clone())
            .await
            .map_err(|e| {
                error!(
                    "Failed to set max incoming bitrate on consumer transport: {}, error: {}",
                    consumer_transport.id(),
                    e
                )
            });
        let _ = consumer_transport
            .set_min_outgoing_bitrate(self.config.min_outgoing_bitrate.clone())
            .await
            .map_err(|e| {
                error!(
                    "Failed to set min outgoing bitrate on consumer transport: {}, error: {}",
                    consumer_transport.id(),
                    e
                )
            });
        let _ = consumer_transport
            .set_max_outgoing_bitrate(self.config.max_outgoing_bitrate.clone())
            .await
            .map_err(|e| {
                error!(
                    "Failed to set max outgoing bitrate on consumer transport: {}, error: {}",
                    consumer_transport.id(),
                    e
                )
            });
        let transport_in_memory = TransportInMemory {
            transport: consumer_transport.clone(),
            meeting_id: meeting_id.clone(),
            participant_id: participant_id.clone(),
            kind: "consumer".to_string(),
        };
        let mut mysql_entities: Vec<Box<dyn dao::Dao>> = Vec::new();
        mysql_entities.push(
            Box::new(ParticipantConsumerTransport {
                mutation_type: Insert,
                participant_id: participant_id.clone(),
                transport_id: Some(consumer_transport.id().to_string()),
            }));
        let mysql = MySql {};
        let mysql_context = MySqlContext {
            pool,
            rollback_on_duplicate: true,
            tx_opts: TxOpts::default(),
        };
        match mysql.mutate(mysql_entities, Arc::from(mysql_context)) {
            Ok(o) => {
                info!("Success in writing to DB");
                {
                    let mut mutable_transport_map = self.inner.transport_map.write().await;
                    if let Some(old_transport_id) = old_consumer_transport_id_option {
                        mutable_transport_map.remove(old_transport_id.as_str());
                    }
                    if let Some(old_transport_id) = old_producer_transport_id_option {
                        mutable_transport_map.remove(old_transport_id.as_str());
                    }
                    mutable_transport_map.insert(consumer_transport.id().to_string(), transport_in_memory);
                }
                Ok(TransportOptions {
                    id: consumer_transport.id(),
                    dtls_parameters: consumer_transport.dtls_parameters().clone(),
                    ice_candidates: consumer_transport.ice_candidates().clone(),
                    ice_parameters: consumer_transport.ice_parameters().clone(),
                })
            }
            Err(e) => {
                // handle duplicate error
                if let DBError::MySqlError(ref mysql_error) = e {
                    if mysql_error.code == 1062 {
                        info!("Duplicate entry for participant transport, ignoring");
                        let transport_map = self.inner.transport_map.read().await;
                        let transport = transport_map.values().filter_map(|t| {
                            if t.participant_id == participant_id && t.meeting_id == meeting_id && t.kind == "consumer" {
                                Some(t.transport.clone())
                            } else {
                                None
                            }
                        }).next();
                        if transport.is_none() {
                            error!("No transport found for participant {} in meeting {}, Unhandled case", participant_id, meeting_id);
                            return Err(format!("No transport found for participant {} in meeting {}", participant_id, meeting_id));
                        }
                        Ok(TransportOptions {
                            id: transport.clone().unwrap().id(),
                            dtls_parameters: transport.clone().unwrap().dtls_parameters().clone(),
                            ice_candidates: transport.clone().unwrap().ice_candidates().clone(),
                            ice_parameters: transport.clone().unwrap().ice_parameters().clone(),
                        })
                    } else {
                        error!("Error in writing to DB {}", e);
                        Err(format!("Error in writing to DB {}", e))
                    }
                } else {
                    error!("Error in writing to DB {}", e);
                    return Err(format!("Error in writing to DB {}", e));
                }
            }
        }
    }

    pub async fn connect_transport(
        &mut self,
        participant_id: String,
        transport_id: String,
        dtls_parameters: DtlsParameters,
    ) -> Result<String, String> {
        let web_rtc_transport_remote_parameters = WebRtcTransportRemoteParameters {
            dtls_parameters
        };
        let transport_map = {
            let lock = self.inner.transport_map.read().await;
            lock.clone()
        };
        if let Some(transport_in_memory) = transport_map.get(transport_id.as_str()) {
            match transport_in_memory.transport
                .connect(web_rtc_transport_remote_parameters).await
            {
                Ok(o) => {
                    info!("Acquired lock and connected producer transport");
                    Ok("Successfully connected producer transport".to_string())
                }
                Err(e) => {
                    // ignore already connect error
                    if let RequestError::Response { reason } = &e {
                        if reason.contains("connect() already called") {
                            info!("Transport already connected, ignoring");
                            return Ok("Transport already connected, ignoring".to_string());
                        }
                    }
                    error!("Failure in connecting producer transport {}", e);
                    Err(format!("Failure in connecting producer transport {}", e))
                }
            }
        } else {
            error!("Transport not found in memory");
            Err("Transport not found in memory".to_string())
        }
    }

    pub async fn restart_ice(&mut self, participant_id: String, transport_ids: Vec<String>) -> Result<RestartIceResponse, String> {
        let transport_map = {
            let lock = self.inner.transport_map.read().await;
            lock.clone()
        };
        let mut response_hash_map = HashMap::new();
        for transport_id in transport_ids {
            if let Some(transport_in_memory) = transport_map.get(transport_id.as_str()) {
                match transport_in_memory.transport
                    .restart_ice().await
                {
                    Ok(o) => {
                        info!("Acquired lock and restarted ice on transport");
                        response_hash_map.insert(transport_id.clone(), o);
                    }
                    Err(e) => {
                        error!("Failure in restarting ice on transport {}", e);
                    }
                }
            } else {
                error!("Transport not found in memory");
            }
        }
        Ok(RestartIceResponse {
            result: response_hash_map
        })
    }

    pub async fn recreate_producer_transport(
        &mut self,
        participant_id: String,
        meeting_id: String,
        old_producer_transport_id: Option<String>,
        old_consumer_transport_id: Option<String>,
        router_id: String,
        instance_id: String,
        new_user: bool,
        pool: Pool,
        meter: Meter) -> Result<RecreateProducerTransportResponse, String> {
        info!("[RecreateProducerTransport] Recreating producer transport for participant_id: {} meeting_id: {}", participant_id.clone(), meeting_id.clone());

        let old_consumers: Vec<String> = self.get_participant_consumers_excluding_instance(participant_id.clone(), meeting_id.clone(), instance_id.clone()).await;
        self.close_all_consumers_in_memory(participant_id.clone(), old_consumers, meeting_id.clone()).await;

        info!("[RecreateProducerTransport] Closed old consumers for participant_id: {} meeting_id: {}", participant_id.clone(), meeting_id.clone());

        let old_producer_ids_list: Vec<String> = self.get_participant_producers_excluding_instance(participant_id.clone(), meeting_id.clone(), instance_id.clone()).await;

        let close_consumer_ids_list: Vec<String> = self.get_consumers_for_producer_ids(old_producer_ids_list.clone(), meeting_id.clone()).await;

        let mut closed_producer_details = {
            if !old_producer_ids_list.is_empty() {
                match self.close_all_producers_in_memory(vec![participant_id.clone()], old_producer_ids_list, meeting_id.clone(), true).await {
                    Ok(o) => o,
                    Err(e) => {
                        error!("Failed to close all producers in memory {}", e);
                        return Err(format!("Failed to close all producers in memory {}", e));
                    }
                }
            } else {
                Vec::new()
            }
        };

        info!("[RecreateProducerTransport] Closed old producers for participant_id: {} meeting_id: {}", participant_id.clone(), meeting_id.clone());

        if !close_consumer_ids_list.is_empty() {
            self.close_all_consumers_in_memory("".to_string(), close_consumer_ids_list, meeting_id.clone()).await;
        }

        info!("[RecreateProducerTransport] Closed dependent consumers for participant_id: {} meeting_id: {}", participant_id.clone(), meeting_id.clone());

        match self.create_producer_transport(router_id.clone(),
                                             participant_id.clone(),
                                             meeting_id,
                                             old_producer_transport_id,
                                             old_consumer_transport_id, pool).await {
            Ok(o) => {
                info!("Successfully created producer transport");
                Ok(RecreateProducerTransportResponse {
                    transport_options: o,
                    closed_producer_details,
                })
            }
            Err(e) => {
                error!("Failed to create producer transport {}", e);
                Err(format!("Failed to create producer transport {}", e))
            }
        }
    }

    pub async fn recreate_consumer_transport(
        &mut self,
        participant_id: String,
        router_id: String,
        meeting_id: String,
        instance_id: String,
        old_producer_transport_id: Option<String>,
        old_consumer_transport_id: Option<String>,
        new_user: bool,
        pool: Pool,
        meter: Meter) -> Result<TransportOptions, String> {
        info!("[RecreateConsumerTransport] Recreating consumer transport for participant_id: {} meeting_id: {}", participant_id.clone(), meeting_id.clone());

        let old_consumers: Vec<String> = self.get_participant_consumers_excluding_instance(participant_id.clone(), meeting_id.clone(), instance_id.clone()).await;
        self.close_all_consumers_in_memory(participant_id.clone(), old_consumers, meeting_id.clone()).await;

        info!("[RecreateConsumerTransport] Closed old consumers for participant_id: {} meeting_id: {}", participant_id.clone(), meeting_id.clone());

        let old_producer_ids_list: Vec<String> = self.get_participant_producers_excluding_instance(participant_id.clone(), meeting_id.clone(), instance_id.clone()).await;
        let close_consumer_ids_list: Vec<String> = self.get_consumers_for_producer_ids(old_producer_ids_list.clone(), meeting_id.clone()).await;

        if !old_producer_ids_list.is_empty() {
            match self.close_all_producers_in_memory(vec![participant_id.clone()], old_producer_ids_list, meeting_id.clone(), true).await {
                Ok(o) => {
                    info!("Successfully closed all producers in memory for participant {}", participant_id.clone());
                }
                Err(e) => {
                    error!("Failed to close all producers in memory {}", e);
                    return Err(format!("Failed to close all producers in memory {}", e));
                }
            }
        }

        info!("[RecreateConsumerTransport] Closed old producers for participant_id: {} meeting_id: {}", participant_id.clone(), meeting_id.clone());

        if !close_consumer_ids_list.is_empty() {
            self.close_all_consumers_in_memory("".to_string(), close_consumer_ids_list, meeting_id.clone()).await;
        }

        info!("[RecreateConsumerTransport] Closed dependent consumers for participant_id: {} meeting_id: {}", participant_id.clone(), meeting_id.clone());

        match self.create_consumer_transport(router_id.clone(),
                                             participant_id.clone(),
                                             meeting_id,
                                             old_producer_transport_id,
                                             old_consumer_transport_id, pool).await {
            Ok(o) => Ok(o),
            Err(e) => {
                error!("Failed to create consumer transport {}", e);
                Err(format!("Failed to create consumer transport {}", e))
            }
        }
    }
    pub async fn start_recording(&mut self,
                                 meeting_id: String,
                                 participant_id: String,
                                 transport_id: String,
                                 shared_clock: Clock,
                                 meter: Meter,
                                 kind_enum: Kind) -> Result<(Gstreamer, u16), String> {
        let plain_transport_port: u16;

        let pipeline_failure_counter: Counter<u64> = meter
            .u64_counter("metric_".to_string() + meeting_id.clone().to_string().as_str() + "_pipeline_failures")
            .with_description("Counts the number of pipeline failures")
            .init();
        let pipeline_failure_counter_arc = Arc::new(pipeline_failure_counter.clone());
        let pipeline_failure_counter_clone = Arc::clone(&pipeline_failure_counter_arc);

        let gstreamer_from_map = {
            let temp_pipeline_map = self.inner.pipeline_map.read().await;
            temp_pipeline_map.get(&meeting_id).cloned()
        };
        let (mut gstreamer_new, exists) = match gstreamer_from_map {
            Some(gstreamer) => {
                info!("Gstreamer already exists for meeting_id {}", meeting_id);
                (gstreamer.unwrap(), true)
            }
            None => {
                info!("Creating new Gstreamer for meeting_id {}", meeting_id);
                let gstreamer_new = Gstreamer::new().await;
                (gstreamer_new, false)
            }
        };

        let gst: Result<Gstreamer, String> = {
            if kind_enum == Kind::Audio {
                plain_transport_port = match self.get_free_audio_port().await {
                    Ok(o) => o,
                    Err(e) => {
                        error!("Failed to get free audio port {}", e);
                        return Err(format!("Failed to get free audio port {}", e));
                    }
                };

                match gstreamer_new.clone().start_audio_recording_gstreamer(plain_transport_port as i32, meeting_id.clone(), participant_id.clone(), transport_id.clone(), shared_clock.clone(), meter).await {
                    Ok(gstreamer_self) => {
                        Ok(gstreamer_self)
                    }
                    Err(e) => {
                        error!("Failed to start recording {}", e);
                        Err(format!("Failed to start recording {}", e))
                    }
                }
            } else {
                plain_transport_port = match self.get_free_video_port().await {
                    Ok(o) => o,
                    Err(e) => {
                        error!("Failed to get free video port {}", e);
                        return Err(format!("Failed to get free video port {}", e));
                    }
                };
                match gstreamer_new.clone().start_vp8_video_recording_gstreamer(plain_transport_port as i32,
                                                                        meeting_id.clone(),
                                                                        participant_id.clone(),
                                                                        transport_id.clone(),
                                                                        shared_clock.clone(),
                                                                        meter.clone(), pipeline_failure_counter).await {
                    Ok(gstreamer_self) => {
                        //participant_producer_in_memory.pipeline = Some(pipeline.clone());
                        //pipeline.use_clock(Some(&shared_clock));
                        Ok(gstreamer_self)
                    }
                    Err(e) => {
                        error!("Failed to start recording {}", e);
                        Err(format!("Failed to start recording {}", e))
                    }
                }
            }
        };
        match gst {
            Ok(_) => {}
            Err(e) => {
                error!("Failed to start recording {}", e);
                return Err(format!("Failed to start recording {}", e));
            }
        }
        let gstreamer_self = gst?.clone();
        if !exists {
            let pipeline_arc = Arc::clone(gstreamer_self.pipeline.as_ref().unwrap());
            let pipeline_clone = Arc::clone(&pipeline_arc);
            std::thread::spawn( move || {
                let bus = pipeline_clone.lock().unwrap().bus().unwrap();
                for msg in bus.iter_timed(gstreamer::ClockTime::NONE) {
                    match msg.view() {
                        gstreamer::MessageView::Eos(_) => {
                            info!("Pipeline EOS reached.");
                            match pipeline_clone.lock().unwrap().set_state(State::Null) {
                                Ok(_) => {
                                    info!("Successfully stopped recording");
                                }
                                Err(e) => {
                                    error!("Failed to stop recording {}", e);
                                }
                            }
                            break;
                        }
                        gstreamer::MessageView::Error(err) => {
                            error!("Pipeline Error: {:?}", err);
                            if let Some(debug) = err.debug() {
                                if debug.contains("reason not-linked (-1)") {
                                    warn!("Expected error hence continuing");
                                    pipeline_clone.lock().unwrap().set_state(State::Null).unwrap();
                                    pipeline_clone.lock().unwrap().set_state(State::Playing).unwrap();
                                    continue;
                                }
                            }
                            pipeline_failure_counter_clone.add(1, &vec![]);
                            break;
                        }
                        _ => {
                            //info!("Setting number of retries to 0");
                        }
                    }
                }
                // Clean up
                match pipeline_clone.lock().unwrap().set_state(State::Null) {
                    Ok(_) => {
                        info!("Successfully stopped recording");
                    }
                    Err(e) => {
                        error!("Failed to stop recording {}", e);
                    }
                }
            });
        }

        let pipeline_arc = {
            let gs_clone = gstreamer_self.clone();
            let pipeline_arc = gs_clone.pipeline.as_ref().unwrap().clone();
            let x = pipeline_arc.lock().unwrap().clone();
            x
        };

        let x = match pipeline_arc.set_state(gstreamer::State::Playing) {
            Ok(_) => {
                info!("Successfully set the pipeline state as Playing");
                Ok((gstreamer_self, plain_transport_port))
            }
            Err(e) => {
                error!("Failed to set the pipeline state as Playing {}", e);
                if exists {
                    if kind_enum == Kind::Audio {
                        gstreamer_new.clone().handle_udp_src_failure(participant_id.clone(), "audio".to_string());
                    } else {
                        gstreamer_new.clone().handle_udp_src_failure(participant_id.clone(), "video".to_string());
                    }
                    match pipeline_arc.set_state(gstreamer::State::Playing) {
                        Ok(_) => {
                            info!("Successfully set the pipeline state as Playing after handling failure");
                        }
                        Err(e) => {
                            error!("Failed to set the pipeline state as Playing after handling failure {}", e);
                        }
                    }
                }
                Err(format!("Failed to add new producer for meetingId {} and participantId {}", meeting_id.clone(), participant_id.clone()))
            }
        };
        x
    }

    pub async fn create_folder(&mut self, meeting_id: String) -> Result<String, String> {
        let path = format!("/opt/recordings/{}", meeting_id.clone());
        let mut folder_exists: bool;
        if !std::path::Path::new(path.as_str()).exists() {
            // Create the folder and all missing parent directories
            match fs::create_dir_all(path.clone()) {
                Ok(_) => {
                    folder_exists = true;
                    Ok(path)
                }
                Err(e) => {
                    folder_exists = false;
                    error!("Failed to create folder path : {} : {} ", path, e);
                    Err(format!("Failed to create folder: {} : {} ", path, e))
                }
            }
        } else {
            folder_exists = true;
            info!("Folder already exists: {}", path);
            Ok(path)
        }
    }

    pub async fn create_producer(
        &mut self,
        participant_id: String,
        rtp_parameters: RtpParameters,
        kind: String,
        meeting_id: String,
        producer_router_id: String,
        consumer_router_ids_option: Option<Vec<String>>,
        producer_transport_id: String,
        start_recording: bool,
        instance_id: String,
        meter: Meter,
        pool: Pool,
    ) -> Result<ProducerDetail, String> {
        info!("[create_producer] Creating producer for participant_id: {} meeting_id: {} kind: {}", participant_id.clone(), meeting_id.clone(), kind.clone());
        let key_mutex = {
            let mut lock_map = self.inner.lock_map.write().await;
            lock_map.entry(meeting_id.clone()).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
        };
        info!("[create_producer] Acquired inner lock for meeting_id: {} participant_id: {} kind: {}", meeting_id.clone(), participant_id.clone(), kind.clone());

        let guard = key_mutex.lock().await;
        if let Some(producer_detail) = {
            info!("[create_producer] Acquired producer map lock for meeting_id: {} participant_id: {} kind: {}", meeting_id.clone(), participant_id.clone(), kind.clone());
            let lock = self.inner.producer_map.read().await;
            lock.iter()
                .find(|(_, producer_in_mem)| {
                    producer_in_mem.meeting_id == meeting_id.clone()
                        && producer_in_mem.participant_id == participant_id.clone()
                        && producer_in_mem.kind == kind.clone()
                })
                .map(|(key, _)| ProducerDetail {
                    participant_id: participant_id.clone(),
                    producer_id: key.clone(),
                    kind: kind.clone(),
                })
        } {
            info!("[create_producer] Producer already exists for meeting_id: {} participant_id: {} kind: {}", meeting_id.clone(), participant_id.clone(), kind.clone());
            return Ok(producer_detail);
        }

        info!("[create_producer] Acquiring transport lock for meeting_id: {} participant_id: {} kind: {}", meeting_id.clone(), participant_id.clone(), kind.clone());
        let transport_map = {
            let lock = self.inner.transport_map.read().await;
            lock.clone()
        };
        info!("[create_producer] Acquired transport lock for meeting_id: {} participant_id: {} kind: {}", meeting_id.clone(), participant_id.clone(), kind.clone());

        let shared_clock = self.inner.shared_clock.clone();
        let transport = transport_map.get(producer_transport_id.as_str()).unwrap();
        let mut plain_transport_in_memory: Option<PlainTransport> = None;
        let mut plain_transport_consumer_in_memory: Option<Consumer> = None;
        let mut mysql_entities: Vec<Box<dyn Dao>> = Vec::new();
        let kind_enum = match kind.parse::<Kind>() {
            Ok(o) => o,
            Err(e) => {
                error!("Failed to parse kind: {}", e);
                return Err(format!("Failed to parse kind: {}", e));
            }
        };
        let media_kind = match kind_enum {
            Kind::Video => MediaKind::Video,
            Kind::Audio => MediaKind::Audio,
            Kind::ScreenShare => MediaKind::Video
        };
        let producer_options = ProducerOptions::new(media_kind, rtp_parameters.clone());
        // producer_options.key_frame_request_delay = 10000u32;
        let producer = match transport.transport
            .produce(producer_options)
            .await
        {
            Ok(producer) => producer,
            Err(e) => {
                error!("Failed to create producer: {}", e);
                return Err(format!("Failed to create producer: {}", e));
            }
        };
        let mut plain_transport: Option<PlainTransport> = None;
        let mut plain_transport_id: Option<TransportId> = None;
        let mut plain_transport_consumer: Option<Consumer> = None;
        let mut plain_transport_consumer_id: Option<ConsumerId> = None;
        let mut gstreamer_pipeline: Option<Gstreamer> = None;

        info!("[create_producer] Created producer for meeting_id: {} participant_id: {} kind: {}", meeting_id.clone(), participant_id.clone(), kind.clone());
        let routers_map = {
            let lock = self.inner.routers_map.read().await;
            lock.clone()
        };
        info!("[create_producer] Acquired routers map lock for meeting_id: {} participant_id: {} kind: {}", meeting_id.clone(), participant_id.clone(), kind.clone());

        if start_recording {
            let router = routers_map.get(producer_router_id.as_str()).unwrap().router.clone();
            let mut port_range = match kind_enum {
                Kind::Audio => Some(RangeInclusive::new(self.config.plain_transport_audio_port_range_start, self.config.plain_transport_audio_port_range_end)),
                Kind::Video => Some(RangeInclusive::new(self.config.plain_transport_video_port_range_start, self.config.plain_transport_video_port_range_end)),
                Kind::ScreenShare => Some(RangeInclusive::new(self.config.plain_transport_video_port_range_start, self.config.plain_transport_video_port_range_end)),
            };
            let plain_transport_internal = match router.create_plain_transport(self.producer_plain_transport_options(port_range)).await {
                Ok(o) => o,
                Err(e) => {
                    error!("Failed to create plain transport {}", e);
                    return Err(format!("Failed to create plain transport {}", e));
                }
            };
            match self.create_folder(meeting_id.clone()).await {
                Ok(o) => {
                    info!("Successfully created folder");
                    o
                }
                Err(e) => {
                    error!("Failed to create folder: {}", e);
                    return Err(format!("Failed to create folder: {}", e));
                }
            };
            // let aws_service_option = {
            //     info!("[create_producer] Acquired meeting cloud upload gstreamer instance map lock for meeting_id: {} participant_id: {} kind: {}", meeting_id.clone(), participant_id.clone(), kind.clone());
            //     let mut map = self.inner.meeting_cloud_upload_gstreamer_instance_map.write().await;
            //     info!("[create_producer] Inserting into meeting cloud upload gstreamer instance map for meeting_id: {} participant_id: {} kind: {}", meeting_id.clone(), participant_id.clone(), kind.clone());
            //
            //     match map.entry(meeting_id.clone()) {
            //         Entry::Occupied(entry) => None, // Nothing to do if already exists
            //         Entry::Vacant(entry) => {
            //             let aws_mutex = Arc::new(Mutex::new(AwsService::new(aws_bucket_name).await));
            //             Some(entry.insert(aws_mutex).clone()) // Clone the Arc so it can be used outside
            //         }
            //     }
            // };
            // if let Some(aws_service) = aws_service_option {
            //     let meeting_id_clone = meeting_id.clone();
            //     tokio::spawn(aws::AwsService::upload_to_cloud(Arc::clone(&aws_service), meeting_id_clone));
            // }

            let (gstreamer_instance, plain_transport_port) = match self.start_recording(meeting_id.clone(),
                                                                                        participant_id.clone(),
                                                                                        plain_transport_internal.id().to_string().clone(),
                                                                                        shared_clock.clone(), meter, kind_enum.clone()).await {
                Ok(p) => p,
                Err(e) => {
                    error!("Failed to start recording {}", e);
                    return Err(format!("Failed to start recording {}", e));
                }
            };

            gstreamer_pipeline = Some(gstreamer_instance);


            info!("Enabling plain transport on port {}", plain_transport_port);
            let plain_transport_parameters = PlainTransportRemoteParameters {
                ip: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                port: Some(plain_transport_port),
                rtcp_port: Some(plain_transport_port + 1),
                srtp_parameters: None,
            };
            match plain_transport_internal.connect(plain_transport_parameters).await {
                Ok(o) => info!("Successfully connected plain transport"),
                Err(e) => {
                    error!("Failed to connect plain transport {}", e);
                    return Err(format!("Failed to connect plain transport {}", e));
                }
            }
            let rtp_capabilities = RtpCapabilities {
                codecs: Self::media_codecs(),
                header_extensions: vec![],
            };
            let mut consumer_options = ConsumerOptions::new(producer.id(), rtp_capabilities);
            consumer_options.paused = true;
            let plain_transport_consumer_internal = match plain_transport_internal.consume(consumer_options).await {
                Ok(o) => o,
                Err(e) => {
                    error!("Failed to create consumer: {}", e);
                    return Err(format!("Failed to create consumer: {}", e));
                }
            };
            match plain_transport_consumer_internal.resume().await {
                Ok(o) => info!("Successfully resumed consumer"),
                Err(e) => {
                    error!("Failed to resume consumer: {}", e);
                    return Err(format!("Failed to resume consumer: {}", e));
                }
            }
            plain_transport = Some(plain_transport_internal.clone());
            plain_transport_consumer = Some(plain_transport_consumer_internal.clone());
            plain_transport_id = Some(plain_transport_internal.id().clone());
            plain_transport_consumer_id = Some(plain_transport_consumer_internal.id().clone());
        }
        let producer_router = routers_map.get(producer_router_id.as_str()).unwrap().router.clone();
        if let Some(consumer_router_ids) = consumer_router_ids_option {
            for consumer_router_id in consumer_router_ids {
                let consumer_router = routers_map.get(consumer_router_id.as_str()).unwrap().router.clone();
                producer_router.
                    pipe_producer_to_router(producer.id(), PipeToRouterOptions::new(consumer_router))
                    .await
                    .map_err(|e| {
                        error!("Failed to pipe producer to router: {}", e);
                        format!("Failed to pipe producer to router: {}", e)
                    })?;
            }
        }

        let participant_producers = ParticipantProducers {
            mutation_type: Insert,
            participant_id: Some(participant_id.clone()),
            producers: Some(1),
            kind: Some(kind.to_string()),
            producer_id: Some(producer.id().to_string()),
            meeting_id: Some(meeting_id.clone()),
            transport_id: Some(producer_transport_id.clone()),

        };
        plain_transport_in_memory = plain_transport;
        plain_transport_consumer_in_memory = plain_transport_consumer;
        mysql_entities.push(Box::new(participant_producers));

        let mysql = MySql {};
        if start_recording && plain_transport_in_memory.is_none() {
            error!("Plain transport is none");
            return Err("Plain transport is none".to_string());
        }
        if start_recording && plain_transport_consumer_in_memory.is_none() {
            error!("Plain transport consumer is none");
            return Err("Plain transport consumer is none".to_string());
        }
        let mysql_context = MySqlContext {
            pool,
            rollback_on_duplicate: true,
            tx_opts: TxOpts::default(),
        };
        match mysql.mutate(mysql_entities, Arc::from(mysql_context)) {
            Ok(o) => {
                {
                    info!("[create_producer] Acquiring producer map lock for meeting_id: {} participant_id: {} kind: {}", meeting_id.clone(), participant_id.clone(), kind.clone());
                    let mut producer_map_lock = self.inner.producer_map.write().await;
                    producer_map_lock.insert(producer.id().to_string(), ProducerInMemory {
                        producer: producer.clone(),
                        meeting_id: meeting_id.clone(),
                        participant_id: participant_id.clone(),
                        kind: kind.to_string().clone(),
                        instance_id: instance_id.clone(),
                    });
                }
                if start_recording && plain_transport_in_memory.is_some() {
                    //TODO : may not be a right thing to use producer_id as key, might impact debuggability
                    info!("[create_producer] Acquiring plain transport map lock for meeting_id: {} participant_id: {} kind: {}", meeting_id.clone(), participant_id.clone(), kind.clone());
                    self.inner.plain_transport_map.write().await.insert(producer.id().to_string(),
                                                                        plain_transport_in_memory.unwrap());
                    info!("[create_producer] Acquiring plain transport consumer map lock for meeting_id: {} participant_id: {} kind: {}", meeting_id.clone(), participant_id.clone(), kind.clone());
                    self.inner.plain_transport_consumer_map.write().await.insert(producer.id().to_string(),
                                                                                 plain_transport_consumer_in_memory.unwrap());
                    info!("[create_producer] Inserting gstreamer pipeline into pipeline map for meeting_id: {} participant_id: {} kind: {}", meeting_id.clone(), participant_id.clone(), kind.clone());
                    self.inner.pipeline_map.write().await.insert(meeting_id.to_string(), gstreamer_pipeline);
                }
                Ok(ProducerDetail {
                    participant_id,
                    producer_id: producer.id().to_string().clone(),
                    kind,
                })
            }
            Err(e) => {
                if gstreamer_pipeline.is_some() {
                    info!("[create_producer] Acquiring pipeline map lock for meeting_id: {} participant_id: {} kind: {}", meeting_id.clone(), participant_id.clone(), kind.clone());
                    self.inner.pipeline_map.write().await.insert(meeting_id.to_string(), gstreamer_pipeline);
                }
                error!("Error in writing to DB {}", e);
                Err(format!("Error in writing to DB {}", e))
            }
        }
    }
    /*
    target_participants : <p1, <kind1 : producer_id1, kind2 : producer_id2>>
     */
    /// Create consumers for a participant based on the target producer mappings.
    ///
    /// This will create new consumers in the specified transport for each entry
    /// in `target_participants`, which maps target participant IDs to the
    /// desired media kinds. Existing consumers will be reused if possible.
    pub async fn create_consumer(
        &mut self,
        participant_id: String,
        consumer_transport_id: String,
        target_participants: HashMap<String, HashMap<String, String>>,
        rtp_capabilities: RtpCapabilities,
        meeting_id: String,
        instance_id: String,
        pool: Pool,
    ) -> Result<HashMap<String, HashMap<String, Consumer>>, String> {
        info!("[create_consumer] Creating consumers for participant_id: {} meeting_id: {}", participant_id.clone(), meeting_id.clone());
        let transport_map = {
            let lock = self.inner.transport_map.read().await;
            lock.clone()
        };
        info!("[create_consumer] Acquired transport map lock for participant_id: {} meeting_id: {}", participant_id.clone(), meeting_id.clone());

        let consumer_transport = transport_map.get(consumer_transport_id.as_str()).unwrap();
        let mut temp_consumer_map: HashMap<String, ConsumerInMemory> = HashMap::new();

        info!("[create_consumer] Acquired consumer transport for participant_id: {} meeting_id: {}", participant_id.clone(), meeting_id.clone());
        let shard = self.get_shard(&meeting_id.clone());
        let existing_consumer_map = {
            let lock = shard.read().await;
            lock.clone()
        };
        info!("[create_consumer] Acquired existing consumer map lock for participant_id: {} meeting_id: {}", participant_id.clone(), meeting_id.clone());

        let mut mysql_entities: Vec<Box<dyn dao::Dao>> = Vec::new();
        let mut result: HashMap<String, HashMap<String, Consumer>> = HashMap::new();
        for (target_participant_id, kind_map) in target_participants.iter() {
            for (kind, producer_id_string) in kind_map.iter() {
                let existing_consumer = existing_consumer_map.values().filter_map(|c| {
                    if c.participant_id == participant_id.clone()
                        && c.target_participant_id == target_participant_id.clone()
                        && c.kind == kind.clone()
                        && c.meeting_id == meeting_id.clone() && c.instance_id == instance_id.clone()
                    {
                        Some(c)
                    } else {
                        None
                    }
                });
                if existing_consumer.clone().count() == 1 {
                    result.entry(target_participant_id.clone())
                        .or_insert_with(HashMap::new).insert(kind.to_string(), existing_consumer.into_iter().next().unwrap().consumer.clone());
                    continue;
                } else if existing_consumer.clone().count() > 1 {
                    error!("More than one existing consumer found for participant {}, target participant {}, kind {}, meeting_id {}, instance_id {}",
                           participant_id.clone(), target_participant_id.clone(), kind.clone(), meeting_id.clone(), instance_id.clone());
                    return Err(format!("More than one existing consumer found for participant {}, target participant {}, kind {}, meeting_id {}, instance_id {}",
                                       participant_id.clone(), target_participant_id.clone(), kind.clone(), meeting_id.clone(), instance_id.clone()));
                }
                let producer_id = match ProducerId::from_str(producer_id_string) {
                    Ok(o) => o,
                    Err(e) => {
                        error!("Failed to parse producer id: {}", e);
                        return Err(format!("Failed to parse producer id: {}", e));
                    }
                };
                let mut consumer_options = ConsumerOptions::new(producer_id, rtp_capabilities.clone());
                let consumer = match consumer_transport.transport
                    .consume(consumer_options)
                    .await
                {
                    Ok(consumer) => consumer,
                    Err(e) => {
                        error!("Error in creating consumer {}", e);
                        return Err(format!("Error in creating consumer {}", e));
                    }
                };
                temp_consumer_map.insert(consumer.id().to_string(), ConsumerInMemory {
                    consumer: consumer.clone(),
                    participant_id: participant_id.clone(),
                    target_participant_id: target_participant_id.clone(),
                    kind: kind.clone(),
                    meeting_id: meeting_id.clone(),
                    instance_id: instance_id.clone(),
                    producer_id: producer_id_string.clone(),
                });
                let participant_consumer = ParticipantConsumers {
                    mutation_type: Insert,
                    participant_id: Some(participant_id.clone()),
                    consumers: Some(1),
                    target_participant_id: Some(target_participant_id.clone()),
                    kind: Some(kind.to_string()),
                    consumer_id: Some(consumer.id().to_string().clone()),
                    meeting_id: Some(meeting_id.clone()),
                    transport_id: Some(consumer_transport_id.clone()),
                };
                mysql_entities.push(Box::new(participant_consumer));
                let mut temp_target_details = result.entry(target_participant_id.clone()).or_insert_with(HashMap::new);
                temp_target_details.insert(kind.to_string(), consumer.clone());
            }
        }
        let mysql = MySql {};
        let mysql_context = MySqlContext {
            pool,
            rollback_on_duplicate: true,
            tx_opts: TxOpts::default(),
        };

        info!("[create_consumer] Prepared all MySQL entities for participant_id: {} meeting_id: {}", participant_id.clone(), meeting_id.clone());
        let shard = self.get_shard(&meeting_id.clone());
        match mysql.mutate(mysql_entities, Arc::from(mysql_context)) {
            Ok(o) => {
                shard.write().await.extend(temp_consumer_map.clone());
                info!("[create_consumer] Successfully created consumers for participant_id: {} meeting_id: {}", participant_id.clone(), meeting_id.clone());
                Ok(result)
            }
            Err(e) => {
                error!("Error in writing to DB {}", e);
                Err(format!("Error in writing to DB {}", e))
            }
        }
    }

    /// Resume a list of consumers by ID for a participant in a meeting.
    ///
    /// This will call `resume()` on each consumer, allowing them to start
    /// receiving media again. Returns the list of successfully resumed consumer
    /// IDs.
    pub async fn resume_consumer(
        &mut self,
        participant_id: String,
        consumer_ids: Vec<String>,
        meeting_id: String
    ) -> Result<Vec<String>, String> {
        let shard = self.get_shard(&meeting_id.clone());
        let consumer_map = {
            let lock = shard.read().await;
            lock.clone()
        };
        let mut result: Vec<String> = Vec::new();
        for consumer_id in consumer_ids {
            let required_consumer_in_memory = consumer_map.get(consumer_id.as_str()).unwrap().clone();
            if let Err(e) = required_consumer_in_memory.consumer.resume().await {
                return Err(format!("[mod.rs.resume_consumer] Failed to resume err {}", e));
            }
            result.push(consumer_id);
        }
        Ok(result)
    }

    /// Resume a list of producers by ID for a participant.
    ///
    /// This will call `resume()` on each producer, allowing them to start
    /// sending media again. Returns the details of the successfully resumed
    /// producers.
    pub async fn resume_producer(
        &mut self,
        participant_id: String,
        producer_ids: HashMap<String, String>,
    ) -> Result<ProducerResponse, String> {
        info!("[mod.rs.resume_producer] Resuming producers for participant_id: {}", participant_id.clone());
        let producer_map = {
            let lock = self.inner.producer_map.read().await;
            lock.clone()
        };
        info!("[mod.rs.resume_producer] Acquired producer map lock for participant_id: {}", participant_id.clone());

        let mut result: Vec<ProducerDetail> = Vec::new();
        for (producer_id, kind) in producer_ids.iter() {
            let required_producer_in_memory = producer_map.get(producer_id.as_str()).unwrap().clone();
            if let Err(e) = required_producer_in_memory.producer.resume().await {
                return Err(format!("[mod.rs.resume_producer] Failed to resume err {}", e));
            }
            result.push(ProducerDetail {
                producer_id: producer_id.clone(),
                participant_id: participant_id.clone(),
                kind: kind.to_string(),
            });
        }
        Ok(ProducerResponse {
            producer_details: result
        })
    }

    /// Pause a list of producers by ID for a participant.
    ///
    /// This will call `pause()` on each producer, temporarily stopping them
    /// from sending media. Returns the details of the successfully paused
    /// producers.
    pub async fn pause_producer(
        &mut self,
        participant_id: String,
        producer_ids: HashMap<String, String>,
    ) -> Result<ProducerResponse, String> {
        info!("[mod.rs.pause_producer] Pausing producers for participant_id: {}", participant_id.clone());
        let producer_map = {
            let lock = self.inner.producer_map.read().await;
            lock.clone()
        };
        info!("[mod.rs.pause_producer] Acquired producer map lock for participant_id: {}", participant_id.clone());
        let mut result: Vec<ProducerDetail> = Vec::new();
        for (producer_id, kind) in producer_ids.iter() {
            let required_producer_in_memory = producer_map.get(producer_id.as_str()).unwrap().clone();
            if let Err(e) = required_producer_in_memory.producer.pause().await {
                return Err(format!("[mod.rs.resume_producer] Failed to resume err {}", e));
            }
            result.push(ProducerDetail {
                producer_id: producer_id.clone(),
                participant_id: participant_id.clone(),
                kind: kind.to_string(),
            });
        }
        Ok(ProducerResponse {
            producer_details: result
        })
    }

    /// Pause a list of consumers by ID for a participant.
    ///
    /// This will call `pause()` on each consumer, temporarily stopping them
    /// from receiving media. Returns the IDs of the successfully paused consumers.
    pub async fn pause_consumer(
        &mut self,
        participant_id: String,
        consumer_ids: Vec<String>,
        meeting_id: String
    ) -> Result<Vec<String>, String> {
        info!("[mod.rs.pause_consumer] Pausing consumers for participant_id: {}", participant_id.clone());
        let shard = self.get_shard(&meeting_id.clone());
        let consumer_map = {
            let lock = shard.read().await;
            lock.clone()
        };
        info!("[mod.rs.pause_consumer] Acquired consumer map lock for participant_id: {}", participant_id.clone());

        let mut result: Vec<String> = Vec::new();
        for consumer_id in consumer_ids {
            let required_consumer_in_memory = consumer_map.get(consumer_id.as_str()).unwrap().clone();
            if let Err(e) = required_consumer_in_memory.consumer.pause().await {
                return Err(format!("[mod.rs.resume_consumer] Failed to pause err {}", e));
            }
            result.push(consumer_id);
        }
        Ok(result)
    }

    /// Close and clean up producers for a participant in a meeting.
    ///
    /// This removes the producer associations from the database and closes the
    /// transports. If `producer_kind_map` is provided, only the specified
    /// producers will be closed.
    pub async fn close_producer(
        &mut self,
        participant_id: String,
        meeting_id: String,
        producer_kind_map: HashMap<String, String>,
        pool: Pool,
    ) -> Result<ProducerResponse, String> {
        let mut producer_ids: Vec<String> = Vec::new();
        let mut mysql_entities: Vec<Box<dyn Dao>> = Vec::new();
        let closed_kinds: Vec<String> = producer_kind_map.values().cloned().collect();
        for producer_id in producer_kind_map.keys() {
            let participant_producers = ParticipantProducers {
                mutation_type: Delete,
                participant_id: Some(participant_id.clone()),
                producers: None,
                kind: None,
                producer_id: Some(producer_id.clone()),
                meeting_id: Some(meeting_id.clone()),
                transport_id: None,
            };
            mysql_entities.push(Box::new(participant_producers));
            producer_ids.push(producer_id.clone());
        }
        let mysql = MySql {};
        let mysql_context = MySqlContext {
            pool,
            rollback_on_duplicate: true,
            tx_opts: TxOpts::default(),
        };
        match mysql.delete(mysql_entities, Arc::from(mysql_context)) {
            Ok(o) => info!("Success in writing to DB"),
            Err(e) => {
                error!("Error in cleaning up in DB {}", e);
                // Ideally should not throw an error since DB failure should not impact the user experience, will look into this later
                return Err(format!("Error in cleaning up in DB {}", e));
            }
        }

        info!("[CloseProducer] Successfully cleaned up DB entries for producers for participant {} meeting_id: {}", participant_id.clone(), meeting_id.clone());

        if closed_kinds.len() == 1 && closed_kinds[0] == "screenshare" {
            match self.close_all_producers_in_memory(
                vec![participant_id.clone()],
                producer_ids.clone(),
                meeting_id.clone(),
                false).await {
                Ok(producer_details) => {
                    info!("[CloseProducer] Successfully closed all screenshare producers in memory for participant {} meeting_id: {}", participant_id.clone(), meeting_id.clone());
                    Ok(ProducerResponse {
                        producer_details
                    })
                }
                Err(e) => {
                    error!("Failed to close all screenshare producers in memory {}", e);
                    Err(format!("Failed to close all screenshare producers in memory {}", e))
                }
            }
        } else {
            match self.close_all_producers_in_memory(
                vec![participant_id.clone()],
                producer_ids.clone(),
                meeting_id.clone(),
                true).await {
                Ok(producer_details) => {
                    info!("[CloseProducer] Successfully closed all producers in memory for participant {} meeting_id: {}", participant_id.clone(), meeting_id.clone());
                    Ok(ProducerResponse {
                        producer_details
                    })
                }
                Err(e) => {
                    error!("Failed to close all producers in memory {}", e);
                    Err(format!("Failed to close all producers in memory {}", e))
                }
            }
        }
    }

    /// Close and clean up all producers in memory for a set of participants.
    ///
    /// This removes the producers from the in-memory maps and stops any
    /// associated media processing. It is typically called when a meeting ends
    /// or a participant leaves.
    pub async fn close_all_producers_in_memory(
        &mut self,
        participant_ids: Vec<String>,
        producer_ids: Vec<String>,
        meeting_id: String,
        remove_from_pipeline: bool,
    ) -> Result<Vec<ProducerDetail>, String> {
        let mut producer_details: Vec<ProducerDetail> = Vec::new();
        {
            let mut producer_map = self.inner.producer_map.write().await;
            let mut plain_transport_map = self.inner.plain_transport_map.write().await;
            let mut plain_transport_consumer_map = self.inner.plain_transport_consumer_map.write().await;

            for producer_id in &producer_ids {
                if let Some(v) = producer_map.remove(producer_id) {
                    producer_details.push(ProducerDetail {
                        producer_id: producer_id.clone(),
                        participant_id: participant_ids[0].clone(),
                        kind: v.kind.clone(),
                    });
                    plain_transport_map.remove(producer_id);
                    plain_transport_consumer_map.remove(producer_id);
                }
            }
        }

        if remove_from_pipeline {
            let temp_pipeline_map = self.inner.pipeline_map.read().await;
            let gstreamer_from_map = temp_pipeline_map.get(&meeting_id);
            if let (Some(gstreamer_from_map)) = gstreamer_from_map {
                let gstreamer = gstreamer_from_map.clone().unwrap();
                let gstreamer_clone = gstreamer.clone();
                let pipeline = gstreamer_clone.pipeline.unwrap();
                let gstreamer_inner_unlock = gstreamer_clone.inner.lock().unwrap();
                let pipeline_unlock = pipeline.lock().unwrap();
                info!("Removing udp_src_element_audio");
                let mut remove_elements = Vec::new();
                let mut udp_src_elements_audio = Vec::new();
                let mut udp_src_elements_audio_length = 0;
                {
                    let mut udp_src_elements_audio_map = gstreamer_inner_unlock.udp_src_element_audio_map.lock().unwrap();
                    for id in &participant_ids {
                        if let Some(value) = udp_src_elements_audio_map.remove(id) {
                            match value.set_state(Null) {
                                Ok(_) => {
                                    info!("Successfully set udp_src_element_audio_map element to Null for participant {}", id);
                                }
                                Err(err) => {
                                    error!("Failed to set udp_src_element_audio_map element to Null for participant {}: {}", id, err);
                                }
                            }
                            udp_src_elements_audio.push(value);
                        }
                    }
                    udp_src_elements_audio_length = udp_src_elements_audio_map.len();
                }
                remove_elements.extend(udp_src_elements_audio.clone());
                let mut udp_src_elements_video = Vec::new();
                let mut udp_src_elements_video_length = 0;
                {
                    let mut udp_src_element_video_map = gstreamer_inner_unlock.udp_src_element_video_map.lock().unwrap();
                    for id in &participant_ids {
                        if let Some(value) = udp_src_element_video_map.remove(id) {
                            match value.set_state(Null) {
                                Ok(_) => {
                                    info!("Successfully set udp_src_element_video_map element to Null for participant {}", id);
                                }
                                Err(err) => {
                                    error!("Failed to set udp_src_element_video_map element to Null for participant {}: {}", id, err);
                                }
                            }
                            udp_src_elements_video.push(value);
                        }
                    }
                    udp_src_elements_video_length = udp_src_element_video_map.len();
                }
                remove_elements.extend(udp_src_elements_video.clone());
                let mut udp_rtcp_src_elements_audio = Vec::new();
                {
                    let mut udp_rtcp_src_element_audio_map = gstreamer_inner_unlock.udp_rtcp_src_element_audio_map.lock().unwrap();
                    for id in &participant_ids {
                        if let Some(value) = udp_rtcp_src_element_audio_map.remove(id) {
                            match value.set_state(Null) {
                                Ok(_) => {
                                    info!("Successfully set udp_rtcp_src_element_audio_map element to Null for participant {}", id);
                                }
                                Err(err) => {
                                    error!("Failed to set udp_rtcp_src_element_audio_map element to Null for participant {}: {}", id, err);
                                }
                            }
                            udp_rtcp_src_elements_audio.push(value);
                        }
                    }
                }
                remove_elements.extend(udp_rtcp_src_elements_audio.clone());
                info!("Removing queue_element_audio");
                let mut queue_elements_audio = Vec::new();
                info!("Trying to get queue_element_audio_map lock");
                {
                    let mut queue_element_audio_map = gstreamer_inner_unlock.queue_element_audio_map.lock().unwrap();
                    for id in &participant_ids {
                        if let Some(value) = queue_element_audio_map.remove(id) {
                            info!("Trying to set queue_element_audio_map element to Null for participant {}", id);
                            match value.set_state(Null) {
                                Ok(_) => {
                                    info!("Successfully set queue_element_audio_map element to Null for participant {}", id);
                                }
                                Err(err) => {
                                    error!("Failed to set queue_element_audio_map element to Null for participant {}: {}", id, err);
                                }
                            }
                            queue_elements_audio.push(value);
                        }
                    }
                }
                remove_elements.extend(queue_elements_audio.clone());
                info!("Removing rtp_opus_de_pay_element_audio");
                let mut rtp_opus_de_pay_elements_audio = Vec::new();
                {
                    let mut rtp_opus_de_pay_element_audio_map = gstreamer_inner_unlock.rtp_opus_de_pay_element_audio_map.lock().unwrap();
                    for id in &participant_ids {
                        if let Some(value) = rtp_opus_de_pay_element_audio_map.remove(id) {
                            match value.set_state(Null) {
                                Ok(_) => {
                                    info!("Successfully set rtp_opus_de_pay_element_audio_map element to Null for participant {}", id);
                                }
                                Err(err) => {
                                    error!("Failed to set rtp_opus_de_pay_element_audio_map element to Null for participant {}: {}", id, err);
                                }
                            }
                            rtp_opus_de_pay_elements_audio.push(value);
                        }
                    }
                }
                remove_elements.extend(rtp_opus_de_pay_elements_audio.clone());
                info!("Removing opus_dec_element_audio");
                let mut opus_dec_elements_audio = Vec::new();
                {
                    let mut opus_dec_element_audio_map = gstreamer_inner_unlock.opus_dec_element_audio_map.lock().unwrap();
                    for id in &participant_ids {
                        if let Some(value) = opus_dec_element_audio_map.remove(id) {
                            match value.set_state(Null) {
                                Ok(_) => {
                                    info!("Successfully set opus_dec_element_audio_map element to Null for participant {}", id);
                                }
                                Err(err) => {
                                    error!("Failed to set opus_dec_element_audio_map element to Null for participant {}: {}", id, err);
                                }
                            }
                            opus_dec_elements_audio.push(value);
                        }
                    }
                }
                remove_elements.extend(opus_dec_elements_audio.clone());
                info!("Removing udp_rtcp_sink_sink_element_audio");
                let mut udp_rtcp_sink_sink_elements_audio = Vec::new();
                {
                    let mut udp_rtcp_sink_sink_element_audio_map = gstreamer_inner_unlock.udp_rtcp_sink_sink_element_audio_map.lock().unwrap();
                    for id in &participant_ids {
                        if let Some(value) = udp_rtcp_sink_sink_element_audio_map.remove(id) {
                            match value.set_state(Null) {
                                Ok(_) => {
                                    info!("Successfully set udp_rtcp_sink_sink_element_audio_map element to Null for participant {}", id);
                                }
                                Err(err) => {
                                    error!("Failed to set udp_rtcp_sink_sink_element_audio_map element to Null for participant {}: {}", id, err);
                                }
                            }
                            udp_rtcp_sink_sink_elements_audio.push(value);
                        }
                    }
                }
                remove_elements.extend(udp_rtcp_sink_sink_elements_audio.clone());
                let mut audio_mixer_pads = Vec::new();
                {
                    let mut audio_mixed_pad_map = gstreamer_inner_unlock.audio_mixed_pad_map.lock().unwrap();
                    for id in &participant_ids {
                        if let Some(value) = audio_mixed_pad_map.remove(id) {
                            audio_mixer_pads.push(value);
                        }
                    }
                }
                let audio_mixer_option = gstreamer_inner_unlock.audio_mixer_element.clone();
                match audio_mixer_option {
                    Some(mixer) => {
                        let mixer_unlock = mixer.lock().unwrap().clone();
                        for pad in audio_mixer_pads {
                            mixer_unlock.release_request_pad(&pad);
                        }
                    }
                    None => {
                        info!("Audio mixer element not found");
                        // return Err("Audio mixer element not found".to_string());
                    }
                };
                let mut udp_rtcp_src_elements_video = Vec::new();
                {
                    let mut udp_rtcp_src_element_video_map = gstreamer_inner_unlock.udp_rtcp_src_element_video_map.lock().unwrap();
                    for id in &participant_ids {
                        if let Some(value) = udp_rtcp_src_element_video_map.remove(id) {
                            match value.set_state(Null) {
                                Ok(_) => {
                                    info!("Successfully set udp_rtcp_src_element_video_map element to Null for participant {}", id);
                                }
                                Err(err) => {
                                    error!("Failed to set udp_rtcp_src_element_video_map element to Null for participant {}: {}", id, err);
                                }
                            }
                            udp_rtcp_src_elements_video.push(value);
                        }
                    }
                }

                remove_elements.extend(udp_rtcp_src_elements_video.clone());
                let mut queue_elements_video = Vec::new();
                {
                    let mut queue_element_vp8_video_map = gstreamer_inner_unlock.queue_element_vp8_video_map.lock().unwrap();
                    for id in &participant_ids {
                        if let Some(value) = queue_element_vp8_video_map.remove(id) {
                            match value.set_state(Null) {
                                Ok(_) => {
                                    info!("Successfully set queue_element_video_map element to Null for participant {}", id);
                                }
                                Err(err) => {
                                    error!("Failed to set queue_element_video_map element to Null for participant {}: {}", id, err);
                                }
                            }
                            queue_elements_video.push(value);
                        }
                    }
                }
                remove_elements.extend(queue_elements_video.clone());
                let mut rtp_vp8_de_pay_elements_video = Vec::new();
                {
                    let mut rtp_vp8_de_pay_element_video_map = gstreamer_inner_unlock.rtp_vp8_de_pay_element_video_map.lock().unwrap();
                    for id in &participant_ids {
                        if let Some(value) = rtp_vp8_de_pay_element_video_map.remove(id) {
                            match value.set_state(Null) {
                                Ok(_) => {
                                    info!("Successfully set rtp_vp8_de_pay_element_video_map element to Null for participant {}", id);
                                }
                                Err(err) => {
                                    error!("Failed to set rtp_vp8_de_pay_element_video_map element to Null for participant {}: {}", id, err);
                                }
                            }
                            rtp_vp8_de_pay_elements_video.push(value);
                        }
                    }
                }
                remove_elements.extend(rtp_vp8_de_pay_elements_video.clone());
                let mut vp8_dec_elements_video = Vec::new();
                {
                    let mut vp8_dec_element_video_map = gstreamer_inner_unlock.vp8_dec_element_video_map.lock().unwrap();
                    for id in &participant_ids {
                        if let Some(value) = vp8_dec_element_video_map.remove(id) {
                            match value.set_state(Null) {
                                Ok(_) => {
                                    info!("Successfully set vp8_dec_element_video_map element to Null for participant {}", id);
                                }
                                Err(err) => {
                                    error!("Failed to set vp8_dec_element_video_map element to Null for participant {}: {}", id, err);
                                }
                            }
                            vp8_dec_elements_video.push(value);
                        }
                    }
                }
                remove_elements.extend(vp8_dec_elements_video.clone());
                let mut caps_elements_video = Vec::new();
                {
                    let mut caps_filter_element_video_map = gstreamer_inner_unlock.caps_filter_element_video_map.lock().unwrap();
                    for id in &participant_ids {
                        if let Some(value) = caps_filter_element_video_map.remove(id) {
                            match value.set_state(Null) {
                                Ok(_) => {
                                    info!("Successfully set caps_element_video_map element to Null for participant {}", id);
                                }
                                Err(err) => {
                                    error!("Failed to set caps_element_video_map element to Null for participant {}: {}", id, err);
                                }
                            }
                            caps_elements_video.push(value);
                        }
                    }
                }
                remove_elements.extend(caps_elements_video.clone());
                let mut video_scale_elements_video = Vec::new();
                {
                    let mut video_scale_element_video_map = gstreamer_inner_unlock.video_scale_element_video_map.lock().unwrap();
                    for id in &participant_ids {
                        if let Some(value) = video_scale_element_video_map.remove(id) {
                            match value.set_state(Null) {
                                Ok(_) => {
                                    info!("Successfully set video_scale_element_video_map element to Null for participant {}", id);
                                }
                                Err(err) => {
                                    error!("Failed to set video_scale_element_video_map element to Null for participant {}: {}", id, err);
                                }
                            }
                            video_scale_elements_video.push(value);
                        }
                    }
                }
                remove_elements.extend(video_scale_elements_video.clone());
                let mut video_convert_elements_video = Vec::new();
                {
                    let mut video_convert_element_video_map = gstreamer_inner_unlock.video_convert_element_video_map.lock().unwrap();
                    for id in &participant_ids {
                        if let Some(value) = video_convert_element_video_map.remove(id) {
                            match value.set_state(Null) {
                                Ok(_) => {
                                    info!("Successfully set video_convert_element_video_map element to Null for participant {}", id);
                                }
                                Err(err) => {
                                    error!("Failed to set video_convert_element_video_map element to Null for participant {}: {}", id, err);
                                }
                            }
                            video_convert_elements_video.push(value);
                        }
                    }
                }
                remove_elements.extend(video_convert_elements_video.clone());
                let mut rtp_bin_sink_pads = Vec::new();
                let mut rtcp_bin_sink_pads = Vec::new();
                let mut session_ids = Vec::new();
                {
                    gstreamer_inner_unlock.session_participant_meta_map.lock().unwrap().retain(|key, value| {
                        let keep = !participant_ids.contains(&value.participant_id);
                        if !keep {
                            rtp_bin_sink_pads.push(value.rtp_sink_pad.clone());
                            rtcp_bin_sink_pads.push(value.rtcp_sink_pad.clone());
                            session_ids.push(key.clone());
                        }
                        keep
                    });
                }
                /*let mut video_pads = Vec::new();
                {
                    gstreamer_inner_unlock.video_compositor_pad_map.lock().unwrap().retain(|key, value| {
                        let keep = !participant_ids.contains(key);
                        if !keep {
                            video_pads.push(value.clone());
                        }
                        keep
                    });
                }*/
                match pipeline_unlock.set_state(Paused) {
                    Ok(_) => {
                        info!("Pipeline paused successfully");
                    }
                    Err(err) => {
                        error!("Failed to pause pipeline: {}", err);
                        return Err("Failed to pause pipeline".to_string());
                    }
                }
                for element in remove_elements {
                    match element.set_state(Null) {
                        Ok(_) => {
                            info!("Successfully set element to Null");
                        }
                        Err(err) => {
                            error!("Failed to set element to Null: {}", err);
                            return Err(format!("Failed to set element to Null: {}", err));
                        }
                    }
                }
                match pipeline_unlock.remove_many(&udp_src_elements_audio) {
                    Ok(_) => {
                        info!("Successfully removed udp_src_elements from pipeline");
                    }
                    Err(err) => {
                        error!("Failed to remove udp_src_elements from pipeline: {}", err);
                        return Err(format!("Failed to remove udp_src_elements from pipeline: {}", err));
                    }
                }
                match pipeline_unlock.remove_many(&udp_rtcp_src_elements_audio) {
                    Ok(_) => {
                        info!("Successfully removed udp_rtcp_src_elements from pipeline");
                    }
                    Err(err) => {
                        error!("Failed to remove udp_rtcp_src_elements from pipeline: {}", err);
                        return Err(format!("Failed to remove udp_rtcp_src_elements from pipeline: {}", err));
                    }
                }
                // Unlink the rtp_bin_pads here
                let rtp_bin_element = gstreamer_inner_unlock.rtp_bin_element.clone();
                if let Some(rtp_bin) = rtp_bin_element {
                    let rtp_bin_unlock = rtp_bin.lock().unwrap();
                    info!("Number of pads connected to rtp_bin element before removal: {}", rtp_bin_unlock.pads().len());

                    let mut session_id_ssrc_map = gstreamer_inner_unlock.session_id_ssrc_map.lock().unwrap();
                    for session_id in session_ids {
                        if let Some(ssrc_list) = session_id_ssrc_map.remove(&session_id) {
                            for ssrc in ssrc_list {
                                rtp_bin_unlock.emit_by_name::<()>("clear-ssrc", &[&session_id, &ssrc]);
                            }
                        }
                    }
                    for sink_pad in rtcp_bin_sink_pads {
                        info!("Releasing rtcp sink pad for rtp_bin element");
                        rtp_bin_unlock.release_request_pad(&sink_pad);
                    }
                    for sink_pad in rtp_bin_sink_pads {
                        info!("Releasing rtp sink pad for rtp_bin element");
                        rtp_bin_unlock.release_request_pad(&sink_pad);
                    }
                    info!("Number of pads connected to rtp_bin element after removal: {}", rtp_bin_unlock.pads().len());
                } else {
                    error!("RTP bin element not found");
                    return Err("RTP bin element not found".to_string());
                }
                match pipeline_unlock.remove_many(&udp_rtcp_sink_sink_elements_audio) {
                    Ok(_) => {
                        info!("Successfully removed udp_rtcp_sink_sink_elements from pipeline");
                    }
                    Err(err) => {
                        error!("Failed to remove udp_rtcp_sink_sink_elements from pipeline: {}", err);
                        return Err(format!("Failed to remove udp_rtcp_sink_sink_elements from pipeline: {}", err));
                    }
                }
                match pipeline_unlock.remove_many(&queue_elements_audio) {
                    Ok(_) => {
                        info!("Successfully removed queue_elements from pipeline");
                    }
                    Err(err) => {
                        error!("Failed to remove queue_elements from pipeline: {}", err);
                        return Err(format!("Failed to remove queue_elements from pipeline: {}", err));
                    }
                }
                match pipeline_unlock.remove_many(&rtp_opus_de_pay_elements_audio) {
                    Ok(_) => {
                        info!("Successfully removed rtp_opus_de_pay_elements from pipeline");
                    }
                    Err(err) => {
                        error!("Failed to remove rtp_opus_de_pay_elements from pipeline: {}", err);
                        return Err(format!("Failed to remove rtp_opus_de_pay_elements from pipeline: {}", err));
                    }
                }
                match pipeline_unlock.remove_many(&opus_dec_elements_audio) {
                    Ok(_) => {
                        info!("Successfully removed opus_dec_elements from pipeline");
                    }
                    Err(err) => {
                        error!("Failed to remove opus_dec_elements from pipeline: {}", err);
                        return Err(format!("Failed to remove opus_dec_elements from pipeline: {}", err));
                    }
                }
                match pipeline_unlock.remove_many(&udp_src_elements_video) {
                    Ok(_) => {
                        info!("Successfully removed udp_src_elements_video from pipeline");
                    }
                    Err(err) => {
                        error!("Failed to remove udp_src_elements_video from pipeline: {}", err);
                        return Err(format!("Failed to remove udp_src_elements_video from pipeline: {}", err));
                    }
                }
                match pipeline_unlock.remove_many(&udp_rtcp_src_elements_video) {
                    Ok(_) => {
                        info!("Successfully removed udp_rtcp_src_elements_video from pipeline");
                    }
                    Err(err) => {
                        error!("Failed to remove udp_rtcp_src_elements_video from pipeline: {}", err);
                        return Err(format!("Failed to remove udp_rtcp_src_elements_video from pipeline: {}", err));
                    }
                }
                match pipeline_unlock.remove_many(&queue_elements_video) {
                    Ok(_) => {
                        info!("Successfully removed queue_elements_video from pipeline");
                    }
                    Err(err) => {
                        error!("Failed to remove queue_elements_video from pipeline: {}", err);
                        return Err(format!("Failed to remove queue_elements_video from pipeline: {}", err));
                    }
                }
                match pipeline_unlock.remove_many(&rtp_vp8_de_pay_elements_video) {
                    Ok(_) => {
                        info!("Successfully removed rtp_vp8_de_pay_elements_video from pipeline");
                    }
                    Err(err) => {
                        error!("Failed to remove rtp_vp8_de_pay_elements_video from pipeline: {}", err);
                        return Err(format!("Failed to remove rtp_vp8_de_pay_elements_video from pipeline: {}", err));
                    }
                }
                match pipeline_unlock.remove_many(&vp8_dec_elements_video) {
                    Ok(_) => {
                        info!("Successfully removed vp8_dec_elements_video from pipeline");
                    }
                    Err(err) => {
                        error!("Failed to remove vp8_dec_elements_video from pipeline: {}", err);
                        return Err(format!("Failed to remove vp8_dec_elements_video from pipeline: {}", err));
                    }
                }
                match pipeline_unlock.remove_many(&caps_elements_video) {
                    Ok(_) => {
                        info!("Successfully removed caps_elements_video from pipeline");
                    }
                    Err(err) => {
                        error!("Failed to remove caps_elements_video from pipeline: {}", err);
                        return Err(format!("Failed to remove caps_elements_video from pipeline: {}", err));
                    }
                }
                match pipeline_unlock.remove_many(&video_scale_elements_video) {
                    Ok(_) => {
                        info!("Successfully removed video_scale_elements_video from pipeline");
                    }
                    Err(err) => {
                        error!("Failed to remove video_scale_elements_video from pipeline: {}", err);
                        return Err(format!("Failed to remove video_scale_elements_video from pipeline: {}", err));
                    }
                }
                match pipeline_unlock.remove_many(&video_convert_elements_video) {
                    Ok(_) => {
                        info!("Successfully removed video_convert_elements_video from pipeline");
                    }
                    Err(err) => {
                        error!("Failed to remove video_convert_elements_video from pipeline: {}", err);
                        return Err(format!("Failed to remove video_convert_elements_video from pipeline: {}", err));
                    }
                }
                /*let video_compositor_option = gstreamer_inner_unlock.video_compositor_element.clone();
                match video_compositor_option {
                    Some(compositor) => {
                        let compositor_unlock = compositor.lock().unwrap().clone();
                        info!("Number of pads connected to video compositor before removal: {}", compositor_unlock.pads().len());
                        for pad in video_pads {
                            info!("Releasing request pad for video compositor element");
                            compositor_unlock.release_request_pad(&pad);
                        }
                        info!("Number of pads connected to video compositor: {}", compositor_unlock.pads().len());
                    }
                    None => {
                        info!("Video compositor element not found");
                    }
                };*/
                if udp_src_elements_audio_length > 0 || udp_src_elements_video_length > 0
                {
                    match pipeline_unlock.set_state(Playing) {
                        Ok(_) => {
                            info!("Pipeline set to Playing successfully");
                        }
                        Err(err) => {
                            error!("Failed to set pipeline to Playing: {}", err);
                            return Err("Failed to set pipeline to Playing".to_string());
                        }
                    }
                }
            };
        }
        Ok(producer_details)
    }
    /// Close and clean up all consumers for a participant in a meeting.
    ///
    /// This removes the consumers from the in-memory maps and stops any
    /// associated media processing. It is typically called when a meeting ends
    /// or a participant leaves.
    pub async fn close_all_consumers_in_memory(
        &mut self,
        participant_id: String,
        consumer_ids: Vec<String>,
        meeting_id: String
    ) {
        let shard = self.get_shard(&meeting_id.clone());
        let mut shard = shard.write().await;
        for id in &consumer_ids {
            shard.remove(id);
        }

    }

    /// Close and remove consumers from a participant in a meeting.
    ///
    /// This deletes the consumer associations from the database and closes the
    /// transports. It can be called with a list of specific consumer IDs to
    /// close only those, or all consumers for the participant.
    pub async fn close_consumer(
        &mut self,
        participant_id: String,
        consumer_ids: Vec<String>,
        meeting_id: String,
        pool: Pool,
    ) -> Result<(), String> {
        let mut mysql_entities: Vec<Box<dyn Dao>> = Vec::new();
        for consumer_id in consumer_ids.clone() {
            let participant_consumers = ParticipantConsumers {
                mutation_type: Delete,
                participant_id: None,
                consumers: None,
                target_participant_id: None,
                kind: None,
                consumer_id: Some(consumer_id.clone()),
                meeting_id: Some(meeting_id.clone()),
                transport_id: None,
            };
            mysql_entities.push(Box::new(participant_consumers));
        }
        let mysql_context = MySqlContext {
            pool,
            rollback_on_duplicate: true,
            tx_opts: TxOpts::default(),
        };
        let mysql = MySql {};
        match mysql.mutate(mysql_entities, Arc::from(mysql_context)) {
            Ok(o) => info!("Success in writing to DB"),
            Err(e) => {
                error!("Error in cleaning up in DB {}", e);
                // Ideally should not throw an error since DB failure should not impact the user experience, will look into this later
                return Err(format!("Error in cleaning up in DB {}", e));
            }
        }
        info!("[CloseConsumer] Successfully cleaned up consumers from DB for participant_id: {} meeting_id: {}", participant_id.clone(), meeting_id.clone());
        let shard = self.get_shard(&meeting_id.clone());
        let mut shard = shard.write().await;
        for key in &consumer_ids {
            shard.remove(key);
        }
        info!("[CloseConsumer] Successfully cleaned up consumers from memory for participant_id: {} meeting_id: {}", participant_id.clone(), meeting_id.clone());
        Ok(())
    }

    /// Remove a participant from a meeting and clean up resources.
    ///
    /// This deletes the participant's associations from the database, closes
    /// transports, and removes the participant from any active routers. It also
    /// sends a LEAVE event to the analytic client.
    pub async fn leave_meeting(
        &mut self,
        participant_id: String,
        meeting_id: String,
        instance_id: String,
        producer_transport_id: Option<String>,
        consumer_transport_id: Option<String>,
        pool: Pool,
    ) -> Result<LeaveMeetingResponse, String> {
        let mut entities: Vec<Box<dyn Dao>> = Vec::new();
        let participant_meeting = ParticipantMeeting {
            mutation_type: Delete,
            meeting_id: meeting_id.clone(),
            participant_id: Some(participant_id.clone()),
            instance_id: Some(instance_id.clone()),
        };
        entities.push(Box::new(participant_meeting));
        let mysql_context = MySqlContext {
            pool,
            rollback_on_duplicate: true,
            tx_opts: TxOpts::default(),
        };
        let mysql = MySql {};
        match mysql.delete(entities, Arc::from(mysql_context)) {
            Ok(o) => {
                info!("Success in deleting from DB");
                if o <= 0 {
                    return Ok(LeaveMeetingResponse {
                        response: "Participant not found in the meeting".to_string(),
                        send_signal: false,
                    });
                }
                let mut transport_ids = Vec::new();
                if let Some(producer_transport_id) = producer_transport_id {
                    transport_ids.push(producer_transport_id.clone());
                }
                if let Some(consumer_transport_id) = consumer_transport_id {
                    transport_ids.push(consumer_transport_id.clone());
                }
                if !transport_ids.is_empty() {
                    let mut transport_map = self.inner.transport_map.write().await;
                    for id in &transport_ids {
                        transport_map.remove(id);
                    }
                }

                let old_consumers = self.get_participant_consumers_for_instance_in_meeting(participant_id.clone(), meeting_id.clone(), instance_id.clone()).await;
                self.close_all_consumers_in_memory(participant_id.clone(), old_consumers, meeting_id.clone()).await;

                let old_producer_ids_list = self.get_participant_producers_for_instance_in_meeting(participant_id.clone(), meeting_id.clone(), instance_id.clone()).await;
                if !old_producer_ids_list.is_empty() {
                    match self.close_all_producers_in_memory(vec![participant_id.clone()], old_producer_ids_list.clone(), meeting_id.clone(), true).await {
                        Ok(o) => {
                            info!("Successfully closed all producers in memory");
                        }
                        Err(e) => {
                            error!("Failed to close all producers in memory {}", e);
                            return Err(format!("Failed to close all producers in memory {}", e));
                        }
                    }
                }

                let close_consumer_ids_list: Vec<String> = self.get_consumers_for_producer_ids(old_producer_ids_list.clone(), meeting_id.clone()).await;
                if !close_consumer_ids_list.is_empty() {
                    self.close_all_consumers_in_memory("".to_string(), close_consumer_ids_list, meeting_id.clone()).await;
                }

                // Send Leave Event
                self.push_user_session_event(participant_id.clone(), meeting_id.clone(), "LEAVE".to_string(), instance_id.clone());
                Ok(LeaveMeetingResponse {
                    response: "Participant left the meeting".to_string(),
                    send_signal: true,
                })
            }
            Err(e) => {
                error!("Error in deleting from DB {}", e);
                Err("Failure in leaving the meeting".to_string())
            }
        }
    }

    /// End a meeting and clean up all associated resources.
    ///
    /// This removes all routers, transports, producers and consumers associated
    /// with the meeting. It also deletes the meeting record from the database
    /// and stops any active recording pipelines.
    pub async fn end_meeting(
        &mut self,
        meeting_id: String,
        router_ids: Vec<String>,
        transport_ids: Vec<String>,
        participant_ids: Vec<String>,
        pool: Pool) -> Result<String, String>
    {
        info!("In end_meeting for meeting_id: {}", meeting_id.clone());
        let mysql = MySql {};
        let mysql_context = MySqlContext {
            pool,
            rollback_on_duplicate: true,
            tx_opts: TxOpts::default(),
        };
        let mut mysql_entities: Vec<Box<dyn Dao>> = Vec::new();
        let meeting_status = MeetingStatus {
            mutation_type: Delete,
            meeting_id: meeting_id.clone(),
            enabled: None,
        };
        mysql_entities.push(Box::new(meeting_status));
        match mysql.mutate(mysql_entities, Arc::from(mysql_context.clone())) {
            Ok(rows) => {}
            Err(e) => {
                error!("Error in deleting from DB for meeting_id: {} err: {}", meeting_id.clone(), e);
                return Err("Failure in ending the meeting".to_string());
            }
        };
        {
            let mut routers_map = self.inner.routers_map.write().await;
            for id in &router_ids {
                routers_map.remove(id);
            }
        }

        {
            let mut transport_map = self.inner.transport_map.write().await;
            for id in &transport_ids {
                transport_map.remove(id);
            }
        }

        info!("In end_meeting, closing all consumers for meeting_id: {}", meeting_id.clone());
        let old_consumers = self.get_all_consumers_for_meeting(meeting_id.clone()).await;
        self.close_all_consumers_in_memory("".to_string(), old_consumers, meeting_id.clone()).await;

        info!("In end_meeting, closing all producers in memory for meeting_id: {}", meeting_id.clone());
        let old_producer_ids_list = self.get_all_producers_for_meeting(meeting_id.clone()).await;
        if !old_producer_ids_list.is_empty() {
            info!("In end_meeting, found {} producers to close for meeting_id: {}", old_producer_ids_list.clone().len(), meeting_id.clone());
            match self.close_all_producers_in_memory(participant_ids, old_producer_ids_list, meeting_id.clone(), false).await {
                Ok(o) => {
                    info!("Successfully closed all producers in memory for meeting_id: {}", meeting_id.clone());
                }
                Err(e) => {
                    error!("Failed to close all producers in memory for meeting_id: {} err: {}", meeting_id.clone(), e);
                    return Err(format!("Failed to close all producers in memory {}", e));
                }
            }
        }

        info!("In end_meeting, stopping gstreamer pipeline for meeting_id: {}", meeting_id.clone());
        let mut temp_pipeline_map = {
            let lock = self.inner.pipeline_map.write().await;
            lock.clone()
        };
        let gstreamer_from_map = temp_pipeline_map.remove(&meeting_id);

        info!("In end_meeting, removed gstreamer pipeline from map for meeting_id: {}", meeting_id.clone());
        if let Some(gstreamer_from_map) = gstreamer_from_map {
            let gstreamer = gstreamer_from_map.clone().unwrap();
            let pipeline_lock = gstreamer.pipeline.as_ref().unwrap();
            let pipeline_arc_clone = Arc::clone(&pipeline_lock);
            tokio::task::spawn_blocking(move || {
                let mut pipeline_guard = pipeline_arc_clone.lock().unwrap();
                match pipeline_guard.set_state(Null) {
                    Ok(_) => {
                        info!("Pipeline set to Null successfully")

                    }
                    Err(err) => {
                        error!("Failed to set pipeline to Null: {}", err);
                    }
                }
            });
        }
        // {
        //     if let Some(mut aws_service) = self.inner.meeting_cloud_upload_gstreamer_instance_map.write().await.remove(meeting_id.as_str()) {
        //         info!("Trying to stop file upload thread to cloud for meeting_id: {}", meeting_id.clone());
        //         aws_service.lock().await.set_stop_upload_thread_to_cloud(true).await;
        //     }
        // }
        info!("Successfully ended the meeting with meeting_id: {}", meeting_id.clone());
        Ok("Success ended the meeting".to_string())
    }

    /// Retrieve the IDs of all producers belonging to a participant.
    ///
    /// This queries the in-memory producer map and returns the IDs of all
    /// active producers for the given participant.
    pub async fn get_participant_producers(&mut self, participant_id: String) -> Vec<String> {
        let producer_map = {
            let lock = self.inner.producer_map.read().await;
            lock.clone()
        };
        producer_map.into_iter().filter(|(_, v)|
            v.participant_id == participant_id).map(|(_, v)| v.producer.id().to_string()).collect()
    }

    /// Retrieve the IDs of all consumers belonging to a participant.
    ///
    /// This queries the in-memory consumer map (shard) and returns the IDs of
    /// all active consumers for the given participant in the specified meeting.
    pub async fn get_participant_consumers(&mut self, participant_id: String, meeting_id: String) -> Vec<String> {
        let shard = self.get_shard(&meeting_id.clone());
        let consumer_map = {
            let lock = shard.read().await;
            lock.clone()
        };
        consumer_map.into_iter().filter(|(_, v)|
            v.participant_id == participant_id).map(|(_, v)| v.consumer.id().to_string()).collect()
    }

    /// Retrieve the IDs of all producers for a participant in a meeting instance.
    ///
    /// This queries the in-memory producer map and filters by participant ID,
    /// meeting ID and instance ID, returning the matching producer IDs.
    pub async fn get_participant_producers_for_instance_in_meeting(&mut self, participant_id: String, meeting_id: String, instance_id: String) -> Vec<String> {
        let producer_map = {
            let lock = self.inner.producer_map.read().await;
            lock.clone()
        };
        producer_map.into_iter().filter(|(_, v)|
            v.participant_id == participant_id && v.meeting_id == meeting_id && v.instance_id == instance_id).map(|(_, v)| v.producer.id().to_string()).collect()
    }

    /// Retrieve the IDs of all consumers for a participant in a meeting instance.
    ///
    /// This queries the in-memory consumer map (shard) and filters by participant
    /// ID, meeting ID and instance ID, returning the matching consumer IDs.
    pub async fn get_participant_consumers_for_instance_in_meeting(&mut self, participant_id: String, meeting_id: String, instance_id: String) -> Vec<String> {
        let shard = self.get_shard(&meeting_id.clone());
        let consumer_map = {
            let lock = shard.read().await;
            lock.clone()
        };
        consumer_map.into_iter().filter(|(_, v)|
            v.participant_id == participant_id && v.meeting_id == meeting_id && v.instance_id == instance_id).map(|(_, v)| v.consumer.id().to_string()).collect()
    }

    /// Retrieve the IDs of all producers for a participant in a meeting, excluding
    /// a specific instance.
    ///
    /// This is useful to find producers that should be kept alive when recreating
    /// transports for a participant.
    pub async fn get_participant_producers_excluding_instance(&mut self, participant_id: String, meeting_id: String, instance_id: String) -> Vec<String> {
        let producer_map = {
            let lock = self.inner.producer_map.read().await;
            lock.clone()
        };
        producer_map.into_iter().filter(|(_, v)|
            v.participant_id == participant_id && v.meeting_id == meeting_id && v.instance_id != instance_id).map(|(_, v)| v.producer.id().to_string()).collect()
    }

    /// Retrieve the IDs of all consumers that depend on a set of producer IDs.
    ///
    /// This is used to find and close consumers that are no longer needed when
    /// a producer is stopped or removed.
    pub async fn get_consumers_for_producer_ids(&mut self, producer_ids: Vec<String>, meeting_id: String) -> Vec<String> {
        let shard = self.get_shard(&meeting_id.clone());
        let consumer_map = {
            let lock = shard.read().await;
            lock.clone()
        };
        consumer_map.into_iter().filter(|(_, v)|
            producer_ids.contains(&v.producer_id)).map(|(_, v)| v.consumer.id().to_string()).collect()
    }

    /// Retrieve the IDs of all consumers for a participant in a meeting, excluding
    /// a specific instance.
    ///
    /// This is useful to find consumers that should be closed when a participant
    /// leaves or changes configuration.
    pub async fn get_participant_consumers_excluding_instance(&mut self, participant_id: String, meeting_id: String, instance_id: String) -> Vec<String> {
        let shard = self.get_shard(&meeting_id.clone());
        let consumer_map = {
            let lock = shard.read().await;
            lock.clone()
        };
        consumer_map.into_iter().filter(|(_, v)|
            v.participant_id == participant_id && v.meeting_id == meeting_id && v.instance_id != instance_id).map(|(_, v)| v.consumer.id().to_string()).collect()
    }

    /// Retrieve the IDs of all producers in a meeting.
    ///
    /// This queries the in-memory producer map and returns the IDs of all
    /// active producers for the given meeting.
    pub async fn get_all_producers_for_meeting(&mut self, meeting_id: String) -> Vec<String> {
        let producer_map = {
            let lock = self.inner.producer_map.read().await;
            lock.clone()
        };
        producer_map.into_iter().filter(|(_, v)|
            v.meeting_id == meeting_id).map(|(_, v)| v.producer.id().to_string()).collect()
    }

    /// Retrieve the IDs of all consumers in a meeting.
    ///
    /// This queries the in-memory consumer map (shard) and returns the IDs of
    /// all active consumers for the given meeting.
    pub async fn get_all_consumers_for_meeting(&mut self, meeting_id: String) -> Vec<String> {
        let shard = self.get_shard(&meeting_id.clone());
        let consumer_map = {
            let lock = shard.read().await;
            lock.clone()
        };
        consumer_map.into_iter().filter(|(_, v)|
            v.meeting_id == meeting_id).map(|(_, v)| v.consumer.id().to_string()).collect()
    }

    /// Get the shard for a given meeting ID.
    ///
    /// This hashes the meeting ID and selects the corresponding shard from the
    /// internal consumer map shards. It panics if the shard index is out of
    /// bounds, which should never happen if the system is configured correctly.
    pub fn get_shard(&self, meeting_id: &str) -> &Arc<RwLock<HashMap<String, ConsumerInMemory>>> {
        let mut hasher = DefaultHasher::new();
        meeting_id.hash(&mut hasher);
        let index = (hasher.finish() as usize) % self.inner.consumer_map_shards.len();
        &self.inner.consumer_map_shards[index]
    }
}
