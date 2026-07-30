import os
import glob
import json
import tomllib
import subprocess
from datetime import datetime
from textual.app import App, ComposeResult
from textual.screen import Screen, ModalScreen
from textual.widgets import Header, Footer, DataTable, Static, Button, Input, Label
from textual.containers import Container, Horizontal, Vertical
from sentinel_py.dbus_client import SentinelDBusClient

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
                Static("[bold cyan]Sentinel Recreated v1.0.0[/bold cyan]\n", id="title_banner"),
                Static("Loading daemon status...", id="status_panel"),
                classes="column"
            ),
            Vertical(
                Static("[bold yellow]Live Auth Log (Auto-refresh 5s)[/bold yellow]\n"),
                DataTable(id="log_table"),
                classes="column"
            )
        )
        yield Footer()

    def on_mount(self) -> None:
        table = self.query_one("#log_table", DataTable)
        table.add_columns("Time", "User", "Result", "Dist", "Tier", "Liveness", "Spoof", "ms")
        self.refresh_dashboard()
        self.set_interval(5.0, self.refresh_dashboard)

    def refresh_dashboard(self) -> None:
        try:
            status = self.app.dbus_client.get_status()
            models = status.get("models_loaded", {})
            m_scrfd = "[green]✓[/green]" if models.get("scrfd_500m_kps") else "[red]✗[/red]"
            m_mfn = "[green]✓[/green]" if models.get("mobile_facenet") else "[red]✗[/red]"
            m_spoof = "[green]✓[/green]" if models.get("minifasnetv2") else "[red]✗[/red]"

            last_res = status.get("last_auth_result", "None")
            res_color = "green" if "GRANTED" in last_res else ("red" if "DENIED" in last_res else "yellow")

            status_text = (
                f"[bold]Daemon Uptime:[/bold] {status.get('daemon_uptime_secs', 0)}s\n"
                f"[bold]Camera Source:[/bold] {status.get('camera_source', 'N/A')}\n"
                f"[bold]Enrolled Users:[/bold] {status.get('enrolled_users_count', 0)}\n\n"
                f"[bold]Models Loaded:[/bold]\n"
                f"  SCRFD 500M: {m_scrfd}\n"
                f"  MobileFaceNet: {m_mfn}\n"
                f"  MiniFASNetV2: {m_spoof}\n\n"
                f"[bold]Last Auth Result:[/bold] [{res_color}]{last_res}[/{res_color}]"
            )
            self.query_one("#status_panel", Static).update(status_text)
        except Exception as e:
            self.query_one("#status_panel", Static).update(f"[red]Error fetching status: {e}[/red]")

        # Auth log reading with missing/empty file graceful handling
        table = self.query_one("#log_table", DataTable)
        table.clear()
        today_log = datetime.now().strftime("/var/log/sentinel/auth_%Y-%m-%d.log")
        has_entries = False
        if os.path.exists(today_log):
            try:
                with open(today_log, "r") as f:
                    lines = [line.strip() for line in f if line.strip()][-10:]
                for line in lines:
                    parts = line.split("|")
                    if len(parts) >= 8:
                        ts, usr, res, dist, tier, live, spoof, ms = parts[:8]
                        c = "green" if res == "GRANTED" else ("red" if res == "DENIED" else "yellow")
                        table.add_row(ts, usr, f"[{c}]{res}[/{c}]", dist, tier, live, spoof, ms)
                        has_entries = True
            except Exception:
                pass
        if not has_entries:
            table.add_row("-", "-", "[dim]No auth events today[/dim]", "-", "-", "-", "-", "-")

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
        table.add_columns("Username", "Core Vecs", "Adaptive Vecs", "Last Adaptation", "Enrolled Date")
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
        except Exception:
            pass
        if saved_row is not None and saved_row < table.row_count:
            table.move_cursor(row=saved_row)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "btn_enroll":
            def on_enroll_modal(username: str):
                if username:
                    with self.app.suspend():
                        subprocess.run(["sentinel", "enroll", username])
                    self.refresh_users()
            self.app.push_screen(EnrollModal(), on_enroll_modal)
        elif event.button.id == "btn_remove":
            table = self.query_one("#users_table", DataTable)
            if table.cursor_row is not None and table.cursor_row < table.row_count:
                row = table.get_row_at(table.cursor_row)
                username = str(row[0])
                def on_confirm(confirmed: bool):
                    if confirmed:
                        try:
                            self.app.dbus_client.remove_user(username)
                        except Exception:
                            pass
                        self.refresh_users()
                self.app.push_screen(ConfirmRemoveModal(username), on_confirm)

