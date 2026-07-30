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
      <!-- Returns: "GRANTED" | "DENIED" | "REQUIRE_2FA" | "TIMEOUT" | "NO_FACE" | "SPOOF" -->
      <arg name="result" type="s" direction="out"/>
      <arg name="distance" type="d" direction="out"/>
      <arg name="tier" type="i" direction="out"/>
    </method>

    <!-- Multi-Stage Interactive Enrollment Session -->
    <method name="StartEnrollment">
      <arg name="username" type="s" direction="in"/>
      <arg name="session_id" type="s" direction="out"/>
    </method>

    <method name="SubmitEnrollmentFrame">
      <arg name="session_id" type="s" direction="in"/>
      <!-- Status: "ACCEPTED" | "FACE_TOO_SMALL" | "MULTIPLE_FACES" | "NO_FACE" | "COMPLETE" -->
      <arg name="status" type="s" direction="out"/>
      <arg name="pose_index" type="i" direction="out"/>
      <arg name="total_poses" type="i" direction="out"/>
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
| `Authenticate` | `yes` (Allow any local user) | Required so GDM and unprivileged PAM invocations can verify faces. |
| `GetStatus` | `yes` | Allows unprivileged status checks via `sentinel status`. |
| `ListUsers` | `yes` | Non-sensitive query for local user listing. |
| `StartEnrollment` | `auth_admin_keep` | Prevents unauthorized users from registering biometric identity templates. |
| `RemoveUser` | `auth_admin` | Requires administrative escalation to delete biometric data. |
| `SetConfig` | `auth_admin` | Administrative change to core thresholds or hardware sources. |
| `GetIntrusionList` | `auth_admin_keep` | Reviewing recorded intrusion attempt screenshots. |

---

## 4. DBus Command Line Debugging Examples

```bash
# Check daemon health status
busctl call com.sentinel.Sentinel /com/sentinel/Sentinel com.sentinel.Sentinel GetStatus

# Trigger test authentication for user 'mehulgolecha'
busctl call com.sentinel.Sentinel /com/sentinel/Sentinel com.sentinel.Sentinel Authenticate s "mehulgolecha"

# Monitor real-time status signals
busctl monitor com.sentinel.Sentinel
```
