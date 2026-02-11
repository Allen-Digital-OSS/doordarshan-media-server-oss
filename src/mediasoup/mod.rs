//! SFU (Selective Forwarding Unit) configuration and helpers for the media server.
//!
//! This module provides configuration data structures and environment-driven
//! loaders used by the SFU layer (MediaSoup integration). The main purpose is
//! to centralize runtime defaults and to offer a `SFUConfig::from_env` helper
//! that reads common configuration from environment variables (or a `.env`
//! file via `crate::common::get_env`).
//!
//! Important notes:
//! - Values are parsed from environment variables using `common::get_env` and
//!   `parse()`; invalid values will cause an `expect` panic with a clear
//!   message. In production you may prefer fallible loading with proper
//!   error propagation.
//!
//! Example
//! ```no_run
//! // Load config from environment (or .env)
//! let cfg = crate::mediasoup::SFUConfig::from_env();
//! println!("SFU listening on {} - {}", cfg.rtc_port_range_start, cfg.rtc_port_range_end);
//! ```

use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::common;

const DEFAULT_MAX_PARTICIPANT_CAPACITY_PER_WORKER: &str = "240";
const DEFAULT_MAX_PRODUCERS_OR_CONSUMERS_PER_WORKER: &str = "480";
const DEFAULT_RTC_PORT_RANGE_START: &'static str = "40000";
const DEFAULT_RTC_PORT_RANGE_END: &'static str = "45000";
const DEFAULT_PLAIN_TRANSPORT_VIDEO_PORT_RANGE_START: &'static str = "50000";
const DEFAULT_PLAIN_TRANSPORT_VIDEO_PORT_RANGE_END: &'static str = "55000";
const DEFAULT_PLAIN_TRANSPORT_AUDIO_PORT_RANGE_START: &'static str = "60000";
const DEFAULT_PLAIN_TRANSPORT_AUDIO_PORT_RANGE_END: &'static str = "65000";
const DEFAULT_RTC_LISTEN_IP: &'static str = "0.0.0.0";
const DEFAULT_MAX_INCOMING_BITRATE: &'static str = "10000000";
const DEFAULT_MIN_OUTGOING_BITRATE: &'static str = "100000";
const DEFAULT_MAX_OUTGOING_BITRATE: &'static str = "10000000";

const DEFAULT_USER_CHANNEL_EVENT_COUNT: &'static str = "100";

const DEFAULT_CONSUMER_SHARD_COUNT: &'static str = "6";

const DEFAULT_AWS_BUCKET_NAME: &'static str = "dd-sfu-bucket";

/// Configuration for the SFU.
///
/// This struct holds typed configuration consumed by the SFU/mediasoup
/// integration. Fields include port ranges, bitrate limits, IPs to bind/announce.
/// Use `SFUConfig::from_env()` to construct this type from environment variables
/// (recommended for CLI or containerized deployments); `SFUConfig::default()`
/// returns sensible defaults suitable for local development.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SFUConfig {
    /// Maximum participants per worker (default from DEFAULT_MAX_PARTICIPANT_CAPACITY_PER_WORKER)
    pub rtc_default_max_participants_per_worker: i32,
    /// Maximum producers or consumers per worker (default from DEFAULT_MAX_PRODUCERS_OR_CONSUMERS_PER_WORKER)
    pub rtc_default_max_producers_or_consumers_per_worker: i32,
    /// Start of the RTP/RTCP port range used by transports
    pub rtc_port_range_start: u16,
    /// End of the RTP/RTCP port range used by transports
    pub rtc_port_range_end: u16,
    /// Start of the plain transport port range for video
    pub plain_transport_video_port_range_start: u16,
    /// End of the plain transport port range for video
    pub plain_transport_video_port_range_end: u16,
    /// Start of the plain transport port range for audio
    pub plain_transport_audio_port_range_start: u16,
    /// End of the plain transport port range for audio
    pub plain_transport_audio_port_range_end: u16,
    /// The local IP to bind RTC listeners to
    pub rtc_listen_ip: IpAddr,
    /// Maximum incoming bitrate (bps)
    pub max_incoming_bitrate: u32,
    /// Minimum outgoing bitrate (bps)
    pub min_outgoing_bitrate: u32,
    /// Maximum outgoing bitrate (bps)
    pub max_outgoing_bitrate: u32,
    //TODO: move this config out of SFUConfig
    /// Number of channels used for user event sharding
    pub user_event_channel_count: usize,
    /// How many shards to split consumers into (used for scaling)
    pub consumer_shard_count: usize
}