class IntrusionsScreen(Screen):
    def compose(self) -> ComposeResult:
        yield Header()
        yield Horizontal(
            Button("Dismiss Selected", id="btn_dismiss", variant="warning"),
            Button("Dismiss All", id="btn_dismiss_all", variant="error"),
            classes="action_bar"
        )
        yield DataTable(id="intrusions_table")
        yield Static("[dim]Note: Open screenshots with: xdg-open /var/lib/sentinel/blacklist/<filename>[/dim]", id="intrusion_note")
        yield Footer()

    def on_mount(self) -> None:
        table = self.query_one("#intrusions_table", DataTable)
        table.add_columns("Filename", "Parsed Timestamp")
        self.refresh_intrusions()

    def refresh_intrusions(self) -> None:
        table = self.query_one("#intrusions_table", DataTable)
        saved_row = table.cursor_row
        table.clear()
        try:
            files = self.app.dbus_client.get_intrusion_list()
            for f in files:
                ts = "N/A"
                if f.startswith("intrusion_") and f.endswith(".jpg"):
                    raw_ts = f[10:-4]
                    if len(raw_ts) == 15 and "_" in raw_ts:
                        d, t = raw_ts.split("_")
                        ts = f"{d[:4]}-{d[4:6]}-{d[6:]} {t[:2]}:{t[2:4]}:{t[4:]}"
                table.add_row(f, ts)
        except Exception:
            pass
        if saved_row is not None and saved_row < table.row_count:
            table.move_cursor(row=saved_row)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        table = self.query_one("#intrusions_table", DataTable)
        if event.button.id == "btn_dismiss":
            if table.cursor_row is not None and table.cursor_row < table.row_count:
                filename = str(table.get_row_at(table.cursor_row)[0])
                try:
                    self.app.dbus_client.dismiss_intrusion(filename)
                except Exception:
                    pass
                self.refresh_intrusions()
        elif event.button.id == "btn_dismiss_all":
            try:
                files = self.app.dbus_client.get_intrusion_list()
                for f in files:
                    self.app.dbus_client.dismiss_intrusion(f)
            except Exception:
                pass
            self.refresh_intrusions()

