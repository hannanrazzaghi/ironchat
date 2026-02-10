# ironchat

A minimal, production-quality encrypted chatroom in Rust.

## Install

### For developers

```bash
cargo install --git https://github.com/hannanrazzaghi/ironchat.git
```

### For server deployment

See [DEPLOY.md](DEPLOY.md) for complete step-by-step server setup with Let's Encrypt, systemd, and client distribution.

## TLS certificates

Generate a self-signed cert (for local dev):

```bash
openssl req -x509 -newkey rsa:4096 -keyout key.pem -out cert.pem -days 365 -nodes -subj "/CN=ironchat"
```

## Run server

```bash
chatd --bind 0.0.0.0:5555 --cert ./cert.pem --key ./key.pem
```

Allowlist and pending files (defaults in current directory):

- allowed.toml
- pending.toml
- identities.toml

Example allowlist:

```toml
allow = ["127.0.0.1"]
```

### Admin commands

```bash
chatd allow add <ip-or-cidr>
chatd allow remove <ip-or-cidr>
chatd allow list
chatd pending list
chatd pending remove <ip>
chatd pending clear
```

## Run client

```bash
chatctl --connect 127.0.0.1:5555 --ca ./cert.pem
```

## Install client (single binary download)

Host a compiled `chatctl` binary on your server, then let users download it directly.

Server-side (build and publish the binary):

```bash
cargo build --release -p chatctl
sudo mkdir -p /var/www/ironchat
sudo cp target/release/chatctl /var/www/ironchat/chatctl
sudo chmod 755 /var/www/ironchat/chatctl
```

Client-side (download and run):

```bash
curl -LO https://yourdomain.com/chatctl
chmod +x chatctl
./chatctl --connect yourdomain.com:5555 --ca ./cert.pem
```

Notes:

- If you use a public TLS cert for the server, you can copy the server cert chain to `./cert.pem` for clients, or provide it alongside the binary.
- If you keep `chatctl` under a different path (like `/downloads/chatctl`), update the URL accordingly.

Development-only (unsafe):

```bash
chatctl --connect 127.0.0.1:5555 --insecure
```

## Client commands

- `/help`
- `/nick <name>`
- `/who`
- `/quit`

## Allowlist and pending behavior

Connections from unknown IPs are rejected with `Not approved. Ask admin.` and written to pending.toml.
Update allowed.toml and connections are re-evaluated on each new connection.

## Identity persistence

Each IP maps to a last known nickname in identities.toml. This is atomic and cleaned for duplicate nicknames.

**NAT caveat:** multiple users behind one NAT will share the same IP identity.

## Optional Redis mode

Enable Redis at runtime with `--redis redis://...` and compile with the redis feature:

```bash
cargo build --features redis
chatd --bind 0.0.0.0:5555 --cert ./cert.pem --key ./key.pem --redis redis://127.0.0.1/
```

Redis stores identities and message history. Allowlist/pending remain file-based.

## TLS smoke test

```bash
openssl s_client -connect 127.0.0.1:5555 -CAfile cert.pem
```

## Logging

Use `RUST_LOG=info` to enable logs:

```bash
RUST_LOG=info chatd --bind 0.0.0.0:5555 --cert ./cert.pem --key ./key.pem
```
