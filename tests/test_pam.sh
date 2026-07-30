#!/usr/bin/env bash
# Tests PAM integration without actually changing sudo behavior
# Uses pamtester if available, falls back to manual instructions

echo "=== PAM Integration Tests ==="

# Test 1: Daemon running — face auth path active
systemctl is-active sentinel || { echo "SKIP: Daemon not running"; exit 1; }
echo "Test 1: Daemon active — PASS"

# Test 2: PAM config contains sentinel line
grep -q "pam_sentinel" /etc/pam.d/sudo && echo "Test 2: PAM configured — PASS" || echo "Test 2: FAIL"

# Test 3: pam_sentinel.so exists and is a valid shared library
file /usr/lib64/security/pam_sentinel.so | grep -q "shared object" && \
    echo "Test 3: PAM module valid ELF — PASS" || echo "Test 3: FAIL"

# Test 4: Daemon responds to Authenticate over DBus
TARGET_USER="${SUDO_USER:-$USER}"
RESULT=$(busctl call com.sentinel.Sentinel /com/sentinel/Sentinel \
    com.sentinel.Sentinel Authenticate "sa{ss}" "$TARGET_USER" 0 2>&1)
echo "Test 4: DBus Authenticate response: $RESULT"

# Test 5: Daemon stopped — fallback to password (manual verification)
echo ""
echo "Test 5 (manual): Stop daemon, run 'sudo echo test', confirm password prompt appears"
echo "  sudo systemctl stop sentinel"
echo "  sudo echo test   ← should ask for password"
echo "  sudo systemctl start sentinel"
