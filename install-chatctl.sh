#!/bin/bash
# Client install script for ironchat
set -e

DOMAIN="chat.yourdomain.com"
BASE_URL="http://$DOMAIN/downloads"

echo "🔧 Downloading chatctl..."
curl -LO $BASE_URL/chatctl
chmod +x chatctl

echo "🔒 Downloading certificate..."
curl -LO $BASE_URL/cert.pem

echo "✅ Done! Run:"
echo "./chatctl --connect $DOMAIN:5555 --ca ./cert.pem"
