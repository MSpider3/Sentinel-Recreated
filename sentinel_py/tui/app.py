import getpass
import json
import os
import re
import subprocess
import tempfile
import tomllib
from datetime import datetime, timezone
from textual.app import App, ComposeResult
from textual.screen import Screen, ModalScreen
from textual.widgets import Header, Footer, DataTable, Static, Button, Input, Label
from textual.containers import Container, Horizontal, Vertical, VerticalScroll
from sentinel_py import __version__
from sentinel_py.dbus_client import SentinelDBusClient

INTRUSION_DIR = "/var/lib/sentinel/blacklist"


def current_user() -> str:
    return os.environ.get("SUDO_USER") or os.environ.get("USER") or getpass.getuser()


def error_text(e: Exception) -> str:
    """The useful part of a DBus error, without the 'org.freedesktop...:' prefix."""
    text = str(e)
    return text.split(": ", 1)[1] if text.startswith("org.") and ": " in text else text


def local_time(utc_stamp: str) -> str:
    """Audit timestamps are UTC ('2026-07-21T14:32:10.104Z'); show local time."""
    try:
        when = datetime.strptime(utc_stamp, "%Y-%m-%dT%H:%M:%S.%fZ").replace(tzinfo=timezone.utc).astimezone()
    except ValueError:
        return utc_stamp
    fmt = "%H:%M:%S" if when.date() == datetime.now().date() else "%d %b %H:%M:%S"
    return when.strftime(fmt)


class EnrollModal(ModalScreen[str]):
    def compose(self) -> ComposeResult:
        yield Container(
            Label("Enter username to enroll:"),
            Input(id="user_input", placeholder="username"),
            Horizontal(
                Button("Cancel", id="btn_cancel", variant="error"),
                Button("Enroll", id="btn_confirm", variant="success"),
                classes="dialog_buttons"
            ),
            id="dialog"
        )

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "btn_confirm":
            inp = self.query_one("#user_input", Input).value.strip()
            if inp:
                self.dismiss(inp)
        else:
            self.dismiss("")

class ConfirmRemoveModal(ModalScreen[bool]):
    def __init__(self, username: str):
        super().__init__()
        self.username = username

    def compose(self) -> ComposeResult:
        yield Container(
            Label(f"Are you sure you want to remove user '{self.username}'?"),
            Horizontal(
                Button("Cancel", id="btn_cancel"),
                Button("Remove", id="btn_confirm", variant="error"),
                classes="dialog_buttons"
            ),
            id="dialog"
        )

    def on_button_pressed(self, event: Button.Pressed) -> None:
        self.dismiss(event.button.id == "btn_confirm")