class SettingsScreen(Screen):
    def compose(self) -> ComposeResult:
        yield Header()
        yield Vertical(
            Label("golden_threshold:"), Input(id="in_golden"),
            Label("standard_threshold:"), Input(id="in_standard"),
            Label("two_factor_threshold:"), Input(id="in_2fa"),
            Label("spoof_threshold:"), Input(id="in_spoof"),
            Label("scrfd_input_size:"), Input(id="in_scrfd"),
            Label("camera.source:"), Input(id="in_camera"),
            Button("Save Configuration", id="btn_save", variant="primary"),
            Static("", id="save_status"),
            Static("Loading benchmarks...", id="benchmark_panel"),
            id="settings_container"
        )
        yield Footer()

    def on_mount(self) -> None:
        self.load_settings()

    def load_settings(self) -> None:
        try:
            self.raw_toml = self.app.dbus_client.get_config()
            cfg = tomllib.loads(self.raw_toml)
            sec = cfg.get("security", {})
            det = cfg.get("detection", {})
            cam = cfg.get("camera", {})

            self.query_one("#in_golden", Input).value = str(sec.get("golden_threshold", 0.28))
            self.query_one("#in_standard", Input).value = str(sec.get("standard_threshold", 0.42))
            self.query_one("#in_2fa", Input).value = str(sec.get("two_factor_threshold", 0.50))
            self.query_one("#in_spoof", Input).value = str(sec.get("spoof_threshold", 0.85))
            self.query_one("#in_scrfd", Input).value = str(det.get("scrfd_input_size", 320))
            self.query_one("#in_camera", Input).value = str(cam.get("source", "/dev/video0"))

            status = self.app.dbus_client.get_status()
            self.query_one("#benchmark_panel", Static).update(
                f"[dim]Daemon Uptime: {status.get('daemon_uptime_secs', 0)}s | "
                f"Enrolled Users: {status.get('enrolled_users_count', 0)} | "
                f"Camera: {status.get('camera_source', 'N/A')}[/dim]"
            )
        except Exception as e:
            self.query_one("#save_status", Static).update(f"[red]Error loading config: {e}[/red]")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "btn_save":
            try:
                updates = {
                    "golden_threshold": float(self.query_one("#in_golden", Input).value),
                    "standard_threshold": float(self.query_one("#in_standard", Input).value),
                    "two_factor_threshold": float(self.query_one("#in_2fa", Input).value),
                    "spoof_threshold": float(self.query_one("#in_spoof", Input).value),
                    "scrfd_input_size": int(self.query_one("#in_scrfd", Input).value),
                    "camera.source": self.query_one("#in_camera", Input).value.strip()
                }

                # Patch TOML preserving other sections
                lines = self.raw_toml.splitlines()
                new_lines = []
                curr_sec = ""
                for line in lines:
                    stripped = line.strip()
                    if stripped.startswith("[") and stripped.endswith("]"):
                        curr_sec = stripped[1:-1].strip()
                        new_lines.append(line)
                        continue
                    if "=" in line and not stripped.startswith("#"):
                        k, _ = line.split("=", 1)
                        k = k.strip()
                        if curr_sec == "camera" and k == "source":
                            new_lines.append(f'source = "{updates["camera.source"]}"')
                            continue
                        elif curr_sec == "security" and k in updates:
                            v = updates[k]
                            new_lines.append(f'{k} = {v:.4f}'.rstrip('0').rstrip('.'))
                            continue
                        elif curr_sec == "detection" and k == "scrfd_input_size":
                            new_lines.append(f'scrfd_input_size = {updates["scrfd_input_size"]}')
                            continue
                    new_lines.append(line)

                new_toml = "\n".join(new_lines) + "\n"
                # Validate with tomllib
                tomllib.loads(new_toml)

                success, msg = self.app.dbus_client.set_config(new_toml)
                if success:
                    self.raw_toml = new_toml
                    self.query_one("#save_status", Static).update("[green]Configuration saved successfully![/green]")
                else:
                    self.query_one("#save_status", Static).update(f"[red]Save failed: {msg}[/red]")
            except Exception as e:
                self.query_one("#save_status", Static).update(f"[red]Error saving config: {e}[/red]")

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
        ("e", "enroll_user", "Enroll"),
        ("u", "switch_screen('users')", "Users"),
        ("i", "switch_screen('intrusions')", "Intrusions"),
        ("s", "switch_screen('settings')", "Settings"),
        ("q", "quit", "Quit")
    ]

    def on_mount(self) -> None:
        self.dbus_client = SentinelDBusClient()
        self.push_screen("dashboard")

    def action_enroll_user(self) -> None:
        def on_enroll_modal(username: str):
            if username:
                with self.suspend():
                    subprocess.run(["sentinel", "enroll", username])
                if self.screen and hasattr(self.screen, "refresh_users"):
                    self.screen.refresh_users()
        self.push_screen(EnrollModal(), on_enroll_modal)

if __name__ == "__main__":
    app = SentinelApp()
    app.run()
