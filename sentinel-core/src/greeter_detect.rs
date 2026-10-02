use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Supported or recognized display manager / greeter types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GreeterType {
    DankGreeter,
    Greetd,
    Gdm,
    Sddm,
    LightDm,
    Unknown,
}

impl GreeterType {
    pub fn as_str(&self) -> &'static str {
        match self {
            GreeterType::DankGreeter => "DankGreeter",
            GreeterType::Greetd => "Greetd",
            GreeterType::Gdm => "Gdm",
            GreeterType::Sddm => "Sddm",
            GreeterType::LightDm => "LightDm",
            GreeterType::Unknown => "Unknown",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            GreeterType::DankGreeter => "Dank Greeter (dms-greeter)",
            GreeterType::Greetd => "Greetd (Generic)",
            GreeterType::Gdm => "GDM (GNOME Display Manager)",
            GreeterType::Sddm => "SDDM (Simple Desktop Display Manager)",
            GreeterType::LightDm => "LightDM",
            GreeterType::Unknown => "Unknown / Custom Lock Screen",
        }
    }

    pub fn default_pam_service(&self) -> &'static str {
        match self {
            GreeterType::DankGreeter | GreeterType::Greetd => "greetd",
            GreeterType::Gdm => "gdm-password",
            GreeterType::Sddm => "sddm",
            GreeterType::LightDm => "lightdm",
            GreeterType::Unknown => "system-auth",
        }
    }
}

/// Comprehensive greeter integration information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GreeterInfo {
    pub greeter_type: GreeterType,
    pub greeter_name: String,
    pub has_tab_trigger: bool,
    pub has_face_pam_icon: bool,
    pub pam_service: String,
    pub pam_service_path: String,
    pub sentinel_pam_configured: bool,
    pub setup_hint: Option<String>,
}

/// Check if a PAM configuration file exists and contains `pam_sentinel.so`.
pub fn is_sentinel_in_pam(pam_file: &Path) -> bool {
    if let Ok(content) = fs::read_to_string(pam_file) {
        content
            .lines()
            .any(|line| {
                let trimmed = line.trim();
                !trimmed.starts_with('#') && trimmed.contains("pam_sentinel.so")
            })
    } else {
        false
    }
}

/// Inspect `/proc` for running display manager processes.
fn detect_from_proc(proc_root: &Path) -> Option<GreeterType> {
    let entries = fs::read_dir(proc_root).ok()?;
    let mut found_greetd = false;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let pid_dir = entry.path();

        // 1. Check comm
        let comm_path = pid_dir.join("comm");
        let comm = fs::read_to_string(&comm_path).unwrap_or_default().trim().to_string();

        // 2. Check cmdline
        let cmdline_path = pid_dir.join("cmdline");
        let cmdline = fs::read_to_string(&cmdline_path).unwrap_or_default();

        if comm.contains("dms-greeter") || cmdline.contains("dms-greeter") {
            return Some(GreeterType::DankGreeter);
        }

        if comm == "greetd" || cmdline.contains("greetd") {
            found_greetd = true;
        } else if comm.starts_with("gdm") || cmdline.contains("/gdm") {
            return Some(GreeterType::Gdm);
        } else if comm.starts_with("sddm") || cmdline.contains("sddm") {
            return Some(GreeterType::Sddm);
        } else if comm.starts_with("lightdm") || cmdline.contains("lightdm") {
            return Some(GreeterType::LightDm);
        }
    }

    if found_greetd {
        Some(GreeterType::Greetd)
    } else {
        None
    }
}

/// Inspect systemd display-manager.service symlink.
fn detect_from_systemd(root: &Path) -> Option<GreeterType> {
    let dm_symlink = root.join("etc/systemd/system/display-manager.service");
    if let Ok(target) = fs::read_link(&dm_symlink) {
        let target_str = target.to_string_lossy().to_lowercase();
        if target_str.contains("greetd") {
            return Some(GreeterType::Greetd);
        } else if target_str.contains("gdm") {
            return Some(GreeterType::Gdm);
        } else if target_str.contains("sddm") {
            return Some(GreeterType::Sddm);
        } else if target_str.contains("lightdm") {
            return Some(GreeterType::LightDm);
        }
    }
    None
}

/// Check if greetd configuration references dms-greeter.
fn is_greetd_using_dms(root: &Path) -> bool {
    let greetd_conf = root.join("etc/greetd/config.toml");
    if let Ok(content) = fs::read_to_string(&greetd_conf) {
        content.contains("dms-greeter")
    } else {
        false
    }
}