/// Implement the default SFU configuration used when no environment values are provided.
impl Default for SFUConfig {
    fn default() -> Self {
        Self {
            rtc_default_max_participants_per_worker: DEFAULT_MAX_PARTICIPANT_CAPACITY_PER_WORKER
                .parse()
                .unwrap(),
            rtc_default_max_producers_or_consumers_per_worker:
            DEFAULT_MAX_PRODUCERS_OR_CONSUMERS_PER_WORKER
                .parse()
                .unwrap(),
            rtc_port_range_start: DEFAULT_RTC_PORT_RANGE_START.parse().unwrap(),
            rtc_port_range_end: DEFAULT_RTC_PORT_RANGE_END.parse().unwrap(),
            plain_transport_video_port_range_start: DEFAULT_PLAIN_TRANSPORT_VIDEO_PORT_RANGE_START.parse().unwrap(),
            plain_transport_video_port_range_end: DEFAULT_PLAIN_TRANSPORT_VIDEO_PORT_RANGE_END.parse().unwrap(),
            plain_transport_audio_port_range_start: DEFAULT_PLAIN_TRANSPORT_AUDIO_PORT_RANGE_START.parse().unwrap(),
            plain_transport_audio_port_range_end: DEFAULT_PLAIN_TRANSPORT_AUDIO_PORT_RANGE_END.parse().unwrap(),
            rtc_listen_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            max_incoming_bitrate: DEFAULT_MAX_INCOMING_BITRATE.parse().unwrap(),
            min_outgoing_bitrate: DEFAULT_MIN_OUTGOING_BITRATE.parse().unwrap(),
            max_outgoing_bitrate: DEFAULT_MAX_OUTGOING_BITRATE.parse().unwrap(),
            user_event_channel_count: DEFAULT_USER_CHANNEL_EVENT_COUNT.parse().unwrap(),
            consumer_shard_count: DEFAULT_CONSUMER_SHARD_COUNT.parse().unwrap(),
        }
    }
}

const RTC_DEFAULT_MAX_PARTICIPANTS_PER_WORKER_CONF_KEY: &'static str =
    "RTC_DEFAULT_MAX_PARTICIPANTS_PER_WORKER";

const RTC_DEFAULT_MAX_PRODUCERS_OR_CONSUMERS_PER_WORKER_CONF_KEY: &'static str =
    "RTC_DEFAULT_MAX_PRODUCERS_OR_CONSUMERS_PER_WORKER";

const RTC_PORT_RANGE_START_CONF_KEY: &'static str = "RTC_PORT_RANGE_START";

const RTC_PORT_RANGE_END_CONF_KEY: &'static str = "RTC_PORT_RANGE_END";
const DEFAULT_PLAIN_TRANSPORT_VIDEO_PORT_RANGE_START_CONF_KEY: &'static str = "PLAIN_TRANSPORT_VIDEO_PORT_RANGE_START";
const DEFAULT_PLAIN_TRANSPORT_VIDEO_PORT_RANGE_END_CONF_KEY: &'static str = "PLAIN_TRANSPORT_VIDEO_PORT_RANGE_END";
const DEFAULT_PLAIN_TRANSPORT_AUDIO_PORT_RANGE_START_CONF_KEY: &'static str = "PLAIN_TRANSPORT_AUDIO_PORT_RANGE_START";
const DEFAULT_PLAIN_TRANSPORT_AUDIO_PORT_RANGE_END_CONF_KEY: &'static str = "PLAIN_TRANSPORT_AUDIO_PORT_RANGE_END";

const RTC_LISTEN_IP_CONF_KEY: &'static str = "RTC_LISTEN_IP";

const RTC_MAX_INCOMING_BITRATE_CONF_KEY: &'static str = "RTC_MAX_INCOMING_BITRATE";

const RTC_MIN_OUTGOING_BITRATE_CONF_KEY: &'static str = "RTC_MIN_OUTGOING_BITRATE";

const RTC_MAX_OUTGOING_BITRATE_CONF_KEY: &'static str = "RTC_MAX_OUTGOING_BITRATE";