class DashboardScreen(Screen):
    def compose(self) -> ComposeResult:
        yield Header(show_clock=True)
        yield Horizontal(
            Vertical(
                Static(f"[bold cyan]Sentinel Recreated v{__version__}[/bold cyan]\n", id="title_banner"),
                Static("Loading daemon status...", id="status_panel"),
                Button("Test face unlock now", id="btn_test", variant="primary"),
                classes="column"
            ),
            Vertical(
                Static("[bold yellow]Recent Authentication Log (Auto-refresh 5s)[/bold yellow]"),
                Static(
                    "[dim]Distance: 0 = same face, smaller is a better match. "
                    "Anti-spoof: 0 to 1, higher = more likely a live face.[/dim]\n"
                ),
                DataTable(id="log_table"),
                classes="column"
            )
        )
        yield Footer()

    def on_mount(self) -> None:
        table = self.query_one("#log_table", DataTable)
        table.add_columns("Time", "User", "Result", "Distance", "Tier", "Anti-spoof", "Took (ms)")
        self.refresh_dashboard()
        self.set_interval(5.0, self.refresh_dashboard)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "btn_test":
            self.app.action_test_auth()

    def refresh_dashboard(self) -> None:
        # The refresh timer keeps running after another screen is opened;
        # there is nothing to update (and no widgets to find) until we are back.
        if not self.is_current:
            return
        try:
            status = self.app.dbus_client.get_status()
            models = status.get("models_loaded", {})
            m_scrfd = "[green]✓[/green]" if models.get("scrfd_500m_kps") else "[red]✗[/red]"
            m_mfn = "[green]✓[/green]" if models.get("mobile_facenet") else "[red]✗[/red]"
            m_spoof = "[green]✓[/green]" if models.get("minifasnetv2") else "[red]✗ (face unlock is off without it)[/red]"

            last_res = status.get("last_auth_result", "None")
            res_color = "green" if "GRANTED" in last_res else ("red" if ("DENIED" in last_res or "SPOOF" in last_res) else "yellow")

            status_text = (
                f"[bold]Daemon Uptime:[/bold] {status.get('daemon_uptime_secs', 0)}s\n"
                f"[bold]Camera Source:[/bold] {status.get('camera_source', 'N/A')}\n"
                f"[bold]Enrolled Users:[/bold] {status.get('enrolled_users_count', 0)}\n\n"
                f"[bold]Models Loaded:[/bold]\n"
                f"  SCRFD 500M: {m_scrfd}\n"
                f"  MobileFaceNet: {m_mfn}\n"
                f"  Anti-spoof: {m_spoof}\n\n"
                f"[bold]Last Auth Result:[/bold] [{res_color}]{last_res}[/{res_color}]\n"
            )
            self.query_one("#status_panel", Static).update(status_text)
        except Exception as e:
            self.query_one("#status_panel", Static).update(f"[red]Error fetching status: {error_text(e)}[/red]")

        # Auth log reading via DBus service call
        table = self.query_one("#log_table", DataTable)
        table.clear()
        empty_reason = "No auth events yet"
        has_entries = False
        if getattr(self, "log_error", None):
            empty_reason = self.log_error
        else:
            try:
                lines = self.app.dbus_client.get_recent_auth_log(15)
                for line in reversed(lines):  # newest at the top
                    parts = line.split("|")
                    if len(parts) >= 8:
                        ts, usr, res, dist, tier, _live, spoof, ms = parts[:8]
                        c = "green" if res == "GRANTED" else ("red" if res in ("DENIED", "SPOOF") else "yellow")
                        table.add_row(local_time(ts), usr, f"[{c}]{res}[/{c}]", dist, tier, spoof, ms)
                        has_entries = True
            except Exception as e:
                # If unauthorized or cancelled, avoid asking again every 5 seconds
                self.log_error = f"Log unavailable: {error_text(e)}"
                empty_reason = self.log_error
        if not has_entries:
            table.add_row("-", "-", f"[dim]{empty_reason}[/dim]", "-", "-", "-", "-")

class UsersScreen(Screen):
    def compose(self) -> ComposeResult:
        yield Header()
        yield Horizontal(
            Button("Enroll New User", id="btn_enroll", variant="success"),
            Button("Remove Selected User", id="btn_remove", variant="error"),
            classes="action_bar"
        )
        yield DataTable(id="users_table")
        yield Footer()

    def on_mount(self) -> None:
        table = self.query_one("#users_table", DataTable)
        table.add_columns("Username", "Enrolled Vecs", "Learned Vecs", "Last Learned", "Enrolled Date")
        self.refresh_users()

    def refresh_users(self) -> None:
        table = self.query_one("#users_table", DataTable)
        saved_row = table.cursor_row
        table.clear()
        try:
            users = self.app.dbus_client.list_users()
            for u in users:
                info = self.app.dbus_client.get_user_info(u)
                table.add_row(
                    str(info.get("username", u)),
                    str(info.get("core_vector_count", "N/A")),
                    str(info.get("adaptive_vector_count", "N/A")),
                    str(info.get("last_adaptation_date", "N/A")),
                    str(info.get("enrolled_at", "N/A"))
                )
        except Exception as e:
            self.app.notify(f"Could not load users: {error_text(e)}", severity="error")
        if saved_row is not None and saved_row < table.row_count:
            table.move_cursor(row=saved_row)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "btn_enroll":
            self.app.action_enroll_user()
        elif event.button.id == "btn_remove":
            table = self.query_one("#users_table", DataTable)
            if table.cursor_row is not None and table.cursor_row < table.row_count:
                row = table.get_row_at(table.cursor_row)
                username = str(row[0])
                def on_confirm(confirmed: bool):
                    if confirmed:
                        try:
                            if not self.app.dbus_client.remove_user(username):
                                self.app.notify(f"User '{username}' was not removed.", severity="warning")
                        except Exception as e:
                            self.app.notify(f"Could not remove '{username}': {error_text(e)}", severity="error")
                        self.refresh_users()
                self.app.push_screen(ConfirmRemoveModal(username), on_confirm)

