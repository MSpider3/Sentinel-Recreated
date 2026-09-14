# General-Purpose Sentinel Tests

These test scripts are designed for automated or general manual validation of the Sentinel daemon and client libraries.

### PAM Module Integration Test
```bash
./test_pam.sh
```
Verifies PAM configuration in `/etc/pam.d/sudo`, checks `pam_sentinel.so` validity, and tests DBus Ping/Authenticate.

### Daemon Cold-Start Benchmark
```bash
sudo ./bench_coldstart.sh
```
Measures time from `systemctl start sentinel` until the DBus service responds to `GetStatus`.

### Face Recognition Accuracy Benchmark
```bash
python3 test_recognition.py [--user <username>]
```
Interactive accuracy test measuring recognition distance, lighting variation, and threshold recommendations. Automatically defaults to the current active user.

### Anti-Spoofing Benchmark
```bash
python3 test_spoof.py [--user <username>]
```
Validates MiniFASNet anti-spoof rejection against live faces and 2D photo attacks. Automatically defaults to the current active user.
