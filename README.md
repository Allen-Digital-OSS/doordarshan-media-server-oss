# Doordarshan Media Server (OSS)

**doordarshan-media-server-oss** is an open-source WebRTC SFU and media processing server built on [mediasoup](https://mediasoup.org/), providing real-time audio/video streaming with integrated server-side recording via GStreamer pipelines. It is designed for scalable, production-grade live streaming systems.

This project focuses on reliability in real-world conditions such as long-running sessions, audio/video stalls, synchronization issues, and continuous recording.

---

## ✨ Features

- WebRTC SFU powered by mediasoup
- Real-time audio/video routing
- Server-side recording using GStreamer
- Supports long-running live streams
- Handles A/V stalls with silence / black-frame injection
- Designed for horizontal scaling
- Production-oriented failure handling
- Multi-tenancy support for isolated workloads [MultiTenancy](docs/tenant.md)

---
## Each media server instance:

- Accepts WebRTC producers/consumers
- Routes RTP streams internally via mediasoup
- Feeds audio/video into GStreamer pipelines
- Generates continuous recordings
- Handles missing audio/video by injecting silence or black frames

The server is designed to work alongside [doordarshan-kendra](https://github.com/Allen-Digital-OSS/doordarshan-kendra-oss), which acts as the orchestration and control plane.

---
## 🚀 Use Cases

- Meetings and real-time collaboration
- Large-scale broadcasts
- Interactive streaming platforms
- Real-time conferencing with server-side recording
- Media ingestion pipelines
---
## 📦 Tech Stack

- WebRTC
- mediasoup
- GStreamer
- Rust
- MySQL

---

### Clone the repository

```bash
git clone git@github.com:Allen-Digital-OSS/doordarshan-media-server-oss.git
cd doordarshan-media-server-oss
```
### 🔧 Local Development
Setup and run instructions are available here:

👉 [Local Development Guide](docs/local-setup.md)

### Remote Deployment
Deployment instructions are available here:

👉 <WIP> [Remote Deployment Guide](docs/remote-deployment.md)

## 📖 API Documentation (Swagger UI)
API documentation is available at: http://localhost:3001/docs/#/

---
## 📚 GStreamer Recording Pipeline Series
This media server’s recording architecture is based on our in-depth GStreamer pipeline exploration series:

### Implemented in this repository

- **Part 0 – Architecture & Overview**
  👉 https://medium.com/@allen-blogs/part-0-in-house-webrtc-conferencing-platform-ec5502be9114

- **Part 1 – One Pipeline Per Producer**
  👉 https://medium.com/@allen-blogs/part-1-one-pipeline-per-producer-6174b380f984

- **Part 2 – Combining Audio and Video in a Single Pipeline**
  👉 https://medium.com/@allen-blogs/part-2-combining-audio-and-video-in-a-single-pipeline-3fc2bc6a70cc

- **Part 3 – Multi-Audio / Multi-Video Pipeline**
  👉 https://medium.com/@allen-blogs/part-3-pipeline-for-multi-audio-multi-video-9b969c977323

### Experimental / POC

- **Part 4 – Pipeline with Test Sources**
  (Proof-of-concept implemented but not merged to `main`)
  👉 https://medium.com/@allen-blogs/part-4-pipeline-with-video-audio-test-src-8617345a2629

### Bonus – Experiments & Lessons

- 👉 https://medium.com/@allen-blogs/bonus-gstreamer-pipeline-experiments-and-lessons-ae2fbe096493
---
## 🤝 Contributing
We follow a fork-based contribution model.

Workflow:
1.	Fork the repository
2.	Create a feature branch in your fork
3.	Make your changes
4.	Run pre-commit hooks
5.	Open a Pull Request against main

Direct pushes to the main repository are disabled.

## 📄 License
This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.

## Acknowledgements
Built with:
- [mediasoup](https://mediasoup.org/)
- [GStreamer](https://gstreamer.freedesktop.org/)
- AI tools
