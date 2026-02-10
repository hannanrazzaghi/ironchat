# Server Deployment Guide

Complete steps to deploy `ironchat` on your own server with a public TLS certificate and easy client distribution.

---

## Prerequisites

- Ubuntu/Debian server with public IP
- Domain name pointing to your server (e.g., `chat.yourdomain.com`)
- SSH access to the server
- Port 80 (for Let's Encrypt) and port 5555 (for chatd) available

---

## Step 1: Install Rust on the server

SSH into your server:

```bash
ssh user@chat.yourdomain.com
```

Install Rust:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

Verify:

```bash
cargo --version
```

---

## Step 2: Clone and build the project

```bash
git clone <your-repo-url> ironchat
cd ironchat
cargo build --release
```

Binaries will be at:
- `target/release/chatd` (server)
- `target/release/chatctl` (client)

---

## Step 3: Get a TLS certificate with Let's Encrypt

Install certbot:

```bash
sudo apt update
sudo apt install -y certbot
```

Obtain a certificate (certbot will temporarily bind port 80):

```bash
sudo certbot certonly --standalone -d chat.yourdomain.com
```

Follow the prompts. Certificates will be saved at:
- Cert: `/etc/letsencrypt/live/chat.yourdomain.com/fullchain.pem`
- Key: `/etc/letsencrypt/live/chat.yourdomain.com/privkey.pem`

Set up auto-renewal:

```bash
sudo systemctl enable certbot.timer
sudo systemctl start certbot.timer
```

---

## Step 4: Configure the firewall

Allow port 5555 (chatd):

```bash
sudo ufw allow 5555/tcp
sudo ufw enable
```

If you need HTTP/HTTPS for serving the binary:

```bash
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp
```

---

## Step 5: Create config files

Create allowlist, pending, and identities files in the project directory:

```bash
cd ~/ironchat
cat > allowed.toml <<EOF
allow = ["0.0.0.0/0"]
EOF

touch pending.toml identities.toml
```

*(Adjust `allowed.toml` to restrict by IP/CIDR as needed.)*

---

## Step 6: Run the server

Test run:

```bash
./target/release/chatd \
  --bind 0.0.0.0:5555 \
  --cert /etc/letsencrypt/live/chat.yourdomain.com/fullchain.pem \
  --key /etc/letsencrypt/live/chat.yourdomain.com/privkey.pem
```

Press Ctrl+C to stop after verifying it works.

---

## Step 7: Set up chatd as a systemd service (optional but recommended)

Create a service file:

```bash
sudo nano /etc/systemd/system/chatd.service
```

Paste:

```ini
[Unit]
Description=IronChat Server
After=network.target

[Service]
Type=simple
User=youruser
WorkingDirectory=/home/youruser/ironchat
ExecStart=/home/youruser/ironchat/target/release/chatd \
  --bind 0.0.0.0:5555 \
  --cert /etc/letsencrypt/live/chat.yourdomain.com/fullchain.pem \
  --key /etc/letsencrypt/live/chat.yourdomain.com/privkey.pem
Restart=on-failure
Environment="RUST_LOG=info"

[Install]
WantedBy=multi-user.target
```

Replace `youruser` with your actual username.

Enable and start the service:

```bash
sudo systemctl daemon-reload
sudo systemctl enable chatd
sudo systemctl start chatd
sudo systemctl status chatd
```

View logs:

```bash
sudo journalctl -u chatd -f
```

---

## Step 8: Publish the client binary for download

Set up a web directory:

```bash
sudo apt install -y nginx
sudo mkdir -p /var/www/ironchat
sudo cp target/release/chatctl /var/www/ironchat/chatctl
sudo chmod 755 /var/www/ironchat/chatctl
```

Copy the cert so clients can download it:

```bash
sudo cp /etc/letsencrypt/live/chat.yourdomain.com/fullchain.pem /var/www/ironchat/cert.pem
sudo chmod 644 /var/www/ironchat/cert.pem
```

Configure nginx to serve the files:

```bash
sudo nano /etc/nginx/sites-available/ironchat
```

Paste:

```nginx
server {
    listen 80;
    server_name chat.yourdomain.com;

    location /downloads/ {
        alias /var/www/ironchat/;
        autoindex on;
    }
}
```

Enable the site:

```bash
sudo ln -s /etc/nginx/sites-available/ironchat /etc/nginx/sites-enabled/
sudo nginx -t
sudo systemctl reload nginx
```

---

## Step 9: Test the setup

From another machine:

```bash
curl -LO http://chat.yourdomain.com/downloads/chatctl
curl -LO http://chat.yourdomain.com/downloads/cert.pem
chmod +x chatctl
./chatctl --connect chat.yourdomain.com:5555 --ca ./cert.pem
```

If it connects, you're done!

---

## Step 10: Share instructions with your friends

Send them:

```bash
curl -LO http://chat.yourdomain.com/downloads/chatctl
curl -LO http://chat.yourdomain.com/downloads/cert.pem
chmod +x chatctl
./chatctl --connect chat.yourdomain.com:5555 --ca ./cert.pem
```

Or simplify with an install script (see next section).

---

## Optional: Create an install script for clients

Create `install-chatctl.sh` on your server:

```bash
nano ~/ironchat/install-chatctl.sh
```

Paste:

```bash
#!/bin/bash
set -e

DOMAIN="chat.yourdomain.com"
BASE_URL="http://$DOMAIN/downloads"

echo "Downloading chatctl..."
curl -LO $BASE_URL/chatctl
chmod +x chatctl

echo "Downloading cert..."
curl -LO $BASE_URL/cert.pem

echo "Done! Run:"
echo "./chatctl --connect $DOMAIN:5555 --ca ./cert.pem"
```

Publish it:

```bash
sudo cp install-chatctl.sh /var/www/ironchat/install-chatctl.sh
sudo chmod 755 /var/www/ironchat/install-chatctl.sh
```

Now clients can run:

```bash
curl -sSL http://chat.yourdomain.com/downloads/install-chatctl.sh | bash
```

---

## Admin commands

Once the server is running, manage the allowlist:

```bash
./target/release/chatd allow add 203.0.113.5
./target/release/chatd allow remove 203.0.113.5
./target/release/chatd allow list
./target/release/chatd pending list
./target/release/chatd pending clear
```

---

## Maintenance

### Update the server

```bash
cd ~/ironchat
git pull
cargo build --release
sudo systemctl restart chatd
```

### Renew TLS cert

Certbot auto-renews. Test renewal:

```bash
sudo certbot renew --dry-run
```

After renewal, restart chatd:

```bash
sudo systemctl restart chatd
```

---

## Troubleshooting

### Connection refused
- Check firewall: `sudo ufw status`
- Check chatd is running: `sudo systemctl status chatd`

### Certificate errors
- Ensure DNS points to your server IP
- Verify cert exists: `sudo ls /etc/letsencrypt/live/chat.yourdomain.com/`

### Logs
```bash
sudo journalctl -u chatd -f
```

---

You're all set! Your users can now download and run the client with zero local build steps.
