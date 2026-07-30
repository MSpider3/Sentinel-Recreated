#!/usr/bin/env bash
set -euo pipefail
MODEL_DIR="/var/cache/sentinel/models"
sudo mkdir -p "$MODEL_DIR"
sudo chown root:root "$MODEL_DIR"
sudo chmod 755 "$MODEL_DIR"

echo "=== Sentinel Recreated Model Downloader ==="
echo "Target directory: $MODEL_DIR"

TMP=$(mktemp -d)
echo "Downloading SCRFD-500M & MobileFaceNet (InsightFace buffalo_sc)..."
wget --show-progress \
  "https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_sc.zip" \
  -O "$TMP/buffalo_sc.zip"
unzip -q "$TMP/buffalo_sc.zip" -d "$TMP/buffalo_sc"

sudo cp "$TMP/buffalo_sc/det_500m.onnx"    "$MODEL_DIR/scrfd_500m_kps.onnx"
sudo cp "$TMP/buffalo_sc/w600k_mbf.onnx"   "$MODEL_DIR/mobile_facenet.onnx"
sudo chmod 644 "$MODEL_DIR/"*.onnx
rm -rf "$TMP"

echo ""
echo "Downloading MiniFASNetV2 anti-spoof model..."
MINIFAS_TMP=$(mktemp -d)
# Source: https://github.com/yakhyo/face-anti-spoofing (same binary used by the Python prototype)
wget --show-progress \
  "https://github.com/yakhyo/face-anti-spoofing/releases/download/weights/MiniFASNetV2.onnx" \
  -O "$MINIFAS_TMP/MiniFASNetV2.onnx"
sudo cp "$MINIFAS_TMP/MiniFASNetV2.onnx" "$MODEL_DIR/MiniFASNetV2.onnx"
sudo chmod 644 "$MODEL_DIR/MiniFASNetV2.onnx"
rm -rf "$MINIFAS_TMP"

echo ""
echo "Model download completed successfully!"
ls -lh "$MODEL_DIR"
