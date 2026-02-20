# 📦 Doordarshan Media Server – Remote Deployment Guide

This guide explains how to:

1. Build the base Docker image from the public repository
2. Create a private deployment repository
3. Inject environment-specific configuration
4. Build and run environment-based containers
5. For the container setup please refer to [local-setup.md](docs/local-setup.md) which has details about environment variables and their significance.

---

# Repository Overview

- Public repository → application code only
- Private repository → environment configs
- Config naming convention → `{ENV}.env`

---

# 1. Build Base Image (From Public Repository)

From the root of the public repository (where the main Dockerfile exists):

```
docker build -t doordarshan-media-server:base .
```
(Optional: Push to Docker Hub)
```
docker tag doordarshan-media-server:base <your-dockerhub-username>/doordarshan-media-server:base
docker push <your-dockerhub-username>/doordarshan-media-server:base
```

# 2. Create Private Deployment Repository

```text
.
├── Dockerfile
├── run.sh
└── config/
    ├── dev.env
    ├── staging.env
    └── prod.env
```

## Environment Selection
```
The container selects the configuration file based on the `ENV` environment variable.

It must match a file inside the `config/` directory:
If `ENV` is not provided, it defaults to: config/local.env
Example mappings:

| ENV  | Config File         |
|-----------|--------------------|
| dev       | config/dev.env     |
| staging   | config/staging.env |
| prod      | config/prod.env    |

⚠ Make sure the correct `ENV` is passed during container runtime.
```

# 3. Private Dockerfile
```
FROM <your-dockerhub-username>/doordarshan-media-server:base

WORKDIR /app

COPY ./config /config
COPY run.sh /app/run.sh

ENTRYPOINT ["sh", "run.sh"]
```

## Sample Env File (config/dev.env)
```
RUST_BACKTRACE="full"

SERVER_PORT=3001

LOG_LEVEL="info"
LOG_PATH="doordarshan-media-server.log"

RTC_PORT_RANGE_START=40000
RTC_PORT_RANGE_END=45000
RTC_LISTEN_IP="127.0.0.1"
RTC_ANNOUNCED_IP="127.0.0.1"

MYSQL_HOST="127.0.0.1:3306"
MYSQL_USER="root"
MYSQL_PASSWORD=""
MYSQL_DATABASE="doordarshan"
TENANT=MvN

CLIENT_CONFIG_SERVICE_ENDPOINT="https://api.example.com/v1/events/dummy-session-events/publish"
CLIENT_CONFIG_SERVICE_HEADERS={"user-agent":"doordarshan-dev/1.0 (contact: dev@example.com)"}
```
