//! GStreamer integration helpers and pipeline builders used by the media server.
//!
//! This module contains a lightweight wrapper around GStreamer (via the
//! `gstreamer` crates) used to build and manage recording pipelines for audio
//! and video RTP streams. The `Gstreamer` struct owns pipeline state and maps
//! per-participant dynamic elements (queues, depay/decoders, compositor pads,
//! etc.).
//!
//! Public responsibilities:
//! - Construct and manage a long-lived `gstreamer::Pipeline` shared across
//!   recordings.
//! - Create and attach dynamic elements when new RTP pads appear on `rtpbin`.
//! - Provide helpers to start audio (Opus), VP8 and H.264 recording pipelines
//!   and to gracefully handle element failures and cleanup.
//!
//! Concurrency and ownership notes:
//! - Many internal maps use `Arc<Mutex<...>>` to allow concurrent mutation from
//!   callback contexts (e.g. pad-added handlers) and async callers. Callers
//!   should be careful to avoid deadlocks when holding multiple locks.
//! - The module prefers to reuse long-lived elements (mixer, compositor,
//!   splitmuxsink) rather than recreating them for each participant.

use std::{fmt, fs, thread};
use std::collections::HashMap;
use std::fmt::{format, Formatter};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use aws_config::BehaviorVersion;
use aws_sdk_s3::Client;
use event_listener_primitives::HandlerId;
use futures::future::err;
use gstreamer::{glib, Buffer, Clock, ClockTime, Element, ElementFactory, Format, Pad, PadProbeReturn, PadProbeType, Pipeline, State};
use gstreamer::ffi::GstCaps;
use gstreamer::glib::{SignalHandlerId, Value};
use gstreamer::prelude::{Cast, ElementExt, ElementExtManual, GObjectExtManualGst, GstBinExt, GstBinExtManual, GstObjectExt, ObjectExt, PadExt, PadExtManual, PipelineExt};
use gstreamer::State::{Null, Paused, Playing};
use gstreamer_app::AppSrc;
use gstreamer_sdp::{SDPConnection, SDPMedia, SDPMessage};
use gstreamer_video::VideoConverterConfig;
use log::{debug, error, info};
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Meter};
use tokio::runtime::{Builder, Runtime};
use walkdir::WalkDir;

/// Top-level type representing the GStreamer subsystem used by the server.
///
/// `Gstreamer` contains shared handles to the runtime `Pipeline` and an
/// `Inner` struct that stores maps of dynamic elements keyed by participant.
/// Typical usage:
///
/// - Construct with `Gstreamer::new().await`.
/// - Call `start_audio_recording_gstreamer`, `start_vp8_video_recording_gstreamer`
///   or `start_h264_video_recording_gstreamer` to create and attach elements
///   for a specific participant/port.
/// - When an RTP source fails, call `handle_udp_src_failure` to remove and
///   clean up associated elements and pads.
#[derive(Debug, Clone)]
pub struct Gstreamer {
    pub inner: Arc<Mutex<Inner>>,
    pub audio_handler_id: Option<Arc<Mutex<SignalHandlerId>>>,
    pub video_handler_id: Option<Arc<Mutex<SignalHandlerId>>>,
    pub handler_id: Option<Arc<Mutex<SignalHandlerId>>>,
    pub pipeline: Option<Arc<Mutex<Pipeline>>>,
    pub meeting_id: Option<String>,
    pub participant_id: Option<String>,
}

/// Metadata for an active participant session attached to an `rtpbin` session.
///
/// Used to track which participant/meeting a dynamically requested sink pad
/// belongs to, so the pad can be released or linked to per-participant
/// elements.
pub struct ParticipantSessionMeta {
    pub meeting_id: String,
    pub participant_id: String,
    pub media: String,
    pub port: i32,
    pub rtp_sink_pad: gstreamer::Pad,
    pub rtcp_sink_pad: gstreamer::Pad,
}

/// Internal shared state of the GStreamer subsystem.
///
/// The `Inner` struct stores maps of dynamic elements, requested pads, and
/// session mappings. Fields are intentionally wrapped with `Arc<Mutex<...>>`
/// so they can be safely shared across the async code paths and the
/// synchronous GStreamer callbacks.
pub struct Inner {
    pub session_id: Arc<Mutex<Option<u32>>>,
    pub udp_src_element_audio_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    pub udp_rtcp_src_element_audio_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    // pub rtp_bin_element_audio_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    // pub rtp_bin_element_audio: Option<Arc<Mutex<gstreamer::Element>>>,
    pub udp_rtcp_sink_sink_element_audio_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    pub queue_element_audio_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    pub rtp_opus_de_pay_element_audio_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    pub opus_dec_element_audio_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    pub opus_enc_element_audio: Option<Arc<Mutex<gstreamer::Element>>>,
    pub audio_mixed_pad_map: Arc<Mutex<HashMap<String, gstreamer::Pad>>>,
    // pub udp_src_element_video: Option<Arc<Mutex<gstreamer::Element>>>,
    pub udp_src_element_video_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    pub udp_rtcp_src_element_video_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    // pub rtp_bin_element_video: Option<Arc<Mutex<gstreamer::Element>>>,
    // pub rtp_bin_element_video_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    // pub queue_element_vp8_video: Option<Arc<Mutex<gstreamer::Element>>>,
    pub queue_element_vp8_video_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    // pub rtp_vp8_de_pay_element_video: Option<Arc<Mutex<gstreamer::Element>>>,
    pub rtp_vp8_de_pay_element_video_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    // pub vp8_dec_element_video: Option<Arc<Mutex<gstreamer::Element>>>,
    pub vp8_dec_element_video_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    // pub udp_rtcp_sink_element_video: Option<Arc<Mutex<gstreamer::Element>>>,
    pub udp_rtcp_sink_element_video_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    // pub caps_filter_element_video: Option<Arc<Mutex<gstreamer::Element>>>,
    pub caps_filter_element_video_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    // pub video_scale_element_video: Option<Arc<Mutex<gstreamer::Element>>>,
    pub video_scale_element_video_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    // pub video_convert_element_video: Option<Arc<Mutex<gstreamer::Element>>>,
    pub video_convert_element_video_map: Arc<Mutex<HashMap<String, gstreamer::Element>>>,
    pub video_compositor_element: Option<Arc<Mutex<gstreamer::Element>>>,

    pub video_compositor_pad_map: Arc<Mutex<HashMap<String, gstreamer::Pad>>>,
    pub video_scale_post_compositor_element: Option<Arc<Mutex<gstreamer::Element>>>,
    pub caps_filter_post_compositor_element: Option<Arc<Mutex<gstreamer::Element>>>,
    pub x264_encode_element_video: Option<Arc<Mutex<gstreamer::Element>>>,
    pub audio_mixer_element: Option<Arc<Mutex<gstreamer::Element>>>,
    pub split_mux_sink_element: Option<Arc<Mutex<gstreamer::Element>>>,
    pub split_mux_sink_video_pad: Option<Arc<Mutex<gstreamer::Pad>>>,
    pub audio_pads_in_progress: Arc<Mutex<u32>>,
    pub rtp_bin_element : Option<Arc<Mutex<gstreamer::Element>>>,

    pub session_participant_meta_map: Arc<Mutex<HashMap<u32, ParticipantSessionMeta>>>,
    pub session_id_ssrc_map: Arc<Mutex<HashMap<u32, Vec<u32>>>>,
}

impl fmt::Debug for Inner {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Inner")
            .field("session_id", &self.session_id)
            .finish()
    }
}

impl Gstreamer {
    /// Create a new `Gstreamer` instance with empty internal maps.
    ///
    /// This is an `async` constructor for convenience with surrounding async
    /// code paths; it performs no blocking GStreamer initialization but sets
    /// up the internal `Inner` maps and default `None` pipeline.
    pub async fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                session_id: Arc::new(Mutex::new(None)),
                udp_src_element_video_map: Arc::new(Default::default()),
                queue_element_audio_map: Arc::new(Default::default()),
                rtp_opus_de_pay_element_audio_map: Arc::new(Default::default()),
                opus_dec_element_audio_map: Arc::new(Default::default()),
                opus_enc_element_audio: None,
                video_compositor_element: None,
                video_compositor_pad_map: Arc::new(Default::default()),
                audio_mixer_element: None,
                split_mux_sink_element: None,
                split_mux_sink_video_pad: None,
                queue_element_vp8_video_map: Arc::new(Default::default()),
                rtp_vp8_de_pay_element_video_map: Arc::new(Default::default()),
                /*queue_element_vp8_video: None,
                rtp_vp8_de_pay_element_video: None,
                vp8_dec_element_video: None,
                video_convert_element_video: None,
                video_scale_element_video: None,
                caps_filter_element_video: None,
                udp_rtcp_sink_element_video: None,*/
                x264_encode_element_video: None,
                udp_src_element_audio_map: Arc::new(Default::default()),
                udp_rtcp_src_element_audio_map: Arc::new(Default::default()),
                // rtp_bin_element_audio_map: Arc::new(Default::default()),
                // rtp_bin_element_audio: None,
                udp_rtcp_sink_sink_element_audio_map: Arc::new(Default::default()),
                audio_pads_in_progress: Arc::new(Mutex::new(0)),
                audio_mixed_pad_map: Arc::new(Default::default()),
                // rtp_bin_element_video_map: Arc::new(Default::default()),
                vp8_dec_element_video_map: Arc::new(Default::default()),
                caps_filter_element_video_map: Arc::new(Default::default()),
                video_scale_element_video_map: Arc::new(Default::default()),
                video_convert_element_video_map: Arc::new(Default::default()),
                udp_rtcp_sink_element_video_map: Arc::new(Default::default()),
                // rtp_bin_element_video: None,
                udp_rtcp_src_element_video_map: Arc::new(Default::default()),
                video_scale_post_compositor_element: None,
                caps_filter_post_compositor_element: None,

