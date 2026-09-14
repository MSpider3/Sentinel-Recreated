# Sentinel Test Suites

This directory is organized into two separate environments:

## 1. `tests/general/` (Public / Repository Tracked)
Contains general-purpose validation scripts for CI, packaging verification, and general biometric testing:
- **`bench_coldstart.sh`**: Measures daemon startup time against the 5000ms cold-start target.
- **`test_pam.sh`**: Validates PAM configuration, `pam_sentinel.so` shared library integrity, and DBus connectivity.
- **`test_recognition.py`**: Benchmark test for facial recognition accuracy and distance sensitivity across dynamic users.
- **`test_spoof.py`**: Benchmark test for anti-spoofing verification against photo attacks.

## 2. `tests/personal/` (Local / Git-Ignored)
Intended strictly for local, device-specific testing on the developer's hardware.
- All files in `tests/personal/` are ignored by Git to ensure personal biometric test data, camera logs, and device reports are never uploaded to the public repository.
- Contains local test outputs, camera calibration runs, and benchmark reports (`recognition_report_*.txt`, `spoof_report_*.txt`).
