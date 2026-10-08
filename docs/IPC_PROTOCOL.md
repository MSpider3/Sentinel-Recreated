# IPC Protocol & DBus Interface Specification — Sentinel Recreated

**Document**: `docs/IPC_PROTOCOL.md`  
**Subsystem**: `sentinel-core/src/dbus/` & `packaging/com.sentinel.policy`

---

## 1. Architectural Justification: System DBus vs Unix Sockets

Legacy iterations attempted custom Unix domain sockets with JSON-RPC messaging. This created significant security and integration hurdles:
- **Permission Fragmentation**: PAM processes execute under varying EUIDs (`root`, `gdm`, or unprivileged users during `sudo`), requiring manual socket `chmod`/`chown` management.
- **Lack of Access Control**: JSON-RPC over raw sockets lacks built-in capability checking.
- **Debugging Overhead**: Standard system monitoring tools (`dbus-monitor`, `busctl`) cannot introspect raw custom socket streams.

**Sentinel Recreated** uses **System DBus** via Rust's high-performance `zbus` crate. DBus provides native security integration via **PolicyKit**, standard system introspection, and strict bus-name ownership semantics.

---

## 2. DBus Interface Contract

- **Bus Name**: `com.sentinel.Sentinel`
- **Object Path**: `/com/sentinel/Sentinel`
- **Interface Name**: `com.sentinel.Sentinel`

```xml
<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<node>
  <interface name="com.sentinel.Sentinel">

    <!-- Primary Authentication Method (Called by PAM module) -->
    <method name="Authenticate">
      <arg name="username" type="s" direction="in"/>
      <!-- Caller's SSH_CLIENT / SSH_TTY, if set (advisory; logind is the authority) -->
      <arg name="session_env" type="a{ss}" direction="in"/>
      <!-- Returns: "GRANTED" | "DENIED" | "SPOOF" | "TIMEOUT" | "NO_FACE" | "RATE_LIMITED" -->
      <arg name="result" type="s" direction="out"/>
      <!-- Constant per result (0.0 granted, -1.0 no scan, 1.0 otherwise): the real
           match distance is never returned, it would let callers probe a template -->
      <arg name="distance" type="d" direction="out"/>
      <!-- 1 = strong match, 2 = normal match, 4 = not granted, 0 = no scan -->
      <arg name="tier" type="i" direction="out"/>
    </method>

    <!-- Multi-Stage Interactive Enrollment Session -->
    <method name="StartEnrollment">
      <arg name="username" type="s" direction="in"/>
      <arg name="session_id" type="s" direction="out"/>
    </method>

    <!-- The wizard owns the camera and sends JPEG/PNG frames.
         capture=false: inspect only (live preview). capture=true: store as a template. -->
    <method name="SubmitEnrollmentFrameData">
      <arg name="session_id" type="s" direction="in"/>
      <arg name="frame_data" type="ay" direction="in"/>
      <arg name="capture" type="b" direction="in"/>
      <!-- Status: "ACCEPTED" | "NO_FACE" | "MULTIPLE_FACES" | "OUT_OF_FRAME" | "NOT_FRONTAL" |
           "TOO_DARK" | "TOO_BRIGHT" | "BLURRY" | "TOO_SIMILAR" | "FULL" | "DECODE_ERROR" | "NO_SESSION" -->
      <arg name="status" type="s" direction="out"/>
      <arg name="templates_collected" type="i" direction="out"/>
      <arg name="templates_max" type="i" direction="out"/>
      <!-- [bbox x1,y1,x2,y2, then 5 landmarks x,y] of the detected face, or empty -->
      <arg name="bbox_and_landmarks" type="ad" direction="out"/>
    </method>

    <method name="FinishEnrollment">
      <arg name="session_id" type="s" direction="in"/>
      <arg name="success" type="b" direction="out"/>
      <arg name="message" type="s" direction="out"/>
    </method>

    <method name="CancelEnrollment">
      <arg name="session_id" type="s" direction="in"/>
    </method>

    <!-- System & User Administration -->
    <method name="ListUsers">
      <arg name="users" type="as" direction="out"/>
    </method>

    <method name="RemoveUser">
      <arg name="username" type="s" direction="in"/>
      <arg name="success" type="b" direction="out"/>
    </method>

    <method name="GetConfig">
      <arg name="config_toml" type="s" direction="out"/>
    </method>

    <method name="SetConfig">
      <arg name="config_toml" type="s" direction="in"/>
      <arg name="success" type="b" direction="out"/>
      <arg name="message" type="s" direction="out"/>
    </method>

    <method name="GetStatus">
      <!-- Returns JSON string describing daemon state, uptime, models loaded -->
      <arg name="status_json" type="s" direction="out"/>
    </method>

    <method name="GetIntrusionList">
      <arg name="filenames" type="as" direction="out"/>
    </method>

    <method name="DismissIntrusion">
      <arg name="filename" type="s" direction="in"/>
    </method>

    <method name="GetUserInfo">
      <arg name="username" type="s" direction="in"/>
      <!-- JSON: username, core_vector_count, adaptive_vector_count, last_adaptation_date, enrolled_at -->
      <arg name="info_json" type="s" direction="out"/>
    </method>

    <!-- Newest audit lines (at most 1000), oldest first, read across the daily log files -->
    <method name="GetRecentAuthLog">
      <arg name="lines" type="u" direction="in"/>
      <arg name="log_lines" type="as" direction="out"/>
    </method>

    <method name="GetGreeterInfo">
      <arg name="info_json" type="s" direction="out"/>
    </method>

    <!-- Real-time Event Signals -->
    <signal name="AuthStatusChanged">
      <arg name="status" type="s"/>
      <arg name="message" type="s"/>
    </signal>

  </interface>
</node>
```

---

## 3. PolicyKit Privilege Management Rules

File: `packaging/com.sentinel.policy`

| Method / Action | Policy Rule (`auth_admin` / `yes`) | Justification |
|---|---|---|
| `Authenticate` | no PolicyKit; caller must be root or the target user | Required so PAM invocations (root for login/sudo, the user for lock screens) can verify faces, while nobody can probe another user's template. |
| `GetStatus` | `yes` | Allows unprivileged status checks via `sentinel status`. |
| `ListUsers` | `yes` | Non-sensitive query for local user listing. |
| `StartEnrollment` | `auth_admin_keep` | Prevents unauthorized users from registering biometric identity templates. |
| `RemoveUser` | `auth_admin` | Requires administrative escalation to delete biometric data. |
| `SetConfig` | `auth_admin` | Administrative change to core thresholds or hardware sources. Values are range-checked before they are saved. |
| `GetIntrusionList` | `auth_admin_keep` | Reviewing recorded intrusion attempt screenshots. |

---

## 4. DBus Command Line Debugging Examples

```bash
# Check daemon health status
busctl call com.sentinel.Sentinel /com/sentinel/Sentinel com.sentinel.Sentinel GetStatus

# Trigger test authentication for user '$USER'
busctl call com.sentinel.Sentinel /com/sentinel/Sentinel com.sentinel.Sentinel Authenticate "sa{ss}" "$USER" 0

# Monitor real-time status signals
busctl monitor com.sentinel.Sentinel
```
