// ! Work in progress and none of the below functions are being used currently.
// ! We are re-thinking on this.
use std::collections::HashMap;
use std::os::unix::raw::ino_t;
use std::sync::{Arc};
use std::sync::mpsc::{SyncSender, Receiver};
use std::thread;
use std::thread::sleep;
use std::time::{Duration, Instant};
use chrono::Utc;
use gstreamer::{glib, Element};
use gstreamer::prelude::{Cast, ElementExt, GstObjectExt, ObjectExt};
use log::{debug, error, info};
use mediasoup::consumer::Consumer;
use mediasoup::data_structures::{DtlsState, IceRole, IceState};
use mediasoup::producer::Producer;
use mediasoup::transport::TransportGeneric;
use mediasoup::prelude::{MediaKind, Transport};
use mediasoup::webrtc_transport::{WebRtcTransport, WebRtcTransportStat};
use metrics::{gauge, histogram};
use opentelemetry::{Key, KeyValue, StringValue, Value};
use opentelemetry::metrics::Meter;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task;
use crate::client::AnalyticMetricClient;
use crate::gstreamer::Gstreamer;
use crate::InstanceMeta;
use crate::mediasoupv1::{ConsumerInMemory, ParticipantConsumerInMemory, ParticipantProducerInMemory, ProducerInMemory, RouterInMemory, TransportInMemory};
use crate::common::{UserEvent};
// use crate::mediasoupv1::gcp_metrics::{GcpMetrics, TokenCache};

pub struct Metrics {}
impl Metrics {
    /*pub async fn push_auto_scale_metrics(total_capacity : i32, router_map: Arc<RwLock<HashMap<String, RouterInMemory>>>, instance_meta: InstanceMeta) {
        info!("Initiating thread for pushing auto scale metrics");
        let client = Client::new();
        let cache = Arc::new(Mutex::new(None));
        loop {
            let mut current_capacity = 0;
            for (_, value) in router_map.read().await.iter() {
                let router = value.clone();
                current_capacity = current_capacity + router.capacity;
            }
            debug!("Current capacity: {}", current_capacity);
            // let remaining_capacity = total_capacity - current_capacity;
            GcpMetrics::send_metric(&client, cache.clone(), current_capacity as f64, instance_meta.clone()).await;
            thread::sleep(Duration::from_secs(5));
        }
    }*/