/// Perform greeter detection with configurable filesystem root (enables unit testing).
pub fn detect_with_root(root: &Path) -> GreeterInfo {
    let proc_path = root.join("proc");
    let mut detected_type = if proc_path.exists() {
        detect_from_proc(&proc_path)
    } else {
        None
    };

    if detected_type.is_none() {
        detected_type = detect_from_systemd(root);
    }

    // Refine Greetd -> DankGreeter if /etc/greetd/config.toml points to dms-greeter
    if matches!(detected_type, Some(GreeterType::Greetd) | None) && is_greetd_using_dms(root) {
        detected_type = Some(GreeterType::DankGreeter);
    }

    // If still None, check existing pam.d configurations
    if detected_type.is_none() {
        if root.join("etc/pam.d/greetd").exists() {
            detected_type = Some(GreeterType::Greetd);
        } else if root.join("etc/pam.d/gdm-password").exists() {
            detected_type = Some(GreeterType::Gdm);
        } else if root.join("etc/pam.d/sddm").exists() {
            detected_type = Some(GreeterType::Sddm);
        } else if root.join("etc/pam.d/lightdm").exists() {
            detected_type = Some(GreeterType::LightDm);
        }
    }

    let greeter_type = detected_type.unwrap_or(GreeterType::Unknown);
    let pam_service = greeter_type.default_pam_service().to_string();
    let pam_file_rel = format!("etc/pam.d/{}", pam_service);
    let pam_service_path = root.join(&pam_file_rel);
    let sentinel_pam_configured = is_sentinel_in_pam(&pam_service_path);

    let has_tab_trigger = matches!(greeter_type, GreeterType::DankGreeter);
    let has_face_pam_icon = matches!(greeter_type, GreeterType::DankGreeter);

    let setup_hint = match greeter_type {
        GreeterType::DankGreeter => {
            if sentinel_pam_configured {
                Some("Native Tab-key face authentication and face icon active (upstream PR #21). Press Tab on an empty password field to scan face.".to_string())
            } else {
                Some("Dank Greeter detected, but pam_sentinel.so is missing from /etc/pam.d/greetd. Add 'auth sufficient pam_sentinel.so' to enable face authentication.".to_string())
            }
        }
        GreeterType::Greetd => {
            if sentinel_pam_configured {
                Some("Generic greetd active. Face authentication is triggered via the PAM conversation prompt on Enter.".to_string())
            } else {
                Some("Configure pam_sentinel.so in /etc/pam.d/greetd to enable face authentication.".to_string())
            }
        }
        GreeterType::Gdm => {
            if sentinel_pam_configured {
                Some("GDM does not support custom key triggers or biometric icons. Press Enter on an empty password field to trigger face scan via PAM conversation.".to_string())
            } else {
                Some("Configure pam_sentinel.so in /etc/pam.d/gdm-password to enable face authentication.".to_string())
            }
        }
        GreeterType::Sddm => {
            if sentinel_pam_configured {
                Some("SDDM active. Face authentication is triggered via the PAM conversation prompt on Enter.".to_string())
            } else {
                Some("Configure pam_sentinel.so in /etc/pam.d/sddm to enable face authentication.".to_string())
            }
        }
        GreeterType::LightDm => {
            if sentinel_pam_configured {
                Some("LightDM active. Face authentication is triggered via the PAM conversation prompt on Enter.".to_string())
            } else {
                Some("Configure pam_sentinel.so in /etc/pam.d/lightdm to enable face authentication.".to_string())
            }
        }
        GreeterType::Unknown => {
            if sentinel_pam_configured {
                Some("Custom display manager or lock screen. Face authentication works via standard PAM conversation.".to_string())
            } else {
                Some("Configure pam_sentinel.so in your display manager's PAM service file (e.g., /etc/pam.d/login or /etc/pam.d/system-auth).".to_string())
            }
        }
    };

    GreeterInfo {
        greeter_type,
        greeter_name: greeter_type.display_name().to_string(),
        has_tab_trigger,
        has_face_pam_icon,
        pam_service,
        pam_service_path: pam_service_path.to_string_lossy().to_string(),
        sentinel_pam_configured,
        setup_hint,
    }
}