class IntrusionsScreen(Screen):
    def compose(self) -> ComposeResult:
        yield Header()
        yield Horizontal(
            Button("View Selected", id="btn_view", variant="primary"),
            Button("Dismiss Selected", id="btn_dismiss", variant="warning"),
            Button("Dismiss All", id="btn_dismiss_all", variant="error"),
            classes="action_bar"
        )
        yield DataTable(id="intrusions_table")
        yield Static(
            "[dim]Photos of faces that were clearly not the enrolled user. They are readable by root only, "
            "so viewing one asks for the administrator password.[/dim]",
            id="intrusion_note"
        )
        yield Footer()

    def on_mount(self) -> None:
        table = self.query_one("#intrusions_table", DataTable)
        table.add_columns("Filename", "Taken")
        self.refresh_intrusions()

    def refresh_intrusions(self) -> None:
        table = self.query_one("#intrusions_table", DataTable)
        saved_row = table.cursor_row
        table.clear()
        try:
            files = self.app.dbus_client.get_intrusion_list()
            for f in sorted(files, reverse=True):  # newest first
                ts = "N/A"
                if f.startswith("intrusion_") and f.endswith(".jpg"):
                    raw_ts = f[10:-4]
                    if len(raw_ts) == 15 and "_" in raw_ts:
                        d, t = raw_ts.split("_")
                        ts = f"{d[:4]}-{d[4:6]}-{d[6:]} {t[:2]}:{t[2:4]}:{t[4:]}"
                table.add_row(f, ts)
        except Exception as e:
            self.app.notify(f"Could not load intrusion list: {error_text(e)}", severity="error")
        if saved_row is not None and saved_row < table.row_count:
            table.move_cursor(row=saved_row)

    def selected_filename(self) -> str | None:
        table = self.query_one("#intrusions_table", DataTable)
        if table.cursor_row is not None and table.cursor_row < table.row_count:
            return str(table.get_row_at(table.cursor_row)[0])
        return None

    def view_photo(self, filename: str) -> None:
        # The name goes to a root command, so accept only the daemon's own naming.
        if not re.fullmatch(r"intrusion_\d{8}_\d{6}\.jpg", filename):
            self.app.notify(f"Not a photo: {filename}", severity="error")
            return
        with self.app.suspend():
            print(f"Administrator rights are needed to read {filename} ...")
            proc = subprocess.run(["pkexec", "cat", f"{INTRUSION_DIR}/{filename}"], stdout=subprocess.PIPE)
        if proc.returncode != 0 or not proc.stdout:
            self.app.notify("Could not read the photo (authorisation refused?).", severity="error")
            return
        # Private copy for the image viewer; the runtime dir is cleared at logout.
        fd, path = tempfile.mkstemp(prefix="sentinel_", suffix=".jpg", dir=os.environ.get("XDG_RUNTIME_DIR"))
        with os.fdopen(fd, "wb") as out:
            out.write(proc.stdout)
        try:
            subprocess.Popen(["xdg-open", path], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except OSError as e:
            self.app.notify(f"Could not open an image viewer: {e}", severity="error")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "btn_view":
            filename = self.selected_filename()
            if filename:
                self.view_photo(filename)
        elif event.button.id == "btn_dismiss":
            filename = self.selected_filename()
            if filename:
                try:
                    self.app.dbus_client.dismiss_intrusion(filename)
                except Exception as e:
                    self.app.notify(f"Could not dismiss {filename}: {error_text(e)}", severity="error")
                self.refresh_intrusions()
        elif event.button.id == "btn_dismiss_all":
            try:
                files = self.app.dbus_client.get_intrusion_list()
                for f in files:
                    self.app.dbus_client.dismiss_intrusion(f)
            except Exception as e:
                self.app.notify(f"Could not dismiss all: {error_text(e)}", severity="error")
            self.refresh_intrusions()

class SettingsScreen(Screen):
    # (input id, config section, key, type, label)
    FIELDS = [
        ("in_golden", "security", "golden_threshold", float,
         "golden_threshold — face distance for a strong match (one frame is enough); smaller is stricter:"),
        ("in_standard", "security", "standard_threshold", float,
         "standard_threshold — face distance for a normal match (three frames in a row):"),
        ("in_2fa", "security", "two_factor_threshold", float,
         "two_factor_threshold — above this the face does not match:"),
        ("in_spoof", "security", "spoof_threshold", float,
         "spoof_threshold — anti-spoof score needed for a one-frame grant; larger is stricter:"),
        ("in_spoof_std", "security", "spoof_threshold_standard", float,
         "spoof_threshold_standard — anti-spoof score needed on every frame of a voted grant:"),
        ("in_timeout", "security", "global_session_timeout", float,
         "global_session_timeout — seconds before a scan gives up (2 to 7):"),
        ("in_scrfd", "detection", "scrfd_input_size", int,
         "scrfd_input_size — detector resolution (320 fast, 640 for faces further away):"),
        ("in_camera", "camera", "source", str,
         "camera.source — camera device, e.g. /dev/video0:"),
    ]

    def compose(self) -> ComposeResult:
        yield Header()
        widgets = []
        for input_id, _section, _key, _kind, label in self.FIELDS:
            widgets += [Label(label), Input(id=input_id)]
        yield VerticalScroll(
            *widgets,
            Button("Save Configuration", id="btn_save", variant="primary"),
            Static("", id="save_status"),
            Static("", id="daemon_summary"),
            id="settings_container"
        )
        yield Footer()

    def on_mount(self) -> None:
        self.load_settings()

    @staticmethod
    def to_toml(value) -> str:
        if isinstance(value, float):
            text = f"{value:.4f}".rstrip("0")
            return text + "0" if text.endswith(".") else text
        if isinstance(value, str):
            return json.dumps(value)  # quoted and escaped; valid as a TOML string
        return str(value)

    def load_settings(self) -> None:
        try:
            self.raw_toml = self.app.dbus_client.get_config()
            cfg = tomllib.loads(self.raw_toml)
            for input_id, section, key, kind, _label in self.FIELDS:
                value = cfg.get(section, {}).get(key, "")
                text = f"{value:g}" if kind is float and isinstance(value, (int, float)) else str(value)
                self.query_one(f"#{input_id}", Input).value = text

            status = self.app.dbus_client.get_status()
            self.query_one("#daemon_summary", Static).update(
                f"[dim]Daemon Uptime: {status.get('daemon_uptime_secs', 0)}s | "
                f"Enrolled Users: {status.get('enrolled_users_count', 0)} | "
                f"Camera: {status.get('camera_source', 'N/A')}[/dim]"
            )
        except Exception as e:
            self.query_one("#save_status", Static).update(f"[red]Error loading config: {error_text(e)}[/red]")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id != "btn_save":
            return
        status = self.query_one("#save_status", Static)
        if not getattr(self, "raw_toml", None):
            status.update("[red]Nothing to save: the configuration could not be loaded.[/red]")
            return
        try:
            updates = {}
            for input_id, section, key, kind, _label in self.FIELDS:
                text = self.query_one(f"#{input_id}", Input).value.strip()
                try:
                    updates[(section, key)] = kind(text)
                except ValueError:
                    status.update(f"[red]{key}: '{text}' is not a valid {'number' if kind is not str else 'value'}.[/red]")
                    return

            # Patch TOML preserving other sections and keys
            new_lines = []
            curr_sec = ""
            applied = set()
            for line in self.raw_toml.splitlines():
                stripped = line.strip()
                if stripped.startswith("[") and stripped.endswith("]"):
                    curr_sec = stripped[1:-1].strip()
                elif "=" in line and not stripped.startswith("#"):
                    key = line.split("=", 1)[0].strip()
                    if (curr_sec, key) in updates:
                        new_lines.append(f"{key} = {self.to_toml(updates[(curr_sec, key)])}")
                        applied.add((curr_sec, key))
                        continue
                new_lines.append(line)

            missing = [f"{section}.{key}" for (section, key) in updates if (section, key) not in applied]
            if missing:
                status.update(f"[red]Not saved: the daemon's config has no {', '.join(missing)}.[/red]")
                return

            new_toml = "\n".join(new_lines) + "\n"
            tomllib.loads(new_toml)

            # The daemon range-checks every value and explains what it refuses.
            success, msg = self.app.dbus_client.set_config(new_toml)
            if success:
                self.raw_toml = new_toml
                status.update("[green]Configuration saved. It applies from the next face scan.[/green]")
            else:
                status.update(f"[red]Save failed: {msg}[/red]")
        except Exception as e:
            status.update(f"[red]Error saving config: {error_text(e)}[/red]")

class SentinelApp(App):
    CSS = """
    .column { width: 50%; height: 100%; border: solid green; padding: 1; }
    .action_bar { height: 3; margin-bottom: 1; }
    #dialog { width: 60; height: 13; border: thick $accent; padding: 1 2; background: $surface; align: center middle; }
    .dialog_buttons { margin-top: 1; align: center middle; }
    #settings_container { padding: 1 2; }
    """
    SCREENS = {
        "dashboard": DashboardScreen,
        "users": UsersScreen,
        "intrusions": IntrusionsScreen,
        "settings": SettingsScreen
    }
    BINDINGS = [
        ("d", "switch_screen('dashboard')", "Dashboard"),
        ("t", "test_auth", "Test unlock"),
        ("e", "enroll_user", "Enroll"),
        ("u", "switch_screen('users')", "Users"),
        ("i", "switch_screen('intrusions')", "Intrusions"),
        ("s", "switch_screen('settings')", "Settings"),
        ("q", "quit", "Quit")
    ]

    def on_mount(self) -> None:
        try:
            self.dbus_client = SentinelDBusClient()
        except Exception as e:
            self.exit(message=f"Cannot reach the Sentinel daemon: {error_text(e)}\nIs it running?  systemctl status sentinel")
            return
        self.push_screen("dashboard")

    def refresh_current_screen(self) -> None:
        for name in ("refresh_users", "refresh_dashboard"):
            if hasattr(self.screen, name):
                getattr(self.screen, name)()

    def action_enroll_user(self) -> None:
        def on_enroll_modal(username: str):
            if username:
                with self.suspend():
                    subprocess.run(["sentinel", "enroll", username])
                self.refresh_current_screen()
        self.push_screen(EnrollModal(), on_enroll_modal)

    def action_test_auth(self) -> None:
        """Run one real face scan for the current user, outside the TUI so the result stays readable."""
        with self.suspend():
            subprocess.run(["sentinel", "auth", current_user()])
            try:
                input("\nPress Enter to return to the dashboard...")
            except EOFError:
                pass
        self.refresh_current_screen()

if __name__ == "__main__":
    app = SentinelApp()
    app.run()
