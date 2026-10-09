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
Interactive test: self recognition, sitting distance, lighting, glasses, and a different person. Shows the real match distance and anti-spoof score of every scan (read from the audit log). Automatically defaults to the current active user.

### Anti-Spoofing Benchmark
```bash
python3 test_spoof.py [--user <username>]
```
Five scans of your real face, then five of a photo or phone video **of you** held to the camera. Fails if any photo attempt is granted. Automatically defaults to the current active user.

### Dashboard Test
```bash
python3 test_tui.py
```
Drives every dashboard screen headlessly against a stand-in daemon (no daemon, camera or display needed): log, users, intrusions, loading and saving settings, error messages.

### Exploratory DBus Checks
```bash
python3 test_exploratory_qa.py
```
Calls the running daemon with invalid usernames, file names and session ids to check they are rejected.