                session_participant_meta_map: Arc::new(Default::default()),
                rtp_bin_element: None,
                session_id_ssrc_map: Arc::new(Default::default()),
            })),
            audio_handler_id: None,
            video_handler_id: None,
            handler_id: None,
            pipeline: None,
            meeting_id: None,
            participant_id: None,
        }
    }

    /// Remove and cleanup elements associated with a failing UDP source.
    ///
    /// This function removes the UDP src/rtcp elements from the pipeline,
    /// releases any requested `rtpbin` pads and removes the session metadata
    /// entries for the given `participant_id` and `media` type.
    ///
    /// It attempts to set each element to the `Null` state before removal
    /// and logs any failure encountered during cleanup.
    pub fn handle_udp_src_failure(mut self, participant_id: String, media: String) {
        let gstreamer_inner = self.inner.lock().unwrap();
        let udp_src_element = {
            if media == "audio" {
                gstreamer_inner.udp_src_element_audio_map.lock().unwrap().remove(&participant_id.clone())
            } else {
                gstreamer_inner.udp_src_element_video_map.lock().unwrap().remove(&participant_id.clone())
            }
        };
        let udp_rtcp_element = {
            if media == "audio" {
                gstreamer_inner.udp_rtcp_src_element_audio_map.lock().unwrap().remove(&participant_id.clone())
            } else {
                gstreamer_inner.udp_rtcp_src_element_video_map.lock().unwrap().remove(&participant_id.clone())
            }
        };

        let rtp_bin_element = gstreamer_inner.rtp_bin_element.as_ref().unwrap().lock().unwrap().clone();
        let mut rtp_bin_sink_pads = Vec::new();
        let mut rtcp_bin_sink_pads = Vec::new();
        let mut session_ids = Vec::new();
        {
            gstreamer_inner.session_participant_meta_map.lock().unwrap().retain(|key, value| {
                let keep = !(participant_id.clone() == value.participant_id.clone() && media == value.media.clone());
                if !keep {
                    rtp_bin_sink_pads.push(value.rtp_sink_pad.clone());
                    rtcp_bin_sink_pads.push(value.rtcp_sink_pad.clone());
                    session_ids.push(key.clone());
                }
                keep
            });
        }
        let pipeline = self.pipeline.unwrap().lock().unwrap().clone();
        pipeline.abort_state();
        match udp_src_element {
            Some(udp_src) => {
                match udp_src.set_state(State::Null) {
                    Ok(_) => {
                        info!("Successfully set status as Null for failing udp src element");
                    }
                    Err(e) => {
                        error!("Failed to set status as Null for failing udp src element {}", e);
                    }
                }
                match pipeline.remove(&udp_src) {
                    Ok(_) => {
                        info!("Successfully removed failing udp src element from pipeline");
                    }
                    Err(e) => {
                        error!("Failed to remove failing udp src element from pipeline {}", e);
                    }
                }
            }
            None => {}
        }
        match udp_rtcp_element {
            Some(udp_rtcp) => {
                match udp_rtcp.set_state(State::Null) {
                    Ok(_) => {
                        info!("Successfully set status as Null for failing udp rtcp element");
                    }
                    Err(e) => {
                        error!("Failed to set status as Null for failing udp rtcp element {}", e);
                    }
                }
                match pipeline.remove(&udp_rtcp) {
                    Ok(_) => {
                        info!("Successfully removed failing udp rtcp element from pipeline");
                    }
                    Err(e) => {
                        error!("Failed to remove failing udp rtcp element from pipeline {}", e);
                    }
                }
            }
            None => {}
        }
        for sink_pad in rtcp_bin_sink_pads {
            info!("Releasing rtcp sink pad for rtp_bin element");
            rtp_bin_element.release_request_pad(&sink_pad);
        }
        for sink_pad in rtp_bin_sink_pads {
            info!("Releasing rtp sink pad for rtp_bin element");
            rtp_bin_element.release_request_pad(&sink_pad);
        }
    }
    /// Start an audio recording pipeline (Opus) for the given participant.
    ///
    /// This method will create or reuse long-lived elements (rtpbin, mixer,
    /// opus encoder, splitmuxsink), attach UDP sources, link pads and set up
    /// metrics counters. On success it returns the same `Gstreamer` instance
    /// with an attached pipeline.
    ///
    /// # Arguments
    /// - `audio_port`: UDP port for incoming RTP audio.
    /// - `meeting_id`, `participant_id`: identifiers used for file naming
    ///   and metric labels.
    /// - `transport_id`: optional transport identifier (currently unused
    ///   internally but accepted for API parity).
    /// - `clock`: gstreamer `Clock` used as the pipeline clock.
    /// - `meter`: opentelemetry `Meter` used to create counters.
    ///
    /// # Returns
    /// `Result<Self, String>` with `Ok(self)` when pipeline creation/attachment
    /// succeeds or `Err` with a string message on failure.
    pub async fn start_audio_recording_gstreamer(mut self,
                                                 audio_port: i32,
                                                 meeting_id: String,
                                                 participant_id: String,
                                                 transport_id: String,
                                                 clock: Clock, meter: Meter) -> Result<Self, String> {
        //gstreamer::init().expect("Failed to initialize GStreamer");
        self.meeting_id = Some(meeting_id.clone());
        self.participant_id = Some(participant_id.clone());
        //let pipeline_internal = gstreamer::Pipeline::with_name("Audio Pipeline");
        let mut temp_pipeline = self.pipeline.clone();
        let pipeline = temp_pipeline.get_or_insert_with(|| {
            info!("Creating new pipeline");
            let pipeline_internal = gstreamer::Pipeline::with_name("Pipeline");
            pipeline_internal.use_clock(Some(&clock));
            Arc::new(Mutex::new(pipeline_internal))
        });
        //let pipeline = Arc::new(Mutex::new(pipeline_internal));
        let element_failure_counter: Counter<u64> = meter
            .u64_counter("metric_".to_string() + meeting_id.clone().to_string().as_str() + "_element_failures")
            .with_description("Counts the number of element creation failures")
            .init();
        let linking_failure_counter: Counter<u64> = meter
            .u64_counter("metric_".to_string() + meeting_id.clone().to_string().as_str() + "_linking_failures")
            .with_description("Counts the number of element linking failures")
            .init();
        let pad_request_failure_counter: Counter<u64> = meter
            .u64_counter("metric_".to_string() + meeting_id.clone().to_string().as_str() + "_pad_request_failures")
            .with_description("Counts the number of pad request failures")
            .init();
        let pipeline_failure_counter: Counter<u64> = meter
            .u64_counter("metric_".to_string() + meeting_id.clone().to_string().as_str() + "_pipeline_failures")
            .with_description("Counts the number of pipeline failures")
            .init();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        /* ------------------- Creation of the elements  ------------------- */
        let audio_caps = gstreamer::Caps::builder("application/x-rtp")
            .field("media", "audio")
            .field("clock-rate", 48000)
            .field("encoding-name", "OPUS")
            .field("payload", 111)
            // .field("ssrc", &"4278448574")
            // .field("cname", &"demo-1")
            .build();

        let udp_src_rtp_element = match ElementFactory::make("udpsrc")
            .name(format!("udpsrc_rtp_audio_{}_{}", participant_id, timestamp)).build() {
            Ok(element) => element,
            Err(e) => {
                element_failure_counter.add(1, &vec![KeyValue::new("add", "udpsrc_rtp_audio")]);
                error!("Failed to create udpsrc element {:?}", e);
                return Err("Failed to create udpsrc element".to_string());
            }
        };
        udp_src_rtp_element.set_property("address", &"127.0.0.1");
        udp_src_rtp_element.set_property("port", &audio_port);
        udp_src_rtp_element.set_property("caps", &audio_caps);
        udp_src_rtp_element.set_property("reuse", &false);
        // self.inner.lock().unwrap().udp_src_element_audio = Some(Arc::new(Mutex::new(udp_src_rtp_element.clone())));
        self.inner.lock().unwrap().udp_src_element_audio_map.lock().unwrap().insert(
            participant_id.clone(),
            udp_src_rtp_element.clone(),
        );
        let udp_src_rtcp_element = match ElementFactory::make("udpsrc")
            .name(format!("udpsrc_rtcp_audio_{}_{}", participant_id, timestamp)).build() {
            Ok(element) => element,
            Err(e) => {
                element_failure_counter.add(1, &vec![KeyValue::new("add", "udpsrc_rtcp_audio")]);
                error!("Failed to create udpsrc element {:?}", e);
                return Err("Failed to create udpsrc element".to_string());
            }
        };
        udp_src_rtcp_element.set_property("address", &"127.0.0.1");
        udp_src_rtcp_element.set_property("port", &audio_port + 1);
        udp_src_rtcp_element.set_property("reuse", &false);
        self.inner.lock().unwrap().udp_rtcp_src_element_audio_map.lock().unwrap().insert(
            participant_id.clone(),
            udp_src_rtcp_element.clone(),
        );
        let udp_rtcp_sink_element = match ElementFactory::make("udpsink")
            .name(format!("udpsink_rtcp_audio_{}_{}", participant_id, timestamp)).build() {
            Ok(element) => element,
            Err(e) => {
                element_failure_counter.add(1, &vec![KeyValue::new("add", "udpsink_rtcp_audio")]);
                error!("Failed to create udpsink element {:?}", e);
                return Err("Failed to create udpsink element".to_string());
            }
        };
        udp_rtcp_sink_element.set_property("host", &"127.0.0.1");
        udp_rtcp_sink_element.set_property("port", &audio_port + 1);
        udp_rtcp_sink_element.set_property("async", &false);
        udp_rtcp_sink_element.set_property("sync", &false);
        let (rtp_bin_element, rtp_bin_element_new) = {
            if self.inner.lock().unwrap().rtp_bin_element.is_some() {
                info!("RtpBin element already exists, will not create a new one");
                (self.inner.lock().unwrap().rtp_bin_element.as_ref().unwrap().lock().unwrap().clone(), false)
            } else {
                info!("RtpBin element is not initialized, will create a new one");
                let rtp_bin_element = match ElementFactory::make("rtpbin")
                    .name(format!("rtpbin_audio_{}", timestamp))
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "rtpbin_audio")]);
                        error!("Failed to create rtpbin element {:?}", e);
                        return Err("Failed to create rtpbin element".to_string());
                    }
                };
                rtp_bin_element.set_property_from_value("latency", &Value::from(1000u32));
                let rtp_bin_element_temp = Arc::new(Mutex::new(rtp_bin_element.clone()));
                self.inner.lock().unwrap().rtp_bin_element = Some(rtp_bin_element_temp);
                (rtp_bin_element, true)
            }
        };
        let (audio_mixer_element, audio_mixer_element_new) = {
            if self.inner.lock().unwrap().audio_mixer_element.is_some() {
                info!("Audio mixer element already exists, will not create a new one");
                (self.inner.lock().unwrap().audio_mixer_element.as_ref().unwrap().lock().unwrap().clone(), false)
            } else {
                info!("Audio mixer element is not initialized, will create a new one");
                let audio_mixer_element = match ElementFactory::make("audiomixer")
                    .name(format!("audiomixer_{}", timestamp))
                    .property_from_str("force-live", "true")
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "audiomixer")]);
                        error!("Failed to create audiomixer element {:?}", e);
                        return Err("Failed to create audiomixer element".to_string());
                    }
                };
                audio_mixer_element.set_property_from_str("ignore-inactive-pads", "true");
                let audio_mixer_element_temp = Arc::new(Mutex::new(audio_mixer_element.clone()));
                self.inner.lock().unwrap().audio_mixer_element = Some(audio_mixer_element_temp);
                (audio_mixer_element, true)
            }
        };
        let (opus_enc_element_audio, opus_enc_element_audio_new) = {
            if self.inner.lock().unwrap().opus_enc_element_audio.is_some() {
                info!("OpusEnc element already exists, will not create a new one");
                (self.inner.lock().unwrap().opus_enc_element_audio.as_ref().unwrap().lock().unwrap().clone(), false)
            } else {
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                info!("OpusEnc element is not initialized, will create a new one");
                let opus_enc_element = match ElementFactory::make("opusenc")
                    .name(format!("opusenc_audio_{}", timestamp)).build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "opusenc_audio")]);
                        error!("Failed to create opusenc element {:?}", e);
                        return Err("Failed to create opusenc element".to_string());
                    }
                };
                let opus_enc_element_temp = Arc::new(Mutex::new(opus_enc_element.clone()));
                self.inner.lock().unwrap().opus_enc_element_audio = Some(opus_enc_element_temp);
                (opus_enc_element, true)
            }
        };
        let (compositor_video_element_element, compositor_video_element_element_new) = {
            if self.inner.lock().unwrap().video_compositor_element.is_some() {
                info!("Compositor element already exists, will not create a new one");
                let compositor_video = self.inner.lock().unwrap().video_compositor_element.clone().unwrap();
                let x = (compositor_video.lock().unwrap().clone(), false);
                x
            } else {
                let new_compositor_video_element = match ElementFactory::make("compositor")
                    .name(format!("compositor_vp8_video_{}", timestamp))
                    .property_from_str("force-live", "true")
                    .property_from_str("background", "black")
                    .property_from_str("ignore-inactive-pads", "true")
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "compositor_vp8_video")]);
                        error!("Failed to create compositor element {:?}", e);
                        return Err("Failed to create compositor element".to_string());
                    }
                };
                let compositor_video_element_temp = Arc::new(Mutex::new(new_compositor_video_element.clone()));
                self.inner.lock().unwrap().video_compositor_element = Some(compositor_video_element_temp);
                (new_compositor_video_element, true)
            }
        };
        let (video_scale_post_compositor_element, video_scale_post_compositor_element_new) = {
            if self.inner.lock().unwrap().video_scale_post_compositor_element.is_some() {
                info!("Video scale element already exists, will not create a new one");
                let video_scale = self.inner.lock().unwrap().video_scale_post_compositor_element.clone().unwrap();
                let x = (video_scale.lock().unwrap().clone(), false);
                x
            } else {
                let new_video_scale_post_compositor_element = match ElementFactory::make("videoscale")
                    .name(format!("videoscale_vp8_post_compositor_video_{}", timestamp))
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "videoscale_vp8_post_compositor_video_")]);
                        error!("Failed to create videoscale element {:?}", e);
                        return Err("Failed to create videoscale element".to_string());
                    }
                };
                let video_scale_post_compositor_element_temp = Arc::new(Mutex::new(new_video_scale_post_compositor_element.clone()));
                self.inner.lock().unwrap().video_scale_post_compositor_element = Some(video_scale_post_compositor_element_temp);
                (new_video_scale_post_compositor_element, true)
            }
        };
        let (caps_filter_post_compositor_element, caps_filter_post_compositor_element_new) = {
            if self.inner.lock().unwrap().caps_filter_post_compositor_element.is_some() {
                info!("Caps filter element already exists, will not create a new one");
                let caps_filter_video = self.inner.lock().unwrap().caps_filter_post_compositor_element.clone().unwrap();
                let x = (caps_filter_video.lock().unwrap().clone(), false);
                x
            } else {
                let new_caps_filter_video_element = match ElementFactory::make("capsfilter")
                    .name(format!("caps_filter_vp8_post_compositor_video_{}", timestamp))
                    .property("caps", &gstreamer::Caps::builder("video/x-raw")
                        .field("width", 1280)
                        .field("height", 720)
                        .build())
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "capsfilter_vp8_post_compositor_video_")]);
                        error!("Failed to create capsfilter element {:?}", e);
                        return Err("Failed to create capsfilter element".to_string());
                    }
                };
                let caps_filter_video_element_temp = Arc::new(Mutex::new(new_caps_filter_video_element.clone()));
                self.inner.lock().unwrap().caps_filter_post_compositor_element = Some(caps_filter_video_element_temp);
                (new_caps_filter_video_element, true)
            }
        };
        let (x264_enc_element, x264_enc_element_new) = {
            if self.inner.lock().unwrap().x264_encode_element_video.is_some() {
                info!("X264Enc element already exists, will not create a new one");
                let x264_enc = self.inner.lock().unwrap().x264_encode_element_video.clone().unwrap();
                let x = (x264_enc.lock().unwrap().clone(), false);
                x
            } else {
                let new_x264_enc_element = match ElementFactory::make("x264enc")
                    .name(format!("x264enc_vp8_video_{}", timestamp))
                    .property_from_str("speed-preset", "ultrafast")
                    .property_from_str("tune", "zerolatency")
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "x264enc_vp8_video")]);
                        error!("Failed to create x264enc element {:?}", e);
                        return Err("Failed to create x264enc element".to_string());
                    }
                };
                let x264_enc_element_temp = Arc::new(Mutex::new(new_x264_enc_element.clone()));
                self.inner.lock().unwrap().x264_encode_element_video = Some(x264_enc_element_temp);
                (new_x264_enc_element, true)
            }
        };
        let (split_mux_sink_element, split_mux_sink_element_new) = {
            if self.inner.lock().unwrap().split_mux_sink_element.is_some() {
                info!("SplitMuxSink element already exists, will not create a new one");
                (self.inner.lock().unwrap().split_mux_sink_element.as_ref().unwrap().lock().unwrap().clone(), false)
            } else {
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                info!("SplitMuxSink element is not initialized, will create a new one");
                let muxer = match ElementFactory::make("mpegtsmux")
                    .name(format!("mpegtsmux_{}", timestamp))
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "mpegtsmux")]);
                        error!("Failed to create mpegtsmux element {:?}", e);
                        return Err("Failed to create mpegtsmux element".to_string());
                    }
                };
                let split_mux_sink_element = match ElementFactory::make("splitmuxsink")
                    .name(format!("splitmuxsink_{}", timestamp))
                    .property("max-size-time", ClockTime::from_seconds(30).nseconds())
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "splitmuxsink")]);
                        error!("Failed to create splitmuxsink element {:?}", e);
                        return Err("Failed to create splitmuxsink element".to_string());
                    }
                };
                split_mux_sink_element.set_property("muxer", &muxer);
                let meeting_id_clone = Arc::new(meeting_id.clone());
                let participant_id_clone = Arc::new(participant_id.clone());
                split_mux_sink_element.connect("format-location", false, move |values| {
                    // Generate the custom filename with a timestamp
                    let timestamp_nanos = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("Time went backwards")
                        .as_nanos();
                    let filename = format!("/opt/recordings/{}/chunk_{}.ts", meeting_id_clone, timestamp_nanos);
                    // Return the filename
                    Some(glib::Value::from(&filename))
                });
                let split_mux_sink_element_temp = Arc::new(Mutex::new(split_mux_sink_element.clone()));
                self.inner.lock().unwrap().split_mux_sink_element = Some(split_mux_sink_element_temp);
                (split_mux_sink_element, true)
            }
        };

        /* ------------------- Add elements to the pipeline ------------------- */
        match pipeline.lock().unwrap().set_state(Paused) {
            Ok(_) => {
                info!("Pipeline paused successfully");
            }
            Err(err) => {
                pipeline_failure_counter.add(1, &vec![]);
                error!("Failed to pause pipeline: {}", err);
                return Err("Failed to pause pipeline".to_string());
            }
        }
        match pipeline.lock().unwrap().add_many(&[&udp_src_rtp_element,
            &udp_src_rtcp_element,
            //&udp_rtcp_sink_element,
        ]) {
            Ok(_) => {
                info!("Elements added to the pipeline successfully");
            }
            Err(err) => {
                pipeline_failure_counter.add(1, &vec![]);
                error!("Failed to add elements to the pipeline: {}", err);
                return Err("Failed to add elements to the pipeline".to_string());
            }
        }
        if rtp_bin_element_new {
            match pipeline.lock().unwrap().add(&rtp_bin_element) {
                Ok(_) => {
                    info!("RtpBin element added to the pipeline successfully");
                }
                Err(err) => {
                    element_failure_counter.add(1, &vec![KeyValue::new("add", "rtpbin_audio")]);
                    error!("Failed to add rtpbin element to the pipeline: {}", err);
                    return Err("Failed to add rtpbin element to the pipeline".to_string());
                }
            }
        }
        if audio_mixer_element_new && opus_enc_element_audio_new {
            match pipeline.lock().unwrap().add_many(&[&audio_mixer_element, &opus_enc_element_audio]) {
                Ok(_) => {
                    info!("Audio mixer and opus decoder elements added to the pipeline successfully");
                }
                Err(err) => {
                    error!("Failed to add audio mixer and opus decoder elements to the pipeline: {}", err);
                    return Err("Failed to add audio mixer and opus decoder elements to the pipeline".to_string());
                }
            }
        } else if audio_mixer_element_new || opus_enc_element_audio_new {
            error!("Either audio mixer or opus decoder element is not new, cannot add to pipeline");
            return Err("Either audio mixer or opus decoder element is not new, cannot add to pipeline".to_string());
        }
        if compositor_video_element_element_new && x264_enc_element_new && video_scale_post_compositor_element_new && caps_filter_post_compositor_element_new {
            match pipeline.lock().unwrap().add_many(&[&compositor_video_element_element,
                &x264_enc_element, &video_scale_post_compositor_element, &caps_filter_post_compositor_element]) {
                Ok(_) => {
                    info!("Compositor and x264enc elements added to the pipeline successfully");
                }
                Err(err) => {
                    pipeline_failure_counter.add(1, &vec![KeyValue::new("add", "compositor_x264enc")]);
                    error!("Failed to add compositor and x264enc elements to the pipeline: {}", err);
                    return Err("Failed to add compositor and x264enc elements to the pipeline".to_string());
                }
            }
        } else if compositor_video_element_element_new || x264_enc_element_new || video_scale_post_compositor_element_new || caps_filter_post_compositor_element_new {
            error!("Either compositor, x264enc, video scale or caps filter element is not new, cannot add to pipeline");
            return Err("Either compositor, x264enc, video scale or caps filter element is not new, cannot add to pipeline".to_string());
        }
        if split_mux_sink_element_new {
            match pipeline.lock().unwrap().add(&split_mux_sink_element) {
                Ok(_) => {
                    info!("SplitMuxSink element added to the pipeline successfully");
                }
                Err(err) => {
                    element_failure_counter.add(1, &vec![KeyValue::new("add", "splitmuxsink")]);
                    error!("Failed to add splitmuxsink element to the pipeline: {}", err);
                    return Err("Failed to add splitmuxsink element to the pipeline".to_string());
                }
            }
        }
        /* ------------------- Start Linking of pads / elements ------------------- */
        let udp_src_rtp_src_pad = match udp_src_rtp_element.static_pad("src") {
            Some(pad) => pad,
            None => {
                pad_request_failure_counter.add(1, &[]);
                error!("Failed to get src pad from udpsrc_rtp_audio");
                return Err("Failed to get src pad from udpsrc_rtp_audio".to_string());
            }
        };
        let rtp_bin_sink_pad = match rtp_bin_element.request_pad_simple("recv_rtp_sink_%u") {
            Some(pad) => pad,
            None => {
                pad_request_failure_counter.add(1, &[]);
                error!("Failed to get sink pad from rtpbin");
                return Err("Failed to get sink pad from rtpbin".to_string());
            }
        };
        match udp_src_rtp_src_pad.link(&rtp_bin_sink_pad) {
            Ok(_) => {
                info!("Linked udpsrc and rtpbin successfully");
            }
            Err(err) => {
                linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                error!("Failed to link udpsrc and rtpbin: {}", err);
                return Err("Failed to link udpsrc and rtpbin".to_string());
            }
        }
        let udp_rtcp_src_pad = match udp_src_rtcp_element.static_pad("src") {
            Some(pad) => pad,
            None => {
                pad_request_failure_counter.add(1, &[]);
                error!("Failed to get src pad from udpsrc_rtcp_audio");
                return Err("Failed to get src pad from udpsrc_rtcp_audio".to_string());
            }
        };
        let rtp_bin_rtcp_sink_pad = match rtp_bin_element.request_pad_simple("recv_rtcp_sink_%u") {
            Some(pad) => pad,
            None => {
                pad_request_failure_counter.add(1, &[]);
                error!("Failed to get sink pad from rtpbin");
                return Err("Failed to get sink pad from rtpbin".to_string());
            }
        };
        let sink_pad_name = rtp_bin_sink_pad.name();
        let sink_pad_session: u32 = sink_pad_name
            .strip_prefix("recv_rtp_sink_")
            .unwrap()
            .parse()
            .unwrap();
        self.inner.lock().unwrap().session_participant_meta_map.lock().unwrap().insert(
            sink_pad_session,
            ParticipantSessionMeta {
                meeting_id: meeting_id.clone(),
                participant_id: participant_id.clone(),
                media: "audio".to_string(),
                port: audio_port,
                rtp_sink_pad: rtp_bin_sink_pad.clone(),
                rtcp_sink_pad: rtp_bin_rtcp_sink_pad.clone(),
            },
        );
        match udp_rtcp_src_pad.link(&rtp_bin_rtcp_sink_pad) {
            Ok(_) => {
                info!("Linked udpsrc_rtcp and rtpbin successfully");
            }
            Err(err) => {
                linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                error!("Failed to link udpsrc_rtcp and rtpbin: {}", err);
                return Err("Failed to link udpsrc_rtcp and rtpbin".to_string());
            }
        }
        if audio_mixer_element_new {
            let audio_mixer_src_pad = match audio_mixer_element.static_pad("src") {
                Some(pad) => pad,
                None => {
                    pad_request_failure_counter.add(1, &[]);
                    error!("Failed to get sink pad from audiomixer");
                    return Err("Failed to get sink pad from audiomixer".to_string());
                }
            };
            let opus_enc_sink_pad = match opus_enc_element_audio.static_pad("sink") {
                Some(pad) => pad,
                None => {
                    pad_request_failure_counter.add(1, &[]);
                    error!("Failed to get sink pad from opusdec");
                    return Err("Failed to get sink pad from opusdec".to_string());
                }
            };
            let opus_enc_src_pad = match opus_enc_element_audio.static_pad("src") {
                Some(pad) => pad,
                None => {
                    pad_request_failure_counter.add(1, &[]);
                    error!("Failed to get src pad from opusdec");
                    return Err("Failed to get src pad from opusdec".to_string());
                }
            };
            let split_mux_sink_video_pad = {
                self.inner.lock().unwrap().split_mux_sink_video_pad.clone()
            };
            if split_mux_sink_video_pad.is_none() {
                let compositor_video_src_pad = match compositor_video_element_element.static_pad("src") {
                    Some(pad) => pad,
                    None => {
                        error!("Failed to get src pad from compositor element");
                        return Err("Failed to get src pad from compositor element".to_string());
                    }
                };
                let video_scale_post_compositor_sink_pad = match video_scale_post_compositor_element.static_pad("sink") {
                    Some(pad) => pad,
                    None => {
                        error!("Failed to get sink pad from videoscale element");
                        return Err("Failed to get sink pad from videoscale element".to_string());
                    }
                };
                let x264_enc_src_pad = match x264_enc_element.static_pad("src") {
                    Some(pad) => pad,
                    None => {
                        error!("Failed to get src pad from x264enc element");
                        return Err("Failed to get src pad from x264enc element".to_string());
                    }
                };
                let split_mux_sink_video_pad = match split_mux_sink_element.request_pad_simple("video") {
                    Some(pad) => pad,
                    None => {
                        error!("Failed to get video pad from splitmuxsink");
                        return Err("Failed to get video pad from splitmuxsink".to_string());
                    }
                };
                self.inner.lock().unwrap().split_mux_sink_video_pad = Some(Arc::new(Mutex::new(split_mux_sink_video_pad.clone())));
                match compositor_video_src_pad.link(&video_scale_post_compositor_sink_pad) {
                    Ok(_) => {
                        info!("Linked compositor and videoscale successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link compositor and x264enc: {}", err);
                        return Err("Failed to link compositor and videoscale".to_string());
                    }
                }
                match video_scale_post_compositor_element.link(&caps_filter_post_compositor_element) {
                    Ok(_) => {
                        info!("Linked videoscale and capsfilter successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link videoscale and capsfilter: {}", err);
                        return Err("Failed to link videoscale and capsfilter".to_string());
                    }
                }
                match caps_filter_post_compositor_element.link(&x264_enc_element) {
                    Ok(_) => {
                        info!("Linked capsfilter and x264enc successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link capsfilter and x264enc: {}", err);
                        return Err("Failed to link capsfilter and x264enc".to_string());
                    }
                }
                match x264_enc_src_pad.link(&split_mux_sink_video_pad) {
                    Ok(_) => {
                        info!("Linked x264enc and splitmuxsink successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link x264enc and splitmuxsink: {}", err);
                        return Err("Failed to link x264enc and splitmuxsink".to_string());
                    }
                }
            }
            let split_mux_sink_audio_pad = match split_mux_sink_element.request_pad_simple("audio_%u") {
                Some(pad) => pad,
                None => {
                    pad_request_failure_counter.add(1, &[]);
                    error!("Failed to get sink pad from splitmuxsink");
                    return Err("Failed to get sink pad from splitmuxsink".to_string());
                }
            };

            match audio_mixer_src_pad.link(&opus_enc_sink_pad) {
                Ok(_) => {
                    info!("Linked audiomixer src pad to opusdec sink pad successfully");
                }
                Err(err) => {
                    error!("Failed to link audiomixer src pad to opusdec sink pad: {}", err);
                    return Err("Failed to link audiomixer src pad to opusdec sink pad".to_string());
                }
            }
            match opus_enc_src_pad.link(&split_mux_sink_audio_pad) {
                Ok(_) => {
                    info!("Linked opusdec src pad to splitmuxsink audio pad successfully");
                }
                Err(err) => {
                    linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                    error!("Failed to link opusdec src pad to splitmuxsink audio pad: {}", err);
                    return Err("Failed to link opusdec src pad to splitmuxsink audio pad".to_string());
                }
            }
        }
        self.pipeline = Some(pipeline.clone());
        let self_arc = Arc::new(Mutex::new(self.clone()));
        let self_clone = Arc::clone(&self_arc);
        let meeting_id_arc = Arc::new(meeting_id.clone());
        let meeting_id_clone = Arc::clone(&meeting_id_arc);
        let audio_mixer_element_clone = Arc::new(Mutex::new(audio_mixer_element.clone()));
        if rtp_bin_element_new {
            let handler_id = rtp_bin_element.connect_pad_added(move |rtpbin, pad| {
                Self::rtp_bin_element_handling(&self_clone, &element_failure_counter,
                                               &pipeline_failure_counter, &linking_failure_counter, &pad_request_failure_counter,
                                               rtpbin, pad);
            });
            self.handler_id = Some(Arc::new(Mutex::new(handler_id)));
        }

        /*---------------------------------------------------------------------*/

        /* ------------------- Debugging help like adding of probes ... ------------------- */
        /*---------------------------------------------------------------------*/
        /*pipeline.clone().lock().unwrap().iterate_elements().into_iter().for_each(|element| {
            info!("Element: {:#?}", element);
        });*/
        // self.pipeline = Some(pipeline.clone());
        Ok(self)
    }

    pub fn rtp_bin_element_handling(self_clone: &Arc<Mutex<Gstreamer>>, element_failure_counter: &Counter<u64>,
                                    pipeline_failure_counter: &Counter<u64>, linking_failure_counter: &Counter<u64>, pad_request_failure_counter: &Counter<u64>,
                                    rtpbin: &Element, pad: &Pad)
    {
        info!("Pad added: {}", pad.name());
        info!("{:#?}", pad);
        if pad.is_linked() {
            info!("Pad is already linked");
            return;
        }
        if pad.name().starts_with("recv_rtp_src_") {
            let self_clone = self_clone.lock().unwrap();
            let pipeline_unlock = self_clone.pipeline.as_ref().unwrap().lock().unwrap();
            let bus = pipeline_unlock.bus().unwrap();
            let pad_name = pad.name();
            let parts: Vec<&str> = pad_name.split('_').collect();
            let session_id = if parts.len() > 3 {
                parts[3].parse::<u32>().ok()
            } else {
                None
            };
            let ssrc = if parts.len() > 4 {
                parts[4].parse::<u32>().ok()
            } else {
                None
            };

            match pipeline_unlock.set_state(gstreamer::State::Paused) {
                Ok(_) => {
                    info!("Pipeline paused successfully");
                }
                Err(err) => {
                    pipeline_failure_counter.add(1, &vec![]);
                    error!("Failed to pause pipeline: {}", err);
                    bus.post(gstreamer::message::Error::new(
                        gstreamer::CoreError::Failed,
                        "Failed to pause pipeline",
                    )).unwrap();
                }
            }
            let inner_clone = self_clone.clone().inner.clone();
            let mut inner = inner_clone.lock().unwrap();

            let mut map = inner.session_id_ssrc_map.lock().unwrap();
            map.entry(session_id.unwrap()).or_insert_with(Vec::new).push(ssrc.unwrap());

            let session_participant_meta_map = inner.session_participant_meta_map.lock().unwrap();
            let mut participant_session_meta = session_participant_meta_map.get(&session_id.unwrap().clone()).unwrap().clone();

            if participant_session_meta.media == "video" {
                info!("{:#?}",pad.query_caps(None));
                info!("New pad added: {}", pad.name());
                let mut dynamic_timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                let mut dynamic_queue_element = match ElementFactory::make("queue")
                    .name(format!("queue_vp8_video_{}_{}", participant_session_meta.participant_id, dynamic_timestamp))
                    .property("flush-on-eos", true)
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "queue_vp8_video")]);
                        error!("Failed to create queue element {:?}", e);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to create queue element",
                        )).unwrap();
                        return ();
                    }
                };
                let mut dynamic_rtp_vp8_de_pay_element = match ElementFactory::make("rtpvp8depay")
                    .name(format!("rtpvp8depay_vp8_video_{}_{}", participant_session_meta.participant_id, dynamic_timestamp))
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "rtpvp8depay_vp8_video")]);
                        error!("Failed to create rtpvp8depay element {:?}", e);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to create rtpvp8depay element",
                        )).unwrap();
                        return ();
                    }
                };
                let mut dynamic_vp8_dec_element = match ElementFactory::make("vp8dec")
                    .name(format!("vp8dec_vp8_video_{}_{}", participant_session_meta.participant_id, dynamic_timestamp))
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "vp8dec_vp8_video")]);
                        error!("Failed to create vp8dec element {:?}", e);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to create vp8dec element",
                        )).unwrap();
                        return ();
                    }
                };
                let mut dynamic_caps_filter = match ElementFactory::make("capsfilter")
                    .name(format!("capsfilter_vp8_video_{}_{}", participant_session_meta.participant_id, dynamic_timestamp))
                    .property("caps", &gstreamer::Caps::builder("video/x-raw")
                        .field("width", 1280)
                        .field("height", 720)
                        .build())
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "capsfilter_vp8_video")]);
                        error!("Failed to create capsfilter element {:?}", e);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to create capsfilter element",
                        )).unwrap();
                        return ();
                    }
                };
                let mut dynamic_video_convert_element = match ElementFactory::make("videoconvert")
                    .name(format!("videoconvert_vp8_video_{}_{}", participant_session_meta.participant_id, dynamic_timestamp))
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "videoconvert_vp8_video")]);
                        error!("Failed to create videoconvert element {:?}", e);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed, // Error domain
                            "Failed to create videoconvert element", // Error message
                        )).expect("Failed to create videoconvert element");
                        return ();
                    }
                };
                let mut dynamic_video_scale_element = match ElementFactory::make("videoscale")
                    .name(format!("videoscale_vp8_video_{}_{}", participant_session_meta.participant_id, dynamic_timestamp))
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "videoscale_vp8_video")]);
                        error!("Failed to create videoscale element {:?}", e);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed, // Error domain
                            "Failed to create videoscale element", // Error message
                        )).expect("Failed to create videoscale element");
                        return ();
                    }
                };

                let participant_id = participant_session_meta.participant_id.clone();
                let old_queue_element = inner.queue_element_vp8_video_map.lock().unwrap().insert(
                    participant_id.clone(),
                    dynamic_queue_element.clone(),
                );
                let old_rtp_vp8_de_pay_element = inner.rtp_vp8_de_pay_element_video_map.lock().unwrap().insert(
                    participant_id.clone(),
                    dynamic_rtp_vp8_de_pay_element.clone(),
                );
                let old_vp8_dec_element = inner.vp8_dec_element_video_map.lock().unwrap().insert(
                    participant_id.clone(),
                    dynamic_vp8_dec_element.clone(),
                );
                let old_video_convert_element = inner.video_convert_element_video_map.lock().unwrap().insert(
                    participant_id.clone(),
                    dynamic_video_convert_element.clone(),
                );
                let old_video_scale_element = inner.video_scale_element_video_map.lock().unwrap().insert(
                    participant_id.clone(),
                    dynamic_video_scale_element.clone(),
                );
                let old_caps_filter_element = inner.caps_filter_element_video_map.lock().unwrap().insert(
                    participant_id.clone(),
                    dynamic_caps_filter.clone(),
                );
                if let Some(old_queue_element) = old_queue_element.clone() {
                    match old_queue_element.set_state(gstreamer::State::Null) {
                        Ok(_) => {
                            info!("Queue element set to null state successfully");
                        }
                        Err(err) => {
                            element_failure_counter.add(1, &vec![KeyValue::new("set_state", "queue_element_video")]);
                            error!("Failed to set queue element to null state: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to set queue element to null state",
                            )).unwrap();
                        }
                    }
                    match pipeline_unlock.remove(&old_queue_element) {
                        Ok(_) => {
                            info!("Queue element removed successfully");
                        }
                        Err(err) => {
                            pipeline_failure_counter.add(1, &vec![KeyValue::new("remove", "queue_element_video")]);
                            error!("Failed to remove queue element: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to remove queue element",
                            )).unwrap();
                        }
                    }
                    match old_rtp_vp8_de_pay_element.clone().unwrap().set_state(gstreamer::State::Null) {
                        Ok(_) => {
                            info!("Rtp Vp8 De Pay element set to null state successfully");
                        }
                        Err(err) => {
                            element_failure_counter.add(1, &vec![KeyValue::new("set_state", "rtp_vp8_de_pay_element_video")]);
                            error!("Failed to set rtp_vp8_de_pay element to null state: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to set rtp_vp8_de_pay element to null state",
                            )).unwrap();
                        }
                    }
                    match pipeline_unlock.remove(&old_rtp_vp8_de_pay_element.unwrap()) {
                        Ok(_) => {
                            info!("Rtp Vp8 De Pay element removed successfully");
                        }
                        Err(err) => {
                            pipeline_failure_counter.add(1, &vec![KeyValue::new("remove", "rtp_vp8_de_pay_element_video")]);
                            error!("Failed to remove rtp_vp8_de_pay element: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to remove rtp_vp8_de_pay element",
                            )).unwrap();
                        }
                    }
                    match old_vp8_dec_element.clone().unwrap().set_state(gstreamer::State::Null) {
                        Ok(_) => {
                            info!("Vp8 Dec element set to null state successfully");
                        }
                        Err(err) => {
                            element_failure_counter.add(1, &vec![KeyValue::new("set_state", "vp8_dec_element_video")]);
                            error!("Failed to set vp8_dec element to null state: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to set vp8_dec element to null state",
                            )).unwrap();
                        }
                    }
                    match pipeline_unlock.remove(&old_vp8_dec_element.unwrap()) {
                        Ok(_) => {
                            info!("Vp8 Dec element removed successfully");
                        }
                        Err(err) => {
                            pipeline_failure_counter.add(1, &vec![KeyValue::new("remove", "vp8_dec_element_video")]);
                            error!("Failed to remove vp8_dec element: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to remove vp8_dec element",
                            )).unwrap();
                        }
                    }
                    match old_video_convert_element.clone().unwrap().set_state(gstreamer::State::Null) {
                        Ok(_) => {
                            info!("Video Convert element set to null state successfully");
                        }
                        Err(err) => {
                            element_failure_counter.add(1, &vec![KeyValue::new("set_state", "video_convert_element_video")]);
                            error!("Failed to set video_convert element to null state: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to set video_convert element to null state",
                            )).unwrap();
                        }
                    }
                    match pipeline_unlock.remove(&old_video_convert_element.unwrap()) {
                        Ok(_) => {
                            info!("Video Convert element removed successfully");
                        }
                        Err(err) => {
                            pipeline_failure_counter.add(1, &vec![KeyValue::new("remove", "video_convert_element_video")]);
                            error!("Failed to remove video_convert element: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to remove video_convert element",
                            )).unwrap();
                        }
                    }
                    match old_video_scale_element.clone().unwrap().set_state(gstreamer::State::Null) {
                        Ok(_) => {
                            info!("Video Scale element set to null state successfully");
                        }
                        Err(err) => {
                            element_failure_counter.add(1, &vec![KeyValue::new("set_state", "video_scale_element_video")]);
                            error!("Failed to set video_scale element to null state: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to set video_scale element to null state",
                            )).unwrap();
                        }
                    }
                    match pipeline_unlock.remove(&old_video_scale_element.unwrap()) {
                        Ok(_) => {
                            info!("Video Scale element removed successfully");
                        }
                        Err(err) => {
                            pipeline_failure_counter.add(1, &vec![KeyValue::new("remove", "video_scale_element_video")]);
                            error!("Failed to remove video_scale element: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to remove video_scale element",
                            )).unwrap();
                        }
                    }
                    match old_caps_filter_element.clone().unwrap().set_state(gstreamer::State::Null) {
                        Ok(_) => {
                            info!("Caps Filter element set to null state successfully");
                        }
                        Err(err) => {
                            element_failure_counter.add(1, &vec![KeyValue::new("set_state", "caps_filter_element_video")]);
                            error!("Failed to set caps_filter element to null state: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to set caps_filter element to null state",
                            )).unwrap();
                        }
                    }
                    match pipeline_unlock.remove(&old_caps_filter_element.unwrap()) {
                        Ok(_) => {
                            info!("Caps Filter element removed successfully");
                        }
                        Err(err) => {
                            pipeline_failure_counter.add(1, &vec![KeyValue::new("remove", "caps_filter_element_video")]);
                            error!("Failed to remove caps_filter element: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to remove caps_filter element",
                            )).unwrap();
                        }
                    }
                }

                match pipeline_unlock.add(&dynamic_queue_element) {
                    Ok(_) => {
                        info!("Queue element added successfully");
                    }
                    Err(err) => {
                        pipeline_failure_counter.add(1, &vec![KeyValue::new("add", "queue_element_video")]);
                        error!("Failed to add queue element: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to add queue element",
                        )).unwrap();
                    }
                }
                match pipeline_unlock.add(&dynamic_rtp_vp8_de_pay_element) {
                    Ok(_) => {
                        info!("Rtp Vp8 De Pay element added successfully");
                    }
                    Err(err) => {
                        pipeline_failure_counter.add(1, &vec![KeyValue::new("add", "rtp_vp8_de_pay_element_video")]);
                        error!("Failed to add rtp_vp8_de_pay element: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to add rtp_vp8_de_pay element",
                        )).unwrap();
                    }
                }
                match pipeline_unlock.add(&dynamic_vp8_dec_element) {
                    Ok(_) => {
                        info!("Vp8 Dec element added successfully");
                    }
                    Err(err) => {
                        pipeline_failure_counter.add(1, &vec![KeyValue::new("add", "vp8_dec_element_video")]);
                        error!("Failed to add vp8_dec element: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to add vp8_dec element",
                        )).unwrap();
                    }
                }
                match pipeline_unlock.add(&dynamic_video_convert_element) {
                    Ok(_) => {
                        info!("Video Convert element added successfully");
                    }
                    Err(err) => {
                        pipeline_failure_counter.add(1, &vec![KeyValue::new("add", "video_convert_element_video")]);
                        error!("Failed to add video_convert element: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to add video_convert element",
                        )).unwrap();
                    }
                }
                match pipeline_unlock.add(&dynamic_video_scale_element) {
                    Ok(_) => {
                        info!("Video Scale element added successfully");
                    }
                    Err(err) => {
                        pipeline_failure_counter.add(1, &vec![KeyValue::new("add", "video_scale_element_video")]);
                        error!("Failed to add video_scale element: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to add video_scale element",
                        )).unwrap();
                    }
                }

                match pipeline_unlock.add(&dynamic_caps_filter) {
                    Ok(_) => {
                        info!("Caps Filter element added successfully");
                    }
                    Err(err) => {
                        pipeline_failure_counter.add(1, &vec![KeyValue::new("add", "caps_filter_element_video")]);
                        error!("Failed to add caps_filter element: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to add caps_filter element",
                        )).unwrap();
                    }
                }

                let queue_sink_pad = dynamic_queue_element.static_pad("sink")
                    .expect("queue should have a sink pad");
                info!("Queue Sink Pad {:#?}",queue_sink_pad.query_caps(None));
                match pad.link(&queue_sink_pad) {
                    Ok(_) => {
                        info!("Linked rtpbin and queue successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link rtpbin and queue: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to link rtpbin and queue",
                        )).unwrap();
                    }
                }
                match dynamic_queue_element.link(&dynamic_rtp_vp8_de_pay_element) {
                    Ok(_) => {
                        info!("Linked queue and rtpvp8depay successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link queue and rtpvp8depay: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to link queue and rtpvp8depay",
                        )).unwrap();
                    }
                }
                match dynamic_rtp_vp8_de_pay_element.link(&dynamic_vp8_dec_element) {
                    Ok(_) => {
                        info!("Linked rtpvp8depay and vp8dec successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link rtpvp8depay and vp8dec: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to link rtpvp8depay and vp8dec",
                        )).unwrap();
                    }
                }
                match dynamic_vp8_dec_element.link(&dynamic_video_convert_element) {
                    Ok(_) => {
                        info!("Linked vp8dec and videoconvert successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link vp8dec and videoconvert: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to link vp8dec and videoconvert",
                        )).unwrap();
                    }
                }
                match dynamic_video_convert_element.link(&dynamic_video_scale_element) {
                    Ok(_) => {
                        info!("Linked videoconvert and videoscale successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link videoconvert and videoscale: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to link videoconvert and videoscale",
                        )).unwrap();
                    }
                }
                match dynamic_video_scale_element.link(&dynamic_caps_filter) {
                    Ok(_) => {
                        info!("Linked videoscale and capsfilter successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link videoscale and capsfilter: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to link videoscale and capsfilter",
                        )).unwrap();
                    }
                }
                let caps_src_pad = dynamic_caps_filter.static_pad("src")
                    .expect("caps should have a src pad");
                let compositor_clone = inner.video_compositor_element.clone().unwrap();
                let compositor_video_sink_pad = compositor_clone.lock().unwrap().request_pad_simple("sink_%u").ok_or_else(|| {
                    element_failure_counter.add(1, &vec![KeyValue::new("request_pad", "compositor_vp8_video")]);
                    error!("Failed to request pad from compositor");
                    bus.post(gstreamer::message::Error::new(
                        gstreamer::CoreError::Failed,
                        "Failed to request pad from compositor",
                    )).unwrap();
                    "Failed to request pad from compositor".to_string()
                });
                // compositor_video_sink_pad.clone().unwrap().set_property_from_value("max-last-buffer-repeat", &Value::from(0u64));
                compositor_video_sink_pad.clone().unwrap().set_properties_from_value(
                    &[
                        ("xpos", Value::from(0i32)),
                        ("ypos", Value::from(0i32)),
                        ("width", Value::from(1280i32)),
                        ("height", Value::from(720i32)),
                    ]
                );
                let old_video_compositor_pad = inner.video_compositor_pad_map.lock().unwrap().insert(
                    participant_id.clone(),
                    compositor_video_sink_pad.clone().unwrap(),
                );
                if let Some(old_video_compositor_pad) = old_video_compositor_pad {
                    info!("Old video compositor pad found, releasing it");
                    compositor_clone.lock().unwrap().release_request_pad(&old_video_compositor_pad);
                }
                match caps_src_pad.link(&compositor_video_sink_pad.unwrap()) {
                    Ok(_) => {
                        info!("Linked capsfilter and compositor successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link videoconvert and compositor: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to link videoconvert and compositor",
                        )).unwrap();
                    }
                }

                match pipeline_unlock.set_state(gstreamer::State::Playing) {
                    Ok(_) => {
                        info!("Pipeline started successfully");
                    }
                    Err(err) => {
                        pipeline_failure_counter.add(1, &vec![]);
                        error!("Failed to start pipeline: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to start pipeline",
                        )).unwrap();
                    }
                }
            } else {
                info!("{:#?}",pad.query_caps(None));
                let dynamic_timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                let mut dynamic_queue_element = match ElementFactory::make("queue")
                    .name(format!("queue_audio_{}_{}", participant_session_meta.participant_id, dynamic_timestamp))
                    .property("flush-on-eos", true)
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "queue_audio")]);
                        error!("Failed to create queue element {:?}", e);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to create queue element",
                        )).unwrap();
                        return ();
                    }
                };
                let mut dynamic_rtp_opus_de_pay_element = match ElementFactory::make("rtpopusdepay")
                    .name(format!("rtpopusdepay_audio_{}_{}", participant_session_meta.participant_id, dynamic_timestamp))
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "rtpopusdepay_audio")]);
                        error!("Failed to create rtpopusdepay element {:?}", e);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to create rtpopusdepay element",
                        )).unwrap();
                        return ();
                    }
                };
                let mut dynamic_opus_dec_element = match ElementFactory::make("opusdec")
                    .name(format!("opusdec_audio_{}_{}", participant_session_meta.participant_id, dynamic_timestamp))
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "opusdec_audio")]);
                        error!("Failed to create opusdec element {:?}", e);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to create opusdec element",
                        )).unwrap();
                        return ();
                    }
                };
                let participant_id = participant_session_meta.participant_id.clone();
                info!("LOCK: Trying to lock for insert queue_element_audio_map");
                let old_queue_element = {
                    inner.queue_element_audio_map.lock().unwrap().insert(
                        participant_id.clone(),
                        dynamic_queue_element.clone())
                };
                info!("LOCK: Released lock for insert queue_element_audio_map");
                let old_rtp_opus_de_pay_element = {
                    inner.rtp_opus_de_pay_element_audio_map.lock().unwrap().insert(
                        participant_id.clone(),
                        dynamic_rtp_opus_de_pay_element.clone(),
                    )
                };
                let old_opus_dec_element = {
                    inner.opus_dec_element_audio_map.lock().unwrap().insert(
                        participant_id.clone(),
                        dynamic_opus_dec_element.clone(),
                    )
                };

                if let Some(old_queue_element) = old_queue_element.clone() {
                    info!("Dynamic elements modification");
                    match old_queue_element.set_state(gstreamer::State::Null) {
                        Ok(_) => {
                            info!("Queue element set to Null successfully");
                        }
                        Err(err) => {
                            pipeline_failure_counter.add(1, &vec![KeyValue::new("set_state", "queue_element_audio")]);
                            error!("Failed to set queue element to Null: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to set queue element to Null",
                            )).unwrap();
                        }
                    }
                    match pipeline_unlock.remove(&old_queue_element) {
                        Ok(_) => {
                            info!("Queue element removed successfully");
                        }
                        Err(err) => {
                            pipeline_failure_counter.add(1, &vec![KeyValue::new("remove", "queue_element_audio")]);
                            error!("Failed to remove queue element: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to remove queue element",
                            )).unwrap();
                        }
                    }
                    match old_rtp_opus_de_pay_element.clone().unwrap().set_state(gstreamer::State::Null) {
                        Ok(_) => {
                            info!("Rtp Opus De Pay element set to Null successfully");
                        }
                        Err(err) => {
                            pipeline_failure_counter.add(1, &vec![KeyValue::new("set_state", "rtp_opus_de_pay_element_audio")]);
                            error!("Failed to set rtp_opus_de_pay element to Null: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to set rtp_opus_de_pay element to Null",
                            )).unwrap();
                        }
                    }
                    match pipeline_unlock.remove(&old_rtp_opus_de_pay_element.unwrap()) {
                        Ok(_) => {
                            info!("Rtp Opus De Pay element removed successfully");
                        }
                        Err(err) => {
                            pipeline_failure_counter.add(1, &vec![KeyValue::new("remove", "rtp_opus_de_pay_element_audio")]);
                            error!("Failed to remove rtp_opus_de_pay element: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to remove rtp_opus_de_pay element",
                            )).unwrap();
                        }
                    }
                    match old_opus_dec_element.clone().unwrap().set_state(gstreamer::State::Null) {
                        Ok(_) => {
                            info!("Opus Dec element set to Null successfully");
                        }
                        Err(err) => {
                            pipeline_failure_counter.add(1, &vec![KeyValue::new("set_state", "opus_dec_element_audio")]);
                            error!("Failed to set opus_dec element to Null: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to set opus_dec element to Null",
                            )).unwrap();
                        }
                    }
                    match pipeline_unlock.remove(&old_opus_dec_element.unwrap()) {
                        Ok(_) => {
                            info!("Opus Dec element removed successfully");
                        }
                        Err(err) => {
                            pipeline_failure_counter.add(1, &vec![KeyValue::new("remove", "opus_dec_element_audio")]);
                            error!("Failed to remove opus_dec element: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to remove opus_dec element",
                            )).unwrap();
                        }
                    }
                }
                match pipeline_unlock.add(&dynamic_queue_element) {
                    Ok(_) => {
                        info!("Queue element added successfully");
                    }
                    Err(err) => {
                        pipeline_failure_counter.add(1, &vec![KeyValue::new("add", "queue_element_audio")]);
                        error!("Failed to add queue element: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to add queue element",
                        )).unwrap();
                    }
                }
                match pipeline_unlock.add(&dynamic_rtp_opus_de_pay_element) {
                    Ok(_) => {
                        info!("Rtp Opus De Pay element added successfully");
                    }
                    Err(err) => {
                        pipeline_failure_counter.add(1, &vec![KeyValue::new("add", "rtp_opus_de_pay_element_audio")]);
                        error!("Failed to add rtp_opus_de_pay element: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to add rtp_opus_de_pay element",
                        )).unwrap();
                    }
                }
                match pipeline_unlock.add(&dynamic_opus_dec_element) {
                    Ok(_) => {
                        info!("Opus Dec element added successfully");
                    }
                    Err(err) => {
                        pipeline_failure_counter.add(1, &vec![KeyValue::new("add", "opus_dec_element_audio")]);
                        error!("Failed to add opus_dec element: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to add opus_dec element",
                        )).unwrap();
                    }
                }
                let queue_sink_pad = dynamic_queue_element.static_pad("sink").unwrap();
                info!("Queue Sink Pad {:#?}",queue_sink_pad.query_caps(None));
                match pad.link(&queue_sink_pad) {
                    Ok(_) => {
                        info!("Linked rtpbin and queue successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link rtpbin and queue: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to link rtpbin and queue",
                        )).unwrap();
                    }
                }
                match dynamic_queue_element.link(&dynamic_rtp_opus_de_pay_element) {
                    Ok(_) => {
                        info!("Linked queue and rtpopusdepay successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link queue and rtpopusdepay: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to link queue and rtpopusdepay",
                        )).unwrap();
                    }
                }
                match dynamic_rtp_opus_de_pay_element.link(&dynamic_opus_dec_element) {
                    Ok(_) => {
                        info!("Linked rtpopusdepay and opusdec successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link rtpopusdepay and opusdec: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to link rtpopusdepay and opusdec",
                        )).unwrap();
                    }
                }
                let opus_dec_src_pad = match dynamic_opus_dec_element.static_pad("src") {
                    Some(pad) => pad,
                    None => {
                        pad_request_failure_counter.add(1, &[]);
                        error!("Failed to get src pad from opusenc");
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to get src pad from opusenc",
                        )).unwrap();
                        return ();
                    }
                };
                let audio_mixer_element_clone = inner.audio_mixer_element.clone().unwrap();
                let audio_mixer_sink_pad = match audio_mixer_element_clone.lock().unwrap().request_pad_simple("sink_%u") {
                    Some(pad) => pad,
                    None => {
                        pad_request_failure_counter.add(1, &[]);
                        error!("Failed to get sink pad from audiomixer");
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to get sink pad from audiomixer",
                        )).unwrap();
                        return ();
                    }
                };
                let old_audio_mixer_sink_pad = {
                    inner.audio_mixed_pad_map.lock().unwrap().insert(
                        participant_id.clone(),
                        audio_mixer_sink_pad.clone(),
                    )
                };
                if let Some(old_audio_mixer_sink_pad) = old_audio_mixer_sink_pad {
                    info!("Old audio mixer sink pad found, releasing it");
                    audio_mixer_element_clone.lock().unwrap().release_request_pad(&old_audio_mixer_sink_pad);
                }
                match opus_dec_src_pad.link(&audio_mixer_sink_pad) {
                    Ok(_) => {
                        info!("Linked opus dec src pad to audiomixer sink pad successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link opus dec src pad to audiomixer sink pad: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to link opus dec src pad to audiomixer sink pad",
                        )).unwrap();
                        return ();
                    }
                };

                match pipeline_unlock.set_state(gstreamer::State::Playing) {
                    Ok(_) => {
                        info!("Pipeline started successfully");
                    }
                    Err(err) => {
                        pipeline_failure_counter.add(1, &vec![]);
                        error!("Failed to start pipeline: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to start pipeline",
                        )).unwrap();
                    }
                }
            }
        } else if pad.name().starts_with("recv_rtcp_src_") {
            let self_clone = self_clone.lock().unwrap();
            let pipeline_unlock = self_clone.pipeline.as_ref().unwrap().lock().unwrap();
            let bus = pipeline_unlock.bus().unwrap();
            let pad_name = pad.name();
            let parts: Vec<&str> = pad_name.split('_').collect();
            let session_id = if parts.len() > 3 {
                parts[3].parse::<u32>().ok()
            } else {
                None
            };
            let ssrc = if parts.len() > 4 {
                parts[4].parse::<u32>().ok()
            } else {
                None
            };

            match pipeline_unlock.set_state(gstreamer::State::Paused) {
                Ok(_) => {
                    info!("Pipeline paused successfully");
                }
                Err(err) => {
                    pipeline_failure_counter.add(1, &vec![]);
                    error!("Failed to pause pipeline: {}", err);
                    bus.post(gstreamer::message::Error::new(
                        gstreamer::CoreError::Failed,
                        "Failed to pause pipeline",
                    )).unwrap();
                }
            };
            let inner_clone = self_clone.inner.clone();
            let mut inner = inner_clone.lock().unwrap();

            let mut map = inner.session_id_ssrc_map.lock().unwrap();
            map.entry(session_id.unwrap()).or_insert_with(Vec::new).push(ssrc.unwrap());

            let session_participant_meta_map = inner.session_participant_meta_map.lock().unwrap();
            let participant_session_meta = session_participant_meta_map.get(&session_id.unwrap()).unwrap().clone();
            if participant_session_meta.media == "video" {
                info!("{:#?}",pad.query_caps(None));
                info!("New pad added: {}", pad.name());
                // Get the sink pad of udp sink
                let mut dynamic_timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                let mut dynamic_udp_rtcp_sink_element = match gstreamer::ElementFactory::make("udpsink")
                    .name(format!("udpsink_rtcp_vp8_video_{}_{}", participant_session_meta.participant_id, dynamic_timestamp)).build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "udpsink_rtcp_vp8_video")]);
                        error!("Failed to create udpsink element {:?}", e);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to create udpsink element",
                        )).unwrap();
                        return ();
                    }
                };
                dynamic_udp_rtcp_sink_element.set_property("host", &"127.0.0.1");
                dynamic_udp_rtcp_sink_element.set_property("port", &participant_session_meta.port + 1);
                dynamic_udp_rtcp_sink_element.set_property("async", &false);
                dynamic_udp_rtcp_sink_element.set_property("sync", &false);
                let udp_rtcp_sink_sink_pad = dynamic_udp_rtcp_sink_element.static_pad("sink")
                    .expect("udpsink should have a sink pad");
                info!("Udp Sink Pad {:#?}",udp_rtcp_sink_sink_pad.query_caps(None));
                let participant_id = participant_session_meta.participant_id.clone();
                let old_udp_rtcp_sink_element = inner.udp_rtcp_sink_element_video_map.lock().unwrap().insert(
                    participant_id.clone(),
                    dynamic_udp_rtcp_sink_element.clone(),
                );
                if let Some(old_udp_rtcp_sink_element) = old_udp_rtcp_sink_element.clone() {
                    match old_udp_rtcp_sink_element.set_state(Null) {
                        Ok(_) => {
                            info!("Udp Sink element set to null state successfully");
                        }
                        Err(err) => {
                            element_failure_counter.add(1, &vec![KeyValue::new("set_state", "udpsink_rtcp_vp8_video")]);
                            error!("Failed to set udpsink to null state: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to set udpsink to null state",
                            )).unwrap();
                        }
                    }
                    match pipeline_unlock.remove(&old_udp_rtcp_sink_element) {
                        Ok(_) => {
                            info!("Udp Sink element removed successfully");
                        }
                        Err(err) => {
                            pipeline_failure_counter.add(1, &vec![KeyValue::new("remove", "udpsink_rtcp_vp8_video")]);
                            error!("Failed to remove udpsink: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to remove udpsink",
                            )).unwrap();
                        }
                    }
                }
                match pipeline_unlock.add(&dynamic_udp_rtcp_sink_element) {
                    Ok(_) => {
                        info!("Udp Sink element added successfully");
                    }
                    Err(err) => {
                        pipeline_failure_counter.add(1, &vec![KeyValue::new("add", "udpsink_rtcp_vp8_video")]);
                        error!("Failed to add udpsink: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to add udpsink",
                        )).unwrap();
                    }
                }
                match pad.link(&udp_rtcp_sink_sink_pad) {
                    Ok(_) => {
                        info!("Linked rtpbin and udpsink successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link rtpbin and udpsink: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to link rtpbin and udpsink",
                        )).unwrap();
                    }
                }
                let udp_rtcp_sink_sink_pad = dynamic_udp_rtcp_sink_element.static_pad("sink")
                    .expect("udpsink should have a sink pad");
                info!("Udp Sink Pad {:#?}",udp_rtcp_sink_sink_pad.query_caps(None));
                match pipeline_unlock.set_state(gstreamer::State::Playing) {
                    Ok(_) => {
                        info!("Pipeline started successfully");
                    }
                    Err(err) => {
                        pipeline_failure_counter.add(1, &vec![]);
                        error!("Failed to start pipeline: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to start pipeline",
                        )).unwrap();
                    }
                }
            } else {
                info!("{:#?}",pad.query_caps(None));
                let dynamic_timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                let dynamic_udp_rtcp_sink_element = match ElementFactory::make("udpsink")
                    .name(format!("udpsink_rtcp_audio_{}", dynamic_timestamp)).build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "udpsink_rtcp_audio")]);
                        error!("Failed to create udpsink element {:?}", e);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to create udpsink element",
                        )).unwrap();
                        return ();
                    }
                };
                dynamic_udp_rtcp_sink_element.set_property("host", &"127.0.0.1");
                dynamic_udp_rtcp_sink_element.set_property("port", &participant_session_meta.port + 1);
                dynamic_udp_rtcp_sink_element.set_property("async", &false);
                dynamic_udp_rtcp_sink_element.set_property("sync", &false);
                let participant_id = participant_session_meta.participant_id.clone();
                let old_udp_rtcp_sink_element = inner.udp_rtcp_sink_sink_element_audio_map.lock().unwrap().insert(
                    participant_id.clone(),
                    dynamic_udp_rtcp_sink_element.clone(),
                );
                if let Some(old_udp_rtcp_sink_element) = old_udp_rtcp_sink_element.clone() {
                    match old_udp_rtcp_sink_element.set_state(gstreamer::State::Null) {
                        Ok(_) => {
                            info!("Udp Sink element set to null state successfully");
                        }
                        Err(err) => {
                            element_failure_counter.add(1, &vec![KeyValue::new("set_state", "udpsink_rtcp_audio")]);
                            error!("Failed to set udpsink element to null state: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to set udpsink element to null state",
                            )).unwrap();
                        }
                    }
                    match pipeline_unlock.remove(&old_udp_rtcp_sink_element) {
                        Ok(_) => {
                            info!("Udp Sink element removed successfully");
                        }
                        Err(err) => {
                            pipeline_failure_counter.add(1, &vec![KeyValue::new("remove", "udpsink_rtcp_audio")]);
                            error!("Failed to remove udpsink element: {}", err);
                            bus.post(gstreamer::message::Error::new(
                                gstreamer::CoreError::Failed,
                                "Failed to remove udpsink element",
                            )).unwrap();
                        }
                    }
                }
                pipeline_unlock.add(&dynamic_udp_rtcp_sink_element).expect("Failed to add udpsink to pipeline_unlock");
                let udp_rtcp_sink_sink_pad = dynamic_udp_rtcp_sink_element.static_pad("sink").unwrap();
                info!("Udp Sink Pad {:#?}",udp_rtcp_sink_sink_pad.query_caps(None));
                match pad.link(&udp_rtcp_sink_sink_pad) {
                    Ok(_) => {
                        info!("Linked rtpbin and udpsink successfully");
                    }
                    Err(err) => {
                        linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                        error!("Failed to link rtpbin and udpsink: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to link rtpbin and udpsink",
                        )).unwrap();
                    }
                }
                match pipeline_unlock.set_state(gstreamer::State::Playing) {
                    Ok(_) => {
                        info!("Pipeline started successfully");
                    }
                    Err(err) => {
                        pipeline_failure_counter.add(1, &vec![]);
                        error!("Failed to start pipeline: {}", err);
                        bus.post(gstreamer::message::Error::new(
                            gstreamer::CoreError::Failed,
                            "Failed to start pipeline",
                        )).unwrap();
                    }
                }
            }
        }
    }


    pub async fn start_vp8_video_recording_gstreamer(mut self,
                                                     video_port: i32,
                                                     meeting_id: String,
                                                     participant_id: String,
                                                     transport_id: String,
                                                     clock: Clock,
                                                     meter: Meter, pipeline_failure_counter: Counter<u64>) -> Result<Self, String> {
        /*let pipeline_internal = gstreamer::Pipeline::with_name("Vp8 Video Pipeline");
        pipeline_internal.use_clock(Some(&clock));
        let pipeline = Arc::new(Mutex::new(pipeline_internal));*/
        let mut temp_pipeline = self.pipeline.clone();
        let pipeline = temp_pipeline.get_or_insert_with(|| {
            info!("Creating new pipeline");
            let pipeline_internal = gstreamer::Pipeline::with_name("Pipeline");
            pipeline_internal.use_clock(Some(&clock));
            Arc::new(Mutex::new(pipeline_internal))
        });
        self.meeting_id = Some(meeting_id.clone());
        self.participant_id = Some(participant_id.clone());
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let element_failure_counter: Counter<u64> = meter
            .u64_counter("metric_".to_string() + meeting_id.clone().to_string().as_str() + "_element_failures")
            .with_description("Counts the number of element creation failures")
            .init();
        let linking_failure_counter: Counter<u64> = meter
            .u64_counter("metric_".to_string() + meeting_id.clone().to_string().as_str() + "_linking_failures")
            .with_description("Counts the number of element linking failures")
            .init();
        let pad_request_failure_counter: Counter<u64> = meter
            .u64_counter("metric_".to_string() + meeting_id.clone().to_string().as_str() + "_pad_request_failures")
            .with_description("Counts the number of pad request failures")
            .init();
        /* ------------------- Creation of the elements  ------------------- */
        let video_caps = gstreamer::Caps::builder("application/x-rtp")
            .field("media", "video")
            .field("clock-rate", 90000)
            .field("encoding-name", "VP8")
            .field("payload", 96)
            .build();
        let udp_src_rtp_element = match gstreamer::ElementFactory::make("udpsrc")
            .name(format!("udpsrc_vp8_video_{}_{}", participant_id, timestamp)).build() {
            Ok(element) => element,
            Err(e) => {
                element_failure_counter.add(1, &vec![KeyValue::new("add", "udpsrc_vp8_video")]);
                error!("Failed to create udpsrc element {:?}", e);
                return Err("Failed to create udpsrc element".to_string());
            }
        };
        udp_src_rtp_element.set_property("address", &"127.0.0.1");
        udp_src_rtp_element.set_property("port", &video_port);
        udp_src_rtp_element.set_property("caps", &video_caps);
        udp_src_rtp_element.set_property("reuse", &false);
        // self.inner.lock().unwrap().udp_src_element_video = Some(Arc::new(Mutex::new(udp_src_rtp_element.clone())));
        self.inner.lock().unwrap().udp_src_element_video_map.lock().unwrap().insert(
            participant_id.clone(),
            udp_src_rtp_element.clone(),
        );
        let (compositor_video_element_element, compositor_video_element_element_new) = {
            if self.inner.lock().unwrap().video_compositor_element.is_some() {
                info!("Compositor element already exists, will not create a new one");
                let compositor_video = self.inner.lock().unwrap().video_compositor_element.clone().unwrap();
                let x = (compositor_video.lock().unwrap().clone(), false);
                x
            } else {
                let new_compositor_video_element = match ElementFactory::make("compositor")
                    .name(format!("compositor_vp8_video_{}", timestamp))
                    .property_from_str("force-live", "true")
                    .property_from_str("background", "black")
                    .property_from_str("ignore-inactive-pads", "true")
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "compositor_vp8_video")]);
                        error!("Failed to create compositor element {:?}", e);
                        return Err("Failed to create compositor element".to_string());
                    }
                };
                let compositor_video_element_temp = Arc::new(Mutex::new(new_compositor_video_element.clone()));
                self.inner.lock().unwrap().video_compositor_element = Some(compositor_video_element_temp);
                (new_compositor_video_element, true)
            }
        };
        let (video_scale_post_compositor_element, video_scale_post_compositor_element_new) = {
            if self.inner.lock().unwrap().video_scale_post_compositor_element.is_some() {
                info!("Video scale element already exists, will not create a new one");
                let video_scale = self.inner.lock().unwrap().video_scale_post_compositor_element.clone().unwrap();
                let x = (video_scale.lock().unwrap().clone(), false);
                x
            } else {
                let new_video_scale_post_compositor_element = match ElementFactory::make("videoscale")
                    .name(format!("videoscale_vp8_post_compositor_video_{}", timestamp))
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "videoscale_vp8_post_compositor_video_")]);
                        error!("Failed to create videoscale element {:?}", e);
                        return Err("Failed to create videoscale element".to_string());
                    }
                };
                let video_scale_post_compositor_element_temp = Arc::new(Mutex::new(new_video_scale_post_compositor_element.clone()));
                self.inner.lock().unwrap().video_scale_post_compositor_element = Some(video_scale_post_compositor_element_temp);
                (new_video_scale_post_compositor_element, true)
            }
        };
        let (caps_filter_post_compositor_element, caps_filter_post_compositor_element_new) = {
            if self.inner.lock().unwrap().caps_filter_post_compositor_element.is_some() {
                info!("Caps filter element already exists, will not create a new one");
                let caps_filter_video = self.inner.lock().unwrap().caps_filter_post_compositor_element.clone().unwrap();
                let x = (caps_filter_video.lock().unwrap().clone(), false);
                x
            } else {
                let new_caps_filter_video_element = match ElementFactory::make("capsfilter")
                    .name(format!("caps_filter_vp8_post_compositor_video_{}", timestamp))
                    .property("caps", &gstreamer::Caps::builder("video/x-raw")
                        .field("width", 1280)
                        .field("height", 720)
                        .build())
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "caps_filter_vp8_post_compositor_video_")]);
                        error!("Failed to create capsfilter element {:?}", e);
                        return Err("Failed to create capsfilter element".to_string());
                    }
                };
                let caps_filter_video_element_temp = Arc::new(Mutex::new(new_caps_filter_video_element.clone()));
                self.inner.lock().unwrap().caps_filter_post_compositor_element = Some(caps_filter_video_element_temp);
                (new_caps_filter_video_element, true)
            }
        };
        let (x264_enc_element, x264_enc_element_new) = {
            if self.inner.lock().unwrap().x264_encode_element_video.is_some() {
                info!("X264Enc element already exists, will not create a new one");
                let x264_enc = self.inner.lock().unwrap().x264_encode_element_video.clone().unwrap();
                let x = (x264_enc.lock().unwrap().clone(), false);
                x
            } else {
                let new_x264_enc_element = match ElementFactory::make("x264enc")
                    .name(format!("x264enc_vp8_video_{}", timestamp))
                    .property_from_str("speed-preset", "ultrafast")
                    .property_from_str("tune", "zerolatency")
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "x264enc_vp8_video")]);
                        error!("Failed to create x264enc element {:?}", e);
                        return Err("Failed to create x264enc element".to_string());
                    }
                };
                let x264_enc_element_temp = Arc::new(Mutex::new(new_x264_enc_element.clone()));
                self.inner.lock().unwrap().x264_encode_element_video = Some(x264_enc_element_temp);
                (new_x264_enc_element, true)
            }
        };
        let (split_mux_sink_element, split_mux_sink_element_new) = {
            if self.inner.lock().unwrap().split_mux_sink_element.is_some() {
                info!("SplitMuxSink element already exists, will not create a new one");
                let split_mux_sink = self.inner.lock().unwrap().split_mux_sink_element.clone().unwrap();
                let x = (split_mux_sink.lock().unwrap().clone(), false);
                x
            } else {
                let muxer = match ElementFactory::make("mpegtsmux")
                    .name(format!("mpegtsmux_{}", timestamp))
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "mpegtsmux")]);
                        error!("Failed to create mpegtsmux element {:?}", e);
                        return Err("Failed to create mpegtsmux element".to_string());
                    }
                };
                let new_split_mux_sink_element = match ElementFactory::make("splitmuxsink")
                    .name(format!("splitmuxsink_{}", timestamp))
                    .property("max-size-time", ClockTime::from_seconds(30).nseconds())
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "splitmuxsink")]);
                        error!("Failed to create splitmuxsink element {:?}", e);
                        return Err("Failed to create splitmuxsink element".to_string());
                    }
                };
                new_split_mux_sink_element.set_property("muxer", &muxer);
                let meeting_id_clone = Arc::new(meeting_id.clone());
                let participant_id_clone = Arc::new(participant_id.clone());
                let plain_transport_id_clone = Arc::new(transport_id.clone());
                new_split_mux_sink_element.connect("format-location", false, move |values| {
                    // Generate the custom filename with a timestamp
                    let timestamp_nanos = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("Time went backwards")
                        .as_nanos();
                    let filename = format!("/opt/recordings/{}/chunk_{}.ts", meeting_id_clone, timestamp_nanos);
                    // Return the filename
                    Some(glib::Value::from(&filename))
                });
                let split_mux_sink_element_temp = Arc::new(Mutex::new(new_split_mux_sink_element.clone()));
                self.inner.lock().unwrap().split_mux_sink_element = Some(split_mux_sink_element_temp);
                (new_split_mux_sink_element, true)
            }
        };

        let udp_rtcp_src_element = match ElementFactory::make("udpsrc")
            .name(format!("udpsrc_rtcp_vp8_video_{}_{}", participant_id, timestamp)).build() {
            Ok(element) => element,
            Err(e) => {
                element_failure_counter.add(1, &vec![KeyValue::new("add", "udpsrc_rtcp_vp8_video")]);
                error!("Failed to create udpsrc element {:?}", e);
                return Err("Failed to create udpsrc element".to_string());
            }
        };
        udp_rtcp_src_element.set_property("address", &"127.0.0.1");
        udp_rtcp_src_element.set_property("port", &video_port + 1);
        udp_rtcp_src_element.set_property("reuse", &false);
        self.inner.lock().unwrap().udp_rtcp_src_element_video_map.lock().unwrap().insert(
            participant_id.clone(),
            udp_rtcp_src_element.clone(),
        );
        let (rtp_bin_element, rtp_bin_element_new) = {
            if self.inner.lock().unwrap().rtp_bin_element.is_some() {
                info!("RtpBin element already exists, will not create a new one");
                let rtp_bin = self.inner.lock().unwrap().rtp_bin_element.clone().unwrap();
                let x = (rtp_bin.lock().unwrap().clone(), false);
                x
            } else {
                let new_rtp_bin_element = match ElementFactory::make("rtpbin")
                    .name(format!("rtpbin_vp8_video_{}", timestamp))
                    .build() {
                    Ok(element) => element,
                    Err(e) => {
                        element_failure_counter.add(1, &vec![KeyValue::new("add", "rtpbin_vp8_video")]);
                        error!("Failed to create rtpbin element {:?}", e);
                        return Err("Failed to create rtpbin element".to_string());
                    }
                };
                new_rtp_bin_element.set_property_from_value("latency", &Value::from(1000u32));
                let rtp_bin_element_temp = Arc::new(Mutex::new(new_rtp_bin_element.clone()));
                self.inner.lock().unwrap().rtp_bin_element = Some(rtp_bin_element_temp);
                (new_rtp_bin_element, true)
            }
        };
        /* ------------------- Add elements to the pipeline ------------------- */
        match pipeline.lock().unwrap().set_state(Paused) {
            Ok(_) => {
                info!("Pipeline paused successfully");
            }
            Err(err) => {
                pipeline_failure_counter.add(1, &vec![]);
                error!("Failed to pause pipeline: {}", err);
                return Err("Failed to pause pipeline".to_string());
            }
        }
        match pipeline.lock().unwrap().add_many(&[
            &udp_src_rtp_element,
            &udp_rtcp_src_element
        ]) {
            Ok(_) => {
                info!("Elements added to the pipeline successfully");
            }
            Err(err) => {
                pipeline_failure_counter.add(1, &vec![]);
                error!("Failed to add elements to the pipeline: {}", err);
                return Err("Failed to add elements to the pipeline".to_string());
            }
        }
        if rtp_bin_element_new {
            match pipeline.lock().unwrap().add(&rtp_bin_element) {
                Ok(_) => {
                    info!("RtpBin element added to the pipeline successfully");
                }
                Err(err) => {
                    pipeline_failure_counter.add(1, &vec![KeyValue::new("add", "rtpbin_vp8_video")]);
                    error!("Failed to add rtpbin element to the pipeline: {}", err);
                    return Err("Failed to add rtpbin element to the pipeline".to_string());
                }
            }
        }
        if compositor_video_element_element_new && x264_enc_element_new && video_scale_post_compositor_element_new && caps_filter_post_compositor_element_new {
            match pipeline.lock().unwrap().add_many(&[&compositor_video_element_element,
                &x264_enc_element, &video_scale_post_compositor_element, &caps_filter_post_compositor_element]) {
                Ok(_) => {
                    info!("Compositor and x264enc elements added to the pipeline successfully");
                }
                Err(err) => {
                    pipeline_failure_counter.add(1, &vec![KeyValue::new("add", "compositor_x264enc")]);
                    error!("Failed to add compositor and x264enc elements to the pipeline: {}", err);
                    return Err("Failed to add compositor and x264enc elements to the pipeline".to_string());
                }
            }
        } else if compositor_video_element_element_new || x264_enc_element_new || video_scale_post_compositor_element_new || caps_filter_post_compositor_element_new {
            error!("Either compositor, x264enc, videoscale or capsfilter element is new, but not both");
            return Err("Either compositor, x264enc, videoscale or capsfilter element is new, but not both".to_string());
        }
        if split_mux_sink_element_new {
            match pipeline.lock().unwrap().add(&split_mux_sink_element) {
                Ok(_) => {
                    info!("SplitMuxSink element added to the pipeline successfully");
                }
                Err(err) => {
                    pipeline_failure_counter.add(1, &vec![KeyValue::new("add", "splitmuxsink")]);
                    error!("Failed to add splitmuxsink element to the pipeline: {}", err);
                    return Err("Failed to add splitmuxsink element to the pipeline".to_string());
                }
            }
        }
        /*---------------------------------------------------------------------*/

        /* ------------------- Start Linking of pads / elements ------------------- */
        let udp_src_rtp_src_pad = match udp_src_rtp_element.static_pad("src") {
            Some(pad) => pad,
            None => {
                error!("Failed to get src pad from udpsrc");
                return Err("Failed to get src pad from udpsrc".to_string());
            }
        };
        let rtp_bin_sink_pad = match rtp_bin_element.request_pad_simple("recv_rtp_sink_%u") {
            Some(pad) => pad,
            None => {
                error!("Failed to get recv_rtp_sink pad from rtpbin");
                return Err("Failed to get recv_rtp_sink pad from rtpbin".to_string());
            }
        };
        match udp_src_rtp_src_pad.link(&rtp_bin_sink_pad) {
            Ok(_) => {
                info!("Linked udpsrc and rtpbin successfully");
            }
            Err(err) => {
                linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                error!("Failed to link udpsrc and rtpbin: {}", err);
                return Err("Failed to link udpsrc and rtpbin".to_string());
            }
        }
        let udp_rtcp_src_pad = udp_rtcp_src_element.static_pad("src")
            .expect("Failed to get src pad from udpsrc");
        let rtp_bin_rtcp_sink_pad = rtp_bin_element.request_pad_simple("recv_rtcp_sink_%u").unwrap();
        match udp_rtcp_src_pad.link(&rtp_bin_rtcp_sink_pad) {
            Ok(_) => {
                info!("Linked udpsrc_rtcp and rtpbin successfully");
            }
            Err(err) => {
                linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                error!("Failed to link udpsrc_rtcp and rtpbin: {}", err);
                return Err("Failed to link udpsrc_rtcp and rtpbin".to_string());
            }
        }
        let sink_pad_name = rtp_bin_sink_pad.name();
        let sink_pad_session: u32 = sink_pad_name
            .strip_prefix("recv_rtp_sink_")
            .unwrap()
            .parse()
            .unwrap();
        self.inner.lock().unwrap().session_participant_meta_map.lock().unwrap().insert(
            sink_pad_session,
            ParticipantSessionMeta {
                meeting_id: meeting_id.clone(),
                participant_id: participant_id.clone(),
                media: "video".to_string(),
                port: video_port,
                rtp_sink_pad: rtp_bin_sink_pad.clone(),
                rtcp_sink_pad: rtp_bin_rtcp_sink_pad.clone(),
            },
        );
        if compositor_video_element_element_new {
            let compositor_video_src_pad = match compositor_video_element_element.static_pad("src") {
                Some(pad) => pad,
                None => {
                    error!("Failed to get src pad from compositor element");
                    return Err("Failed to get src pad from compositor element".to_string());
                }
            };
            let video_scale_post_compositor_sink_pad = match video_scale_post_compositor_element.static_pad("sink") {
                Some(pad) => pad,
                None => {
                    error!("Failed to get sink pad from videoscale element");
                    return Err("Failed to get sink pad from videoscale element".to_string());
                }
            };
            let x264_enc_src_pad = match x264_enc_element.static_pad("src") {
                Some(pad) => pad,
                None => {
                    error!("Failed to get src pad from x264enc element");
                    return Err("Failed to get src pad from x264enc element".to_string());
                }
            };
            let split_mux_sink_video_pad_local = self.inner.lock().unwrap().split_mux_sink_video_pad.clone();
            let split_mux_sink_video_pad = if let Some(split_mux_sink_video_pad) = split_mux_sink_video_pad_local {
                info!("Using existing splitmuxsink video pad");
                split_mux_sink_video_pad.lock().unwrap().clone()
            } else {
                info!("Creating new splitmuxsink video pad");
                let split_mux_sink_video_pad = match split_mux_sink_element.request_pad_simple("video") {
                    Some(pad) => pad,
                    None => {
                        error!("Failed to get video pad from splitmuxsink");
                        return Err("Failed to get video pad from splitmuxsink".to_string());
                    }
                };
                self.inner.lock().unwrap().split_mux_sink_video_pad = Some(Arc::new(Mutex::new(split_mux_sink_video_pad.clone())));
                split_mux_sink_video_pad
            };
            match compositor_video_src_pad.link(&video_scale_post_compositor_sink_pad) {
                Ok(_) => {
                    info!("Linked compositor and x264enc successfully");
                }
                Err(err) => {
                    linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                    error!("Failed to link compositor and x264enc: {}", err);
                    return Err("Failed to link compositor and x264enc".to_string());
                }
            }
            match video_scale_post_compositor_element.link(&caps_filter_post_compositor_element) {
                Ok(_) => {
                    info!("Linked videoscale and capsfilter successfully");
                }
                Err(err) => {
                    linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                    error!("Failed to link videoscale and capsfilter: {}", err);
                    return Err("Failed to link videoscale and capsfilter".to_string());
                }
            }
            match caps_filter_post_compositor_element.link(&x264_enc_element) {
                Ok(_) => {
                    info!("Linked capsfilter and x264enc successfully");
                }
                Err(err) => {
                    linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                    error!("Failed to link capsfilter and x264enc: {}", err);
                    return Err("Failed to link capsfilter and x264enc".to_string());
                }
            }
            match x264_enc_src_pad.link(&split_mux_sink_video_pad) {
                Ok(_) => {
                    info!("Linked x264enc and splitmuxsink successfully");
                }
                Err(err) => {
                    linking_failure_counter.add(1, &vec![KeyValue::new("error", err.to_string())]);
                    error!("Failed to link x264enc and splitmuxsink: {}", err);
                    return Err("Failed to link x264enc and splitmuxsink".to_string());
                }
            }
        }
        // let queue_element_arc = Arc::new(Mutex::new(queue_element.clone()));
        // let queue_element_clone = Arc::clone(&queue_element_arc);
        self.pipeline = Some(pipeline.clone());
        let self_arc = Arc::new(Mutex::new(self.clone()));
        let self_clone = Arc::clone(&self_arc);

        if rtp_bin_element_new {
            let handler_id = rtp_bin_element.connect_pad_added(move |rtpbin, pad| {
                Self::rtp_bin_element_handling(&self_clone, &element_failure_counter,
                                               &pipeline_failure_counter, &linking_failure_counter, &pad_request_failure_counter, rtpbin, pad);
            });
            self.handler_id = Some(Arc::new(Mutex::new(handler_id)));
        }

        /*---------------------------------------------------------------------*/

        /* ------------------- Debugging help like adding of probes ... ------------------- */

        /*---------------------------------------------------------------------*/
        //self.pipeline = Some(pipeline.clone());
        Ok(self)
    }

    /// Start an H.264 video recording pipeline for a participant and return the
    /// created `Pipeline` on success.
    ///
    /// This helper creates a dedicated pipeline configured for H.264 RTP,
    /// links elements synchronously and returns the pipeline for the caller
    /// to manage. It is designed for simpler one-off recording use-cases.
    pub async fn start_h264_video_recording_gstreamer(&mut self,
                                                      video_port: i32,
                                                      meeting_id: String,
                                                      participant_id: String,
                                                      transport_id: String,
                                                      clock: Clock) -> Result<Pipeline, String> {
        // gstreamer::init().expect("Failed to initialize GStreamer");
        let pipeline = gstreamer::Pipeline::with_name("H264 Video Pipeline");
        pipeline.use_clock(Some(&clock));
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        /* ------------------- Creation of the elements  ------------------- */
        let video_caps = gstreamer::Caps::builder("application/x-rtp")
            .field("media", "video")
            .field("clock-rate", 90000)
            .field("encoding-name", "H264")
            .field("payload", 125)
            // .field("packetization-mode", 1)
            // .field("ssrc", &"4278448574")
            // .field("cname", &"demo-1")
            .build();

        let udp_src_rtp_element = gstreamer::ElementFactory::make("udpsrc")
            .name(format!("udpsrc_h264_video_{}", timestamp)).build()
            .expect("Failed to create udpsrc element");
        udp_src_rtp_element.set_property("address", &"127.0.0.1");
        udp_src_rtp_element.set_property("port", &video_port);
        udp_src_rtp_element.set_property("caps", &video_caps);
        udp_src_rtp_element.set_property("reuse", &false);
        let udp_rtcp_src_element = gstreamer::ElementFactory::make("udpsrc")
            .name(format!("udpsrc_rtcp_h264_video_{}", timestamp)).build()
            .expect("Failed to create udpsrc element");
        udp_rtcp_src_element.set_property("address", &"127.0.0.1");
        udp_rtcp_src_element.set_property("port", &video_port + 1);
        let udp_rtcp_sink_element = gstreamer::ElementFactory::make("udpsink")
            .name(format!("udpsink_rtcp_h264_video_{}", timestamp)).build()
            .expect("Failed to create udpsink element");
        udp_rtcp_sink_element.set_property("host", &"127.0.0.1");
        udp_rtcp_sink_element.set_property("port", &video_port + 1);
        udp_rtcp_sink_element.set_property("async", &false);
        udp_rtcp_sink_element.set_property("sync", &false);
        let rtp_bin_element = gstreamer::ElementFactory::make("rtpbin")
            .name(format!("rtpbin_h264_video_{}", timestamp)).build()
            .expect("Failed to create rtpbin element");
        let queue_element = gstreamer::ElementFactory::make("queue")
            .name(format!("queue_h264_video_{}", timestamp))
            .build()
            .expect("Error creating queue_opus element");
        let rtp_h264_de_pay_element = gstreamer::ElementFactory::make("rtph264depay")
            .name(format!("rtph264depay_h264_video_{}", timestamp)).build()
            .expect("Failed to create rtph264depay element");
        // let output_pattern = String::from(format!("{}/video_chunk_%05d.ts", meeting_id));
        let muxer = ElementFactory::make("mpegtsmux")
            .name(format!("mpegtsmux_h264_video_{}", timestamp))
            .build()
            .expect("Error creating mpegtsmux element");
        let split_mux_sink_element = ElementFactory::make("splitmuxsink")
            .name(format!("splitmuxsink_h264_video_{}", timestamp))
            //.property("location", output_pattern)
            // .property("max-size-bytes", 1000000u64)
            .property("max-size-time", ClockTime::from_seconds(5).nseconds())
            // .property("send-keyframe-requests", true)
            // .property("muxer-factory", "mp4mux")
            .property("muxer", &muxer)
            .build()
            .expect("Error creating splitmuxsink element");
        let meeting_id_clone = meeting_id.clone();
        let participant_id_clone = participant_id.clone();
        let plain_transport_id_clone = transport_id.clone();
        split_mux_sink_element.connect("format-location", false, move |values| {
            // Generate the custom filename with a timestamp
            let timestamp_nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("Time went backwards")
                .as_nanos();
            let filename = format!("/opt/recordings/{}/{}/chunk_{}.ts", meeting_id_clone, participant_id_clone, timestamp_nanos);

            // Return the filename
            Some(glib::Value::from(&filename))
        });

        /* ------------------- Add elements to the pipeline ------------------- */
        pipeline.add_many(&[
            &udp_src_rtp_element,
            &udp_rtcp_src_element,
            &udp_rtcp_sink_element,
            &rtp_bin_element,
            &queue_element,
            &rtp_h264_de_pay_element,
            &split_mux_sink_element
        ]).expect("Failed to add elements to the pipeline");
        /*---------------------------------------------------------------------*/

        /* ------------------- Start Linking of pads / elements ------------------- */
        udp_src_rtp_element.link(&rtp_bin_element).expect("Failed to link udpsrc and fakesink");
        let udp_rtcp_src_pad = udp_rtcp_src_element.static_pad("src")
            .expect("Failed to get src pad from udpsrc");
        let rtp_bin_rtcp_sink_pad = rtp_bin_element.request_pad_simple("recv_rtcp_sink_%u");
        udp_rtcp_src_pad.link(&rtp_bin_rtcp_sink_pad.unwrap()).expect("Failed to link udpsrc and rtpbin");
        let queue_element_arc = Arc::new(Mutex::new(queue_element.clone()));
        let queue_element_clone = Arc::clone(&queue_element_arc);
        rtp_bin_element.connect_pad_added(move |rtpbin, pad| {
            info!("Pad added: {}", pad.name());
            info!("{:#?}", pad);
            if pad.name().starts_with("recv_rtp_src_") {
                info!("{:#?}",pad.query_caps(None));
                info!("New pad added: {}", pad.name());

                // Get the sink pad of rtpopusdepay
                let queue_sink_pad = queue_element_clone.lock().unwrap().static_pad("sink")
                    .expect("queue should have a sink pad");
                info!("Fake Sink Pad {:#?}",queue_sink_pad.query_caps(None));
                // Link the rtpbin pad to the depayloader
                match pad.link(&queue_sink_pad) {
                    Ok(_) => {
                        info!("Successfully linked rtpbin to queue");
                    }
                    Err(e) => {
                        error!("Failed to link rtpbin to queue {}", e);
                    }
                }
            } else if pad.name().starts_with("recv_rtcp_src_") {
                info!("{:#?}",pad.query_caps(None));
                info!("New pad added: {}", pad.name());

                // Get the sink pad of udp sink
                let udp_rtcp_sink_sink_pad = udp_rtcp_sink_element.static_pad("sink")
                    .expect("udpsink should have a sink pad");
                info!("Udp Sink Pad {:#?}",udp_rtcp_sink_sink_pad.query_caps(None));
                pad.link(&udp_rtcp_sink_sink_pad).expect("Failed to link rtpbin to udpsink");
            }
        });
        if queue_element.link(&rtp_h264_de_pay_element).is_ok() {
            info!("Successfully linked queue and rtp_h264_de_pay");
        } else {
            error!("Failed to link queue and rtp_h264_de_pay");
            return Err("Failed to link queue and rtp_h264_de_pay".to_string());
        }
        let rtp_h264_de_pay_src_pad = rtp_h264_de_pay_element.static_pad("src")
            .expect("Failed to get src pad from rtp_h264_de_pay");
        let split_mux_sink_video_pad = match split_mux_sink_element.request_pad_simple("video") {
            Some(pad) => pad,
            None => {
                error!("Failed to get video pad from splitmuxsink");
                return Err("Failed to get video pad from splitmuxsink".to_string());
            }
        };
        if rtp_h264_de_pay_src_pad.link(&split_mux_sink_video_pad).is_ok() {
            info!("Successfully linked h264_parse and split mux");
        } else {
            error!("Failed to link h264_parse and split mux");
            return Err("Failed to link h264_parse and split mux".to_string());
        }
        /*---------------------------------------------------------------------*/

        /* ------------------- Debugging help like adding of probes ... ------------------- */

        /*---------------------------------------------------------------------*/
        Ok(pipeline)
    }
}