/// Detect the active greeter from the current host root filesystem ("/").
pub fn detect() -> GreeterInfo {
    detect_with_root(Path::new("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_greeter_type_properties() {
        assert_eq!(GreeterType::DankGreeter.as_str(), "DankGreeter");
        assert_eq!(GreeterType::DankGreeter.default_pam_service(), "greetd");
        assert_eq!(GreeterType::Gdm.default_pam_service(), "gdm-password");
        assert_eq!(GreeterType::Sddm.default_pam_service(), "sddm");
        assert_eq!(GreeterType::LightDm.default_pam_service(), "lightdm");
        assert_eq!(GreeterType::Unknown.default_pam_service(), "system-auth");
    }

    #[test]
    fn test_is_sentinel_in_pam() {
        let temp_dir = std::env::temp_dir().join(format!("sentinel_pam_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);
        let pam_file = temp_dir.join("test_pam");

        // 1. Missing sentinel
        fs::write(&pam_file, "auth required pam_unix.so\nauth optional pam_permit.so\n").unwrap();
        assert!(!is_sentinel_in_pam(&pam_file));

        // 2. Commented out sentinel
        fs::write(&pam_file, "# auth sufficient pam_sentinel.so\nauth required pam_unix.so\n").unwrap();
        assert!(!is_sentinel_in_pam(&pam_file));

        // 3. Active sentinel
        fs::write(&pam_file, "auth sufficient pam_sentinel.so\nauth include system-auth\n").unwrap();
        assert!(is_sentinel_in_pam(&pam_file));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_detect_dank_greeter_from_greetd_config() {
        let unique_name = format!("sentinel_greeter_test_{}", std::process::id());
        let temp_root = std::env::temp_dir().join(unique_name);
        let greetd_dir = temp_root.join("etc/greetd");
        let pam_dir = temp_root.join("etc/pam.d");
        fs::create_dir_all(&greetd_dir).unwrap();
        fs::create_dir_all(&pam_dir).unwrap();

        // Write greetd config pointing to dms-greeter
        let config_path = greetd_dir.join("config.toml");
        let mut f = fs::File::create(&config_path).unwrap();
        writeln!(f, "[default_session]").unwrap();
        writeln!(f, "command = \"/usr/bin/dms-greeter --command niri\"").unwrap();

        // Write pam.d/greetd with pam_sentinel.so
        let pam_greetd = pam_dir.join("greetd");
        let mut pf = fs::File::create(&pam_greetd).unwrap();
        writeln!(pf, "auth sufficient pam_sentinel.so").unwrap();
        writeln!(pf, "auth include system-auth").unwrap();

        let info = detect_with_root(&temp_root);
        assert_eq!(info.greeter_type, GreeterType::DankGreeter);
        assert!(info.has_tab_trigger);
        assert!(info.has_face_pam_icon);
        assert_eq!(info.pam_service, "greetd");
        assert!(info.sentinel_pam_configured);
        assert!(info.setup_hint.unwrap().contains("Native Tab-key"));

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_detect_gdm_from_systemd_symlink() {
        let unique_name = format!("sentinel_gdm_test_{}", std::process::id());
        let temp_root = std::env::temp_dir().join(unique_name);
        let sys_dir = temp_root.join("etc/systemd/system");
        let pam_dir = temp_root.join("etc/pam.d");
        fs::create_dir_all(&sys_dir).unwrap();
        fs::create_dir_all(&pam_dir).unwrap();

        #[cfg(unix)]
        {
            let symlink = sys_dir.join("display-manager.service");
            let target = Path::new("/usr/lib/systemd/system/gdm.service");
            let _ = std::os::unix::fs::symlink(target, &symlink);
        }

        let pam_gdm = pam_dir.join("gdm-password");
        let mut pf = fs::File::create(&pam_gdm).unwrap();
        writeln!(pf, "auth sufficient pam_sentinel.so").unwrap();

        let info = detect_with_root(&temp_root);
        assert_eq!(info.greeter_type, GreeterType::Gdm);
        assert!(!info.has_tab_trigger);
        assert!(!info.has_face_pam_icon);
        assert_eq!(info.pam_service, "gdm-password");
        assert!(info.sentinel_pam_configured);
        assert!(info.setup_hint.unwrap().contains("GDM does not support custom key triggers"));

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_detect_unknown_fallback() {
        let unique_name = format!("sentinel_unknown_test_{}", std::process::id());
        let temp_root = std::env::temp_dir().join(unique_name);
        fs::create_dir_all(&temp_root).unwrap();

        let info = detect_with_root(&temp_root);
        assert_eq!(info.greeter_type, GreeterType::Unknown);
        assert!(!info.has_tab_trigger);
        assert!(!info.has_face_pam_icon);
        assert!(!info.sentinel_pam_configured);

        let _ = fs::remove_dir_all(&temp_root);
    }
}