    pub async fn push_metrics_for_pipeline(pipeline_map: Arc<RwLock<HashMap<String, Option<Gstreamer>>>>, meter: Meter, host: String) {
        loop {
            sleep(Duration::from_secs(5));
            for (key, value) in pipeline_map.read().await.iter() {
                /*if value.is_some() {
                    let gstreamer = value.clone().unwrap();
                    let meeting_id = gstreamer.meeting_id.unwrap().clone();
                    let participant_id = gstreamer.participant_id.unwrap().clone();
                    let gstreamer_inner = gstreamer.inner.clone();
                    let gst_element_lock = gstreamer_inner.lock().unwrap();
                    let rtp_element_audio_lock = gst_element_lock.rtp_bin_element_audio.clone();
                    let rtp_element_video_lock = gst_element_lock.rtp_bin_element_video.clone();
                    let session_id_arc = gst_element_lock.session_id.clone();
                    let host_key_value = KeyValue::new("host", host.clone());
                    let participant_key_value = KeyValue::new("participant_id", participant_id.clone());
                    let mut attributes = vec![host_key_value.clone(), participant_key_value.clone()];
                    if session_id_arc.lock().unwrap().clone().is_some() && (rtp_element_audio_lock.is_some() || rtp_element_video_lock.is_some()) {
                        let session_id = session_id_arc.lock().unwrap().clone().unwrap();
                        let queue_element;
                        let rtp_element_unlock = if rtp_element_audio_lock.is_some() {
                            attributes.push(KeyValue::new("media_type", "audio"));
                            queue_element = gst_element_lock.queue_element_audio.clone().unwrap();
                            rtp_element_audio_lock.clone().unwrap()
                        } else {
                            attributes.push(KeyValue::new("media_type", "video"));
                            queue_element = gst_element_lock.queue_element_vp8_video.clone().unwrap();
                            rtp_element_video_lock.clone().unwrap()
                        };

                        {
                            let queue_element_unlock = queue_element.lock().unwrap().clone();
                            let current_level_bytes : u32 = queue_element_unlock
                                .property::<glib::Value>("current-level-bytes").get::<u32>().expect("Failed to get current-level-bytes");
                            meter.u64_histogram("metric_".to_string() + meeting_id.clone().as_str() + "_gstreamer.queue_element.current_level_bytes")
                                .init()
                                .record(current_level_bytes as u64, &attributes);
                            let current_level_time : u64 = queue_element_unlock
                                .property::<glib::Value>("current-level-time").get::<u64>().expect("Failed to get current-level-time");
                            meter.u64_histogram("metric_".to_string() + meeting_id.clone().as_str() + "_gstreamer.queue_element.current_level_time")
                                .init()
                                .record(current_level_time, &attributes);
                            let current_level_buffers : u32 = queue_element_unlock
                                .property::<glib::Value>("current-level-buffers").get::<u32>().expect("Failed to get current-level-buffers");
                            meter.u64_histogram("metric_".to_string() + meeting_id.clone().as_str() + "_gstreamer.queue_element.current_level_buffers")
                                .init()
                                .record(current_level_buffers as u64, &attributes);
                        }
                        let rtp_element = rtp_element_unlock.lock().unwrap();
                        let session_obj = rtp_element.emit_by_name::<glib::Object>("get-internal-session", &[&session_id]);
                        //info!("Successfully retrieved RTP session for session ID: {}", session_id);
                        // Fetch stats property
                        if let Some(stats) = session_obj.property::<Option<gstreamer::Structure>>("stats") {
                            //info!("Successfully retrieved stats {:?} for session ID: {}", stats, session_id);
                            match stats.get::<glib::ValueArray>("source-stats") {
                                Ok(source_stats) => {
                                    for (i, value) in source_stats.iter().enumerate() {
                                        if let Ok(structure) = value.get::<gstreamer::Structure>() {
                                            if let Some(is_sender) = structure.get::<bool>("is-sender").ok() {
                                                //info!("Source Stats Value {}: is-sender: {}", i, is_sender);
                                                if is_sender {
                                                    if let Some(sent_nack_count) = stats.get::<u32>("sent-nack-count").ok() {
                                                        meter.u64_gauge("metric_".to_string() + meeting_id.clone().as_str() + "_gstreamer.rtp_bin.sent_nack_count")
                                                            .init()
                                                            .record(sent_nack_count.into(), &attributes);
                                                    }
                                                    if let Some(recv_nack_count) = stats.get::<u32>("recv-nack-count").ok() {
                                                        meter.u64_gauge("metric_".to_string() + meeting_id.clone().as_str() + "_gstreamer.rtp_bin.recv_nack_count")
                                                            .init()
                                                            .record(recv_nack_count.into(), &attributes);
                                                    }
                                                    if let Some(rtx_count) = stats.get::<u32>("rtx-count").ok() {
                                                        meter.u64_gauge("metric_".to_string() + meeting_id.clone().as_str() + "_gstreamer.rtp_bin.rtx_count")
                                                            .init()
                                                            .record(rtx_count.into(), &attributes);
                                                    }
                                                    if let Some(recv_rtx_req_count) = stats.get::<u32>("recv-rtx-req-count").ok() {
                                                        meter.u64_gauge("metric_".to_string() + meeting_id.clone().as_str() + "_gstreamer.rtp_bin.recv_rtx_req_count")
                                                            .init()
                                                            .record(recv_rtx_req_count.into(), &attributes);
                                                    }
                                                    if let Some(recv_nack_count) = stats.get::<u32>("rtx-drop-count").ok() {
                                                        meter.u64_gauge("metric_".to_string() + meeting_id.clone().as_str() + "_gstreamer.rtp_bin.rtx_drop_count")
                                                            .init()
                                                            .record(recv_nack_count.into(), &attributes);
                                                    }
                                                    if let Some(sent_rtx_req_count) = stats.get::<u32>("sent-rtx-req-count").ok() {
                                                        meter.u64_gauge("metric_".to_string() + meeting_id.clone().as_str() + "_gstreamer.rtp_bin.sent_rtx_req_count")
                                                            .init()
                                                            .record(sent_rtx_req_count.into(), &attributes);
                                                    }

                                                    if let Some(bytes_received) = structure.get::<u64>("bytes-received").ok() {
                                                        meter.u64_gauge("metric_".to_string() + meeting_id.clone().as_str() + "_gstreamer.rtp_bin.bytes_received")
                                                            .init()
                                                            .record(bytes_received, &attributes);
                                                    }
                                                    if let Some(packets_sent) = structure.get::<u64>("packets-sent").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.packets_sent", meeting_id))
                                                            .init()
                                                            .record(packets_sent, &attributes);
                                                    }
                                                    if let Some(packets_received) = structure.get::<u64>("packets-received").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.packets_received", meeting_id))
                                                            .init()
                                                            .record(packets_received, &attributes);
                                                    }
                                                    if let Some(packets_lost) = structure.get::<i32>("packets-lost").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.packets_lost", meeting_id))
                                                            .init()
                                                            .record(packets_lost.max(0) as u64, &attributes);
                                                    }
                                                    if let Some(jitter) = structure.get::<u32>("jitter").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.jitter", meeting_id))
                                                            .init()
                                                            .record(jitter as u64, &attributes);
                                                    }
                                                    if let Some(bitrate) = structure.get::<u64>("bitrate").ok() {
                                                        meter.u64_histogram(format!("metric_{}_gstreamer.rtp_bin.bitrate", meeting_id))
                                                            .init()
                                                            .record(bitrate, &attributes);
                                                    }
                                                    if let Some(sent_pli_count) = structure.get::<u32>("sent-pli-count").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.sent_pli_count", meeting_id))
                                                            .init()
                                                            .record(sent_pli_count as u64, &attributes);
                                                    }

                                                    if let Some(recv_pli_count) = structure.get::<u32>("recv-pli-count").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.recv_pli_count", meeting_id))
                                                            .init()
                                                            .record(recv_pli_count as u64, &attributes);
                                                    }

                                                    if let Some(sent_fir_count) = structure.get::<u32>("sent-fir-count").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.sent_fir_count", meeting_id))
                                                            .init()
                                                            .record(sent_fir_count as u64, &attributes);
                                                    }

                                                    if let Some(recv_fir_count) = structure.get::<u32>("recv-fir-count").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.recv_fir_count", meeting_id))
                                                            .init()
                                                            .record(recv_fir_count as u64, &attributes);
                                                    }

                                                    if let Some(recv_packet_rate) = structure.get::<u32>("recv-packet-rate").ok() {
                                                        meter.u64_histogram(format!("metric_{}_gstreamer.rtp_bin.recv_packet_rate", meeting_id))
                                                            .init()
                                                            .record(recv_packet_rate as u64, &attributes);
                                                    }

                                                    if let Some(sr_ntptime) = structure.get::<u64>("sr-ntptime").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.sr_ntptime", meeting_id))
                                                            .init()
                                                            .record(sr_ntptime, &attributes);
                                                    }

                                                    if let Some(sr_rtptime) = structure.get::<u32>("sr-rtptime").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.sr_rtptime", meeting_id))
                                                            .init()
                                                            .record(sr_rtptime as u64, &attributes);
                                                    }

                                                    if let Some(sr_octet_count) = structure.get::<u32>("sr-octet-count").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.sr_octet_count", meeting_id))
                                                            .init()
                                                            .record(sr_octet_count as u64, &attributes);
                                                    }

                                                    if let Some(sr_packet_count) = structure.get::<u32>("sr-packet-count").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.sr_packet_count", meeting_id))
                                                            .init()
                                                            .record(sr_packet_count as u64, &attributes);
                                                    }

                                                    if let Some(sent_rb_fractionlost) = structure.get::<u32>("sent-rb-fractionlost").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.sent_rb_fractionlost", meeting_id))
                                                            .init()
                                                            .record(sent_rb_fractionlost as u64, &attributes);
                                                    }

                                                    if let Some(sent_rb_packetslost) = structure.get::<i32>("sent-rb-packetslost").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.sent_rb_packetslost", meeting_id))
                                                            .init()
                                                            .record(sent_rb_packetslost.max(0) as u64, &attributes);
                                                    }

                                                    if let Some(sent_rb_jitter) = structure.get::<u32>("sent-rb-jitter").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.sent_rb_jitter", meeting_id))
                                                            .init()
                                                            .record(sent_rb_jitter as u64, &attributes);
                                                    }

                                                    if let Some(rb_packetslost) = structure.get::<i32>("rb-packetslost").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.rb_packetslost", meeting_id))
                                                            .init()
                                                            .record(rb_packetslost.max(0) as u64, &attributes);
                                                    }

                                                    if let Some(rb_jitter) = structure.get::<u32>("rb-jitter").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.rb_jitter", meeting_id))
                                                            .init()
                                                            .record(rb_jitter as u64, &attributes);
                                                    }

                                                    if let Some(rb_round_trip) = structure.get::<u32>("rb-round-trip").ok() {
                                                        meter.u64_gauge(format!("metric_{}_gstreamer.rtp_bin.rb_round_trip", meeting_id))
                                                            .init()
                                                            .record(rb_round_trip as u64, &attributes);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Error in getting source stats: {}", e);
                                }
                            }
                        }
                    };
                }*/
            }
        }
    }

