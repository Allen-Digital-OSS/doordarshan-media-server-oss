### doordarshan-media-server

# Steps to run in local
1. Setup Mysql Server
2. Create user root without password
3. Create database doordarshan
4. Run create_tables.sql
5. Install Rust
6. Install gstreamer and set env variables as well  `https://gstreamer.freedesktop.org/documentation/installing/index.html`
Sample : `export PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/glib/lib/pkgconfig:/opt/homebrew/opt/gstreamer/lib/pkgconfig:/opt/homebrew/opt/libffi/lib/pkgconfig:/opt/homebrew/opt/gobject-introspection/lib/pkgconfig`
7. create local.env file inside config directory, refer the `Local Environment Variables` section
8. Run cargo run
```[2025-12-15 16:35:21] [None] [INFO] Using local environment, logging to console
[2025-12-15 16:35:21] [None] [INFO]
    ____                           __                    __
   / __ \ ____   ____   _____ ____/ /____ _ _____ _____ / /_   ____ _ ____
  / / / // __ \ / __ \ / ___// __  // __ `// ___// ___// __ \ / __ `// __ \
 / /_/ // /_/ // /_/ // /   / /_/ // /_/ // /   (__  )/ / / // /_/ // / / /
/_____/ \____/ \____//_/    \__,_/ \__,_//_/   /____//_/ /_/ \__,_//_/ /_/
    __  ___           __ _           _____
   /  |/  /___   ____/ /(_)____ _   / ___/ ___   _____ _   __ ___   _____
  / /|_/ // _ \ / __  // // __ `/   \__ \ / _ \ / ___/| | / // _ \ / ___/
 / /  / //  __// /_/ // // /_/ /   ___/ //  __// /    | |/ //  __// /
/_/  /_/ \___/ \__,_//_/ \__,_/   /____/ \___//_/     |___/ \___//_/

[2025-12-15 16:35:21] [None] [INFO] Starting Doordarshan MediaServer
[2025-12-15 16:35:21] [None] [INFO] Environment: "local"
[2025-12-15 16:35:21] [None] [INFO] Elastic IP: 127.0.0.1
[2025-12-15 16:35:21] [None] [INFO] Private IP: 127.0.0.1
[2025-12-15 16:35:21] [None] [INFO] MYSQL_CREDS_PATH not provided, defaulting to
[2025-12-15 16:35:21] [None] [INFO] Connecting to Local MySQL
[2025-12-15 16:35:21] [None] [INFO] OTEL_ENDPOINT not provided, defaulting to localhost:4318
[2025-12-15 16:35:21] [None] [INFO] RTC_DEFAULT_MAX_PARTICIPANTS_PER_WORKER not provided, defaulting to 240
[2025-12-15 16:35:21] [None] [INFO] RTC_DEFAULT_MAX_PRODUCERS_OR_CONSUMERS_PER_WORKER not provided, defaulting to 480
[2025-12-15 16:35:21] [None] [INFO] PLAIN_TRANSPORT_VIDEO_PORT_RANGE_START not provided, defaulting to 50000
[2025-12-15 16:35:21] [None] [INFO] PLAIN_TRANSPORT_VIDEO_PORT_RANGE_END not provided, defaulting to 55000
[2025-12-15 16:35:21] [None] [INFO] PLAIN_TRANSPORT_AUDIO_PORT_RANGE_START not provided, defaulting to 60000
[2025-12-15 16:35:21] [None] [INFO] PLAIN_TRANSPORT_AUDIO_PORT_RANGE_END not provided, defaulting to 65000
[2025-12-15 16:35:21] [None] [INFO] RTC_MAX_INCOMING_BITRATE not provided, defaulting to 10000000
[2025-12-15 16:35:21] [None] [INFO] RTC_MIN_OUTGOING_BITRATE not provided, defaulting to 100000
[2025-12-15 16:35:21] [None] [INFO] RTC_MAX_OUTGOING_BITRATE not provided, defaulting to 10000000
[2025-12-15 16:35:21] [None] [INFO] USER_EVENT_CHANNEL_COUNT_NAME not provided, defaulting to 100
[2025-12-15 16:35:21] [None] [INFO] Initialising Mediasoup SFU
[2025-12-15 16:35:21] [None] [INFO] Instance configuration and computed settings | Instance Cores: 11
[2025-12-15 16:35:21] [None] [INFO] Creating worker # 1
[2025-12-15 16:35:21] [None] [INFO] Creating worker # 2
[2025-12-15 16:35:21] [None] [INFO] Insert string is INSERT INTO container_heartbeat (container_id, heartbeat, host, tenant) VALUES ('127.0.0.1-1765796721546031000',1765796721546031000,'127.0.0.1','MvN')
[2025-12-15 16:35:21] [None] [INFO] Insert string is INSERT INTO worker_container (container_id, worker_id) VALUES ('127.0.0.1-1765796721546031000','973b5077-2dae-441f-a43e-d0b98c24e5c6')
[2025-12-15 16:35:21] [None] [INFO] Insert string is INSERT INTO worker_container (container_id, worker_id) VALUES ('127.0.0.1-1765796721546031000','8fea8756-a5f0-477a-9d49-9627b2525078')
[2025-12-15 16:35:21] [None] [INFO] Success in writing to DB
[2025-12-15 16:35:21] [None] [INFO] Media Server started!
[2025-12-15 16:35:21] [None] [INFO] Listening on 0.0.0.0:3001
```

#Swagger UI
http://localhost:3001/docs/#/