/// Loading the configuration from the environment variables.
impl SFUConfig {
    /// Create a new `SFUConfig` by loading values from the environment.
    ///
    /// This function uses `common::get_env` to retrieve environment variables
    /// and `parse` to convert them to the appropriate types. In case of
    /// missing or invalid values, it will panic with a descriptive error
    /// message. This is the recommended way to obtain configuration in
    /// production settings (e.g. Docker containers, systemd services).
    ///
    /// # Example
    /// ```
    /// let cfg = crate::mediasoup::SFUConfig::from_env();
    /// println!("SFU will use RTP ports: {}-{}", cfg.rtc_port_range_start, cfg.rtc_port_range_end);
    /// ```
    pub fn from_env() -> Self {
        Self {
            rtc_default_max_participants_per_worker: common::get_env(
                RTC_DEFAULT_MAX_PARTICIPANTS_PER_WORKER_CONF_KEY,
                DEFAULT_MAX_PARTICIPANT_CAPACITY_PER_WORKER, )
                .parse()
                .expect("Invalid RTC_DEFAULT_MAX_PARTICIPANTS_PER_WORKER"),
            rtc_default_max_producers_or_consumers_per_worker: common::get_env(
                RTC_DEFAULT_MAX_PRODUCERS_OR_CONSUMERS_PER_WORKER_CONF_KEY,
                DEFAULT_MAX_PRODUCERS_OR_CONSUMERS_PER_WORKER, )
                .parse()
                .expect("Invalid RTC_DEFAULT_MAX_PRODUCERS_OR_CONSUMERS_PER_WORKER"),
            rtc_port_range_start: common::get_env(
                RTC_PORT_RANGE_START_CONF_KEY,
                DEFAULT_RTC_PORT_RANGE_START, )
                .parse()
                .expect("Invalid RTC_PORT_RANGE_START"),
            rtc_port_range_end: common::get_env(
                RTC_PORT_RANGE_END_CONF_KEY,
                DEFAULT_RTC_PORT_RANGE_END, )
                .parse()
                .expect("Invalid RTC_PORT_RANGE_END"),
            plain_transport_video_port_range_start: common::get_env(
                DEFAULT_PLAIN_TRANSPORT_VIDEO_PORT_RANGE_START_CONF_KEY,
                DEFAULT_PLAIN_TRANSPORT_VIDEO_PORT_RANGE_START, )
                .parse()
                .expect("Invalid PLAIN_TRANSPORT_VIDEO_PORT_RANGE_START"),
            plain_transport_video_port_range_end: common::get_env(
                DEFAULT_PLAIN_TRANSPORT_VIDEO_PORT_RANGE_END_CONF_KEY,
                DEFAULT_PLAIN_TRANSPORT_VIDEO_PORT_RANGE_END, )
                .parse()
                .expect("Invalid PLAIN_TRANSPORT_VIDEO_PORT_RANGE_END"),
            plain_transport_audio_port_range_start: common::get_env(
                DEFAULT_PLAIN_TRANSPORT_AUDIO_PORT_RANGE_START_CONF_KEY,
                DEFAULT_PLAIN_TRANSPORT_AUDIO_PORT_RANGE_START, )
                .parse()
                .expect("Invalid PLAIN_TRANSPORT_AUDIO_PORT_RANGE_START"),
            plain_transport_audio_port_range_end: common::get_env(
                DEFAULT_PLAIN_TRANSPORT_AUDIO_PORT_RANGE_END_CONF_KEY,
                DEFAULT_PLAIN_TRANSPORT_AUDIO_PORT_RANGE_END, )
                .parse()
                .expect("Invalid PLAIN_TRANSPORT_AUDIO_PORT_RANGE_END"),
            rtc_listen_ip: common::get_env(RTC_LISTEN_IP_CONF_KEY, DEFAULT_RTC_LISTEN_IP)
                .parse()
                .expect("Invalid RTC_LISTEN_IP"),
            max_incoming_bitrate: common::get_env(
                RTC_MAX_INCOMING_BITRATE_CONF_KEY,
                DEFAULT_MAX_INCOMING_BITRATE, )
                .parse()
                .expect("Invalid MAX_INCOMING_BITRATE"),
            min_outgoing_bitrate: common::get_env(
                RTC_MIN_OUTGOING_BITRATE_CONF_KEY,
                DEFAULT_MIN_OUTGOING_BITRATE, )
                .parse()
                .expect("Invalid MIN_OUTGOING_BITRATE"),
            max_outgoing_bitrate: common::get_env(
                RTC_MAX_OUTGOING_BITRATE_CONF_KEY,
                DEFAULT_MAX_OUTGOING_BITRATE, )
                .parse()
                .expect("Invalid MAX_OUTGOING_BITRATE"),
            user_event_channel_count: common::get_env(
                "USER_EVENT_CHANNEL_COUNT_NAME",
                DEFAULT_USER_CHANNEL_EVENT_COUNT, )
                .parse()
                .expect("Invalid USER_EVENT_CHANNEL_COUNT_NAME"),
            consumer_shard_count: common::get_env(
                "CONSUMER_SHARD_COUNT_NAME",
                DEFAULT_CONSUMER_SHARD_COUNT, )
                .parse()
                .expect("Invalid CONSUMER_SHARD_COUNT_NAME")
        }
    }
}