    pub async fn push_metrics_for_transports(transport_map: Arc<RwLock<HashMap<String, TransportInMemory>>>, meter: Meter, host: String) {
        info!("Initiating thread for pushing metrics for transports");
        loop {
            for (key, value) in transport_map.read().await.iter() {
                let transport_in_memory = value.clone();
                let host = host.clone();
                let meter = meter.clone();
                Metrics::push_metrics(transport_in_memory, meter, host).await;
            }
            thread::sleep(Duration::from_secs(5));
        }
    }
    pub async fn push_metrics(transport_in_memory: TransportInMemory, meter: Meter, host: String) {
        let stats = match transport_in_memory.transport.get_stats().await {
            Ok(s) => s,
            Err(e) => {
                error!("Error in getting stats: {}", e);
                return;
            }
        };
        // info!("Pushing metrics for transport {}", transport_in_memory.transport.id());
        let host_key_value = KeyValue::new("host", host.clone());
        let participant_key_value = KeyValue::new("participant_id", transport_in_memory.participant_id.clone());
        let attributes = vec![host_key_value.clone(), participant_key_value.clone()];
        for stat in stats.iter() {
            meter.u64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.bytes_received").init().record(stat.bytes_received, &attributes);
            meter.u64_histogram("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.recv_bitrate").init().record(stat.recv_bitrate as u64, &attributes);
            meter.u64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.bytes_sent").init().record(stat.bytes_sent, &attributes);
            meter.u64_histogram("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.send_bitrate").init().record(stat.send_bitrate as u64, &attributes);
            meter.u64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.rtp_bytes_received").init().record(stat.rtp_bytes_received, &attributes);
            meter.u64_histogram("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.rtp_recv_bitrate").init().record(stat.rtp_recv_bitrate as u64, &attributes);
            meter.u64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.rtp_bytes_sent").init().record(stat.rtp_bytes_sent, &attributes);
            meter.u64_histogram("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.rtp_send_bitrate").init().record(stat.rtp_send_bitrate as u64, &attributes);
            meter.u64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.rtx_bytes_received").init().record(stat.rtx_bytes_received, &attributes);
            meter.u64_histogram("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.rtx_recv_bitrate").init().record(stat.rtx_recv_bitrate as u64, &attributes);
            meter.u64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.rtx_bytes_sent").init().record(stat.rtx_bytes_sent, &attributes);
            meter.u64_histogram("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.rtx_send_bitrate").init().record(stat.rtx_send_bitrate as u64, &attributes);
            meter.u64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.probation_bytes_sent").init().record(stat.probation_bytes_sent, &attributes);
            meter.u64_histogram("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.probation_send_bitrate").init().record(stat.probation_send_bitrate as u64, &attributes);
            if stat.available_outgoing_bitrate.is_some() {
                meter.u64_histogram("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.available_incoming_bitrate").init().record(stat.available_outgoing_bitrate.unwrap() as u64, &attributes);
            }
            if stat.available_incoming_bitrate.is_some() {
                meter.u64_histogram("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.available_incoming_bitrate").init().record(stat.available_incoming_bitrate.unwrap() as u64, &attributes);
            }
            if stat.max_incoming_bitrate.is_some() {
                meter.u64_histogram("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.max_incoming_bitrate").init().record(stat.max_incoming_bitrate.unwrap() as u64, &attributes);
            }
            if stat.max_outgoing_bitrate.is_some() {
                meter.u64_histogram("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.max_outgoing_bitrate").init().record(stat.max_outgoing_bitrate.unwrap() as u64, &attributes);
            }
            if stat.min_outgoing_bitrate.is_some() {
                meter.u64_histogram("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.min_outgoing_bitrate").init().record(stat.min_outgoing_bitrate.unwrap() as u64, &attributes);
            }
            if stat.rtp_packet_loss_received.is_some() {
                meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.rtp_packet_loss_received").init().record(stat.rtp_packet_loss_received.unwrap(), &attributes);
            }
            if stat.rtp_packet_loss_sent.is_some() {
                meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.rtp_packet_loss_sent").init().record(stat.rtp_packet_loss_sent.unwrap(), &attributes);
            }

            if stat.rtp_packet_loss_sent.is_some() {
                meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.rtp_packet_loss_sent").init().record(stat.rtp_packet_loss_sent.unwrap(), &attributes);
            }
            match stat.ice_role {
                IceRole::Controlling => {
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_role.controlling").init().record(1.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_role.controlled").init().record(0.0, &attributes);
                }
                IceRole::Controlled => {
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_role.controlling").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_role.controlled").init().record(1.0, &attributes)
                }
            }
            match stat.ice_state {
                IceState::New => {
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_state.new").init().record(1.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_state.connected").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_state.completed").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_state.disconnected").init().record(0.0, &attributes);
                }
                IceState::Connected => {
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_state.new").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_state.connected").init().record(1.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_state.completed").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_state.disconnected").init().record(0.0, &attributes);
                }
                IceState::Completed => {
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_state.new").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_state.connected").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_state.completed").init().record(1.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_state.disconnected").init().record(0.0, &attributes);
                }
                IceState::Disconnected => {
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_state.new").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_state.connected").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_state.completed").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.ice_state.disconnected").init().record(1.0, &attributes);
                }
            }
            match stat.dtls_state {
                DtlsState::New => {
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.new").init().record(1.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.connected").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.closed").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.connecting").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.failed").init().record(0.0, &attributes);
                }
                DtlsState::Connecting => {
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.new").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.connected").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.connecting").init().record(1.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.closed").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.failed").init().record(0.0, &attributes);
                }
                DtlsState::Connected => {
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.new").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.connected").init().record(1.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.closed").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.connecting").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.failed").init().record(0.0, &attributes);
                }
                DtlsState::Failed => {
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.new").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.connected").init().record(1.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.closed").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.connecting").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.failed").init().record(1.0, &attributes);
                }
                DtlsState::Closed => {
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.new").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.connected").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.closed").init().record(1.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.connecting").init().record(0.0, &attributes);
                    meter.f64_gauge("metric_".to_string() + transport_in_memory.meeting_id.clone().as_str() + "_webrtc_transport.stat.dtls_state.failed").init().record(0.0, &attributes);
                }
            }
        }
    }

    pub async fn push_metrics_for_producers(producers: Arc<RwLock<HashMap<String, ProducerInMemory>>>, meter: Meter, host: String) {
        info!("Initiating thread for pushing metrics for producers");
        loop {
            for (key, value) in producers.read().await.iter() {
                Metrics::push_metrics_for_producer(value.clone(), meter.clone(), host.clone()).await;
            }
            thread::sleep(Duration::from_secs(5));
        }
    }

    pub async fn push_metrics_for_producer(producer_in_memory: ProducerInMemory, meter: Meter, host: String) {
        let stats = match producer_in_memory.producer.get_stats().await {
            Ok(s) => s,
            Err(e) => {
                error!("Error in getting stats: {}", e);
                return;
            }
        };
        // info!("Pushing metrics for producer {}", producer.id());
        let host_key_value = KeyValue::new("host", host.clone());
        let participant_key_value = KeyValue::new("participant_id", producer_in_memory.participant_id.clone());
        let kind_key_value = KeyValue::new("kind", producer_in_memory.kind.clone());
        let attributes = vec![host_key_value.clone(), participant_key_value.clone(), kind_key_value.clone()];
        for stat in stats.iter() {
            meter.f64_gauge("metric_".to_string() + producer_in_memory.meeting_id.clone().as_str() + "_producer.stat.packets_lost").init().record(stat.packets_lost as f64, &attributes);
            meter.u64_gauge("metric_".to_string() + producer_in_memory.meeting_id.clone().as_str() + "_producer.stat.fraction_lost").init().record(stat.fraction_lost as u64, &attributes);
            meter.u64_gauge("metric_".to_string() + producer_in_memory.meeting_id.clone().as_str() + "_producer.stat.packets_discarded").init().record(stat.packets_discarded, &attributes);
            meter.u64_gauge("metric_".to_string() + producer_in_memory.meeting_id.clone().as_str() + "_producer.stat.packets_retransmitted").init().record(stat.packets_retransmitted, &attributes);
            meter.u64_gauge("metric_".to_string() + producer_in_memory.meeting_id.clone().as_str() + "_producer.stat.packets_repaired").init().record(stat.packets_repaired, &attributes);
            meter.u64_gauge("metric_".to_string() + producer_in_memory.meeting_id.clone().as_str() + "_producer.stat.nack_count").init().record(stat.nack_count, &attributes);
            meter.u64_gauge("metric_".to_string() + producer_in_memory.meeting_id.clone().as_str() + "_producer.stat.nack_packet_count").init().record(stat.nack_packet_count, &attributes);
            meter.u64_gauge("metric_".to_string() + producer_in_memory.meeting_id.clone().as_str() + "_producer.stat.pli_count").init().record(stat.pli_count, &attributes);
            meter.u64_gauge("metric_".to_string() + producer_in_memory.meeting_id.clone().as_str() + "_producer.stat.fir_count").init().record(stat.fir_count, &attributes);
            meter.u64_gauge("metric_".to_string() + producer_in_memory.meeting_id.clone().as_str() + "_producer.stat.score").init().record(stat.score as u64, &attributes);
            meter.u64_gauge("metric_".to_string() + producer_in_memory.meeting_id.clone().as_str() + "_producer.stat.packet_count").init().record(stat.packet_count, &attributes);
            meter.u64_gauge("metric_".to_string() + producer_in_memory.meeting_id.clone().as_str() + "_producer.stat.byte_count").init().record(stat.byte_count, &attributes);
            meter.u64_histogram("metric_".to_string() + producer_in_memory.meeting_id.clone().as_str() + "_producer.stat.bitrate").init().record(stat.bitrate as u64, &attributes);
            if stat.round_trip_time.is_some() {
                meter.f64_gauge("metric_".to_string() + producer_in_memory.meeting_id.clone().as_str() + "_producer.stat.round_trip_time").init().record(stat.round_trip_time.unwrap() as f64, &attributes);
            }
            if stat.rtx_packets_discarded.is_some() {
                meter.u64_gauge("metric_".to_string() + producer_in_memory.meeting_id.clone().as_str() + "_producer.stat.rtx_packets_discarded").init().record(stat.rtx_packets_discarded.unwrap(), &attributes);
            }
            meter.f64_gauge("metric_".to_string() + producer_in_memory.meeting_id.clone().as_str() + "_producer.stat.jitter").init().record(stat.jitter as f64, &attributes);
        }
    }

    pub async fn push_metrics_for_consumers(consumers: Arc<RwLock<HashMap<String, ConsumerInMemory>>>, meter: Meter, host: String) {
        info!("Initiating thread for pushing metrics for consumers");
        loop {
            for (key, value) in consumers.read().await.iter() {
                let consumers = value.clone();
                Metrics::push_metrics_for_consumer(value.clone(), meter.clone(), host.clone()).await;
            }
            thread::sleep(Duration::from_secs(5));
        }
    }

    pub async fn push_metrics_for_consumer(consumer_in_memory: ConsumerInMemory, meter: Meter, host: String) {
        let stats = match consumer_in_memory.consumer.get_stats().await {
            Ok(s) => s,
            Err(e) => {
                //error!("Error in getting stats: {}", e);
                return;
            }
        };
        let stat = stats.consumer_stats();
        let host_key_value = KeyValue::new("host", host.clone());
        let participant_key_value = KeyValue::new("participant_id", consumer_in_memory.participant_id.clone());
        fn media_kind_to_string(kind: &MediaKind) -> String {
            match kind {
                MediaKind::Audio => "audio".to_string(),
                MediaKind::Video => "video".to_string(),
            }
        }
        let kind_value = media_kind_to_string(&consumer_in_memory.consumer.kind().clone());
        let kind_key_value = KeyValue::new("kind", Value::String(StringValue::from(kind_value)));
        let attributes = vec![host_key_value.clone(), participant_key_value.clone(), kind_key_value.clone()];
        // info!("Pushing metrics for consumer {}", consumer.id());
        meter.u64_gauge("metric_".to_string() + consumer_in_memory.meeting_id.clone().as_str() + "_consumer.stat.packets_lost").init().record(stat.packets_lost, &attributes);
        meter.u64_gauge("metric_".to_string() + consumer_in_memory.meeting_id.clone().as_str() + "_consumer.stat.fraction_lost").init().record(stat.fraction_lost as u64, &attributes);
        meter.u64_gauge("metric_".to_string() + consumer_in_memory.meeting_id.clone().as_str() + "_consumer.stat.packets_discarded").init().record(stat.packets_discarded, &attributes);
        meter.u64_gauge("metric_".to_string() + consumer_in_memory.meeting_id.clone().as_str() + "_consumer.stat.packets_retransmitted").init().record(stat.packets_retransmitted, &attributes);
        meter.u64_gauge("metric_".to_string() + consumer_in_memory.meeting_id.clone().as_str() + "_consumer.stat.packets_repaired").init().record(stat.packets_repaired, &attributes);
        meter.u64_gauge("metric_".to_string() + consumer_in_memory.meeting_id.clone().as_str() + "_consumer.stat.nack_count").init().record(stat.nack_count, &attributes);
        meter.u64_gauge("metric_".to_string() + consumer_in_memory.meeting_id.clone().as_str() + "_consumer.stat.nack_packet_count").init().record(stat.nack_packet_count, &attributes);
        meter.u64_gauge("metric_".to_string() + consumer_in_memory.meeting_id.clone().as_str() + "_consumer.stat.pli_count").init().record(stat.pli_count, &attributes);
        meter.u64_gauge("metric_".to_string() + consumer_in_memory.meeting_id.clone().as_str() + "_consumer.stat.fir_count").init().record(stat.fir_count, &attributes);
        meter.u64_gauge("metric_".to_string() + consumer_in_memory.meeting_id.clone().as_str() + "_consumer.stat.score").init().record(stat.score as u64, &attributes);
        meter.u64_gauge("metric_".to_string() + consumer_in_memory.meeting_id.clone().as_str() + "_consumer.stat.packet_count").init().record(stat.packet_count, &attributes);
        meter.u64_gauge("metric_".to_string() + consumer_in_memory.meeting_id.clone().as_str() + "_consumer.stat.byte_count").init().record(stat.byte_count, &attributes);
        meter.u64_histogram("metric_".to_string() + consumer_in_memory.meeting_id.clone().as_str() + "_consumer.stat.bitrate").init().record(stat.bitrate as u64, &attributes);
        if stat.round_trip_time.is_some() {
            meter.u64_gauge("metric_".to_string() + consumer_in_memory.meeting_id.clone().as_str() + "_consumer.stat.round_trip_time").init().record(stat.round_trip_time.unwrap() as u64, &attributes);
        }
    }

    pub async fn receive_and_send_user_event(mut rx: Arc<Mutex<Receiver<UserEvent>>>, client: Arc<AnalyticMetricClient>) {
        let retries= 3;
        while let Ok(event) = rx.lock().await.recv() {
            let mut attempt = 0;
            while attempt < retries {
                match client.send_event("doordarshan-user-session-events", event.clone()).await {
                    Ok(_) => {
                        info!("event published successfully");
                        break;
                    }
                    Err(e) => {
                        error!("error sending event: {}", e);
                        attempt += 1
                    }
                }
            }
        }
    }
}
