# ByteChat

ByteChat is a **terminal-based (TUI) chat application**. It features a client-server architecture where users connect over TCP/TLS, authenticate, and communicate in shared rooms or private DMs, with all of it being rendered inside the terminal with colorful animated ASCII art.

## Demo

<video src="readme\Demo.mp4" width="320" height="240" controls></video>

## Features

- **TUI Client** — Four-pane layout (sidebar, messages, input, and anim area) with mouse support, focus cycling, and pulsing focus indicators
- **Multi-Room Chat** — Create, join, and leave rooms; room membership persisted in PostgreSQL with `/join` or `/leave`
- **Private Messages** — Direct message any user with `/dm`
- **Image Sharing** — Upload and display images inline with `/image <path>`
- **Audio Notes** — Record microphone input, upload audio, and play back with live FFT spectrum visualization by holding the `grave accent-key`, when focused on message area to record or typing `/audio <path>` to send an audio file
- **Themes** — 4 color themes (Amber, Matrix, Synthwave, Solarized) switchable with `Ctrl+T`
- **Animations** — 5 sidebar animations (3D Cube, Matrix Rain, Starfield, Torus, Sand Simulation) switchable with `Ctrl+A`
- **Typing Indicators** — Real-time "user is typing..." notifications via Redis
- **Read Receipts** — Track which messages you've seen across rooms
- **Admin Commands** — `/mute`, `/unmute`, `/ban`, `/unban` with audit logging
- **TLS Support** — Optional encryption with auto-generated self-signed certificates
- **Docker Compose** — One-command deployment with PostgreSQL and Redis

## Why This Was Made
I'm still really new at Rust, and I want to learn, so I decided to challenge myself by making a cool project cause the only way to learn is by doing. I learned a lot of new concepts, and I'm enjoying the language so far. 

## Getting Started

### Prerequisites

- Rust (edition 2024)
- Docker and Docker Compose (for PostgreSQL + Redis)

### Setup

1.  (Optional) Generate TLS certificates:
    ```sh
    cargo run --bin init-tls
    ```

2. Start the database services:
   ```sh
   docker compose up -d
   ```

3. In another terminal, start the client:
   ```sh
   cargo run --bin client <name>
   ```

### Configuration

Environment variables:

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | `postgres://chat:chat@localhost:5432/chat` | PostgreSQL connection |
| `REDIS_URL` | `redis://127.0.0.1:6379` | Redis connection |
| `BIND_ADDR` | `0.0.0.0:8082` | Chat server address |
| `UPLOAD_BIND_ADDR` | `0.0.0.0:8083` | Upload server address |
| `UPLOAD_URL` | `http://localhost:8083` | Public upload URL |
| `NO_TLS` | unset | Set to disable TLS |
| `TLS_CERT` | `cert.pem` | TLS certificate path |
| `TLS_KEY` | `key.pem` | TLS key path |

## License
This Project uses an MIT License.