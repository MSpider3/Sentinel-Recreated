use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use image::RgbImage;
use zbus::zvariant;

use crate::config::SentinelConfig;
use crate::gallery::GalleryStore;
use crate::pipeline::{
    align_face, ActiveTier, AuthState, FrameCapture, MobileFaceNet, ScrfdDetector,
    SentinelAuthenticator, SpoofDetector,
};

pub struct EnrollmentSession {
    pub session_id: String,
    pub username: String,
    pub pose_index: usize,
    pub total_poses: usize,
    pub collected_embeddings: Vec<[f32; 512]>,
}

pub struct SentinelService {
    pub config: Arc<Mutex<SentinelConfig>>,
    pub config_path: PathBuf,
    pub models_dir: PathBuf,
    pub start_time: Instant,
    pub last_auth_result: Arc<Mutex<String>>,
    pub active_enrollment: Arc<Mutex<Option<EnrollmentSession>>>,
    pub rt_handle: tokio::runtime::Handle,
}

impl SentinelService {
    pub fn new(
        config: SentinelConfig,
        config_path: PathBuf,
        models_dir: PathBuf,
        rt_handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            config: Arc::new(Mutex::new(config)),
            config_path,
            models_dir,
            start_time: Instant::now(),
            last_auth_result: Arc::new(Mutex::new("None".to_string())),
            active_enrollment: Arc::new(Mutex::new(None)),
            rt_handle,
        }
    }
}

fn is_lid_closed() -> bool {
    let lid_paths = [
        "/proc/acpi/button/lid/LID0/state",
        "/proc/acpi/button/lid/LID/state",
    ];
    for p in &lid_paths {
        if let Ok(content) = std::fs::read_to_string(p) {
            if content.to_lowercase().contains("closed") {
                return true;
            }
        }
    }
    false
}

async fn check_polkit(
    connection: &zbus::Connection,
    header: &zbus::MessageHeader<'_>,
    action_id: &str,
) -> zbus::fdo::Result<()> {
    let sender = header
        .sender()
        .ok_or_else(|| zbus::fdo::Error::Failed("Missing DBus sender".to_string()))?;

    let authority = zbus::Proxy::new(
        connection,
        "org.freedesktop.PolicyKit1",
        "/org/freedesktop/PolicyKit1/Authority",
        "org.freedesktop.PolicyKit1.Authority",
    )
    .await?;

    let mut details = HashMap::new();
    details.insert("name".to_string(), zvariant::Value::from(sender.as_str()));
    let subject = ("system-bus-name", details);

    let action_details: HashMap<String, String> = HashMap::new();
    let flags: u32 = 1; // AllowUserInteraction
    let cancellation_id = "";

    let res: (bool, bool, HashMap<String, String>) = authority
        .call(
            "CheckAuthorization",
            &(subject, action_id, action_details, flags, cancellation_id),
        )
        .await
        .map_err(|e| zbus::fdo::Error::Failed(format!("PolicyKit call error: {}", e)))?;

    let (is_authorized, _is_challenge, _res_details) = res;
    if is_authorized {
        Ok(())
    } else {
        Err(zbus::fdo::Error::NotSupported(format!(
            "PolicyKit authorization failed for action '{}'",
            action_id
        )))
    }
}

#[zbus::interface(name = "com.sentinel.Sentinel")]
impl SentinelService {
    /// Primary authentication method invoked by pam_sentinel.so
    async fn authenticate(
        &self,
        #[zbus(header)] _header: zbus::MessageHeader<'_>,
        #[zbus(signal_context)] ctxt: zbus::SignalContext<'_>,
        username: String,
        session_env: HashMap<String, String>,
    ) -> zbus::fdo::Result<(String, f64, i32)> {
        // 1. Session Context Evaluation (SSH / Lid check)
        if session_env.contains_key("SSH_CLIENT") || session_env.contains_key("SSH_TTY") {
            println!("[DBus] Remote SSH session detected for user '{}' — bypassing camera.", username);
            *self.last_auth_result.lock().unwrap() = "NO_FACE (SSH)".to_string();
            let _ = Self::auth_status_changed(&ctxt, "NO_FACE", "Remote SSH session detected").await;
            return Ok(("NO_FACE".to_string(), -1.0, 0));
        }

        if is_lid_closed() {
            println!("[DBus] Laptop lid is closed for user '{}' — bypassing camera.", username);
            *self.last_auth_result.lock().unwrap() = "NO_FACE (Lid Closed)".to_string();
            let _ = Self::auth_status_changed(&ctxt, "NO_FACE", "Laptop lid closed").await;
            return Ok(("NO_FACE".to_string(), -1.0, 0));
        }

        // 2. Load Gallery Vectors
        let store = GalleryStore::new(&username);
        let gallery = store.all_vectors().map_err(|e| {
            zbus::fdo::Error::Failed(format!("Failed to load gallery for '{}': {}", username, e))
        })?;

        if gallery.is_empty() {
            println!("[DBus] No enrolled gallery vectors for user '{}'.", username);
            *self.last_auth_result.lock().unwrap() = "DENIED (No Enrolled Template)".to_string();
            let _ = Self::auth_status_changed(&ctxt, "DENIED", "No enrolled template").await;
            return Ok(("DENIED".to_string(), 1.0, 4));
        }

        let _ = Self::auth_status_changed(&ctxt, "CALIBRATING", "Initializing camera and models...").await;

        let config = self.config.lock().unwrap().clone();
        let models_dir = self.models_dir.clone();
        let last_auth_res = Arc::clone(&self.last_auth_result);

        let (res_str, dist, tier) = self.rt_handle.spawn_blocking(move || {
            let scrfd_path = models_dir.join("scrfd_500m_kps.onnx");
            let mfn_path = models_dir.join("mobile_facenet.onnx");
            let minifas_path = models_dir.join("MiniFASNetV2.onnx");

            if !scrfd_path.exists() || !mfn_path.exists() {
                return ("DENIED".to_string(), 1.0, 4);
            }

            let detector = match ScrfdDetector::new_with_input_size(
                scrfd_path.to_str().unwrap(),
                config.detection.score_threshold,
                config.detection.nms_threshold,
                config.detection.min_face_size_px,
                config.detection.scrfd_input_size,
            ) {
                Ok(d) => d,
                Err(_) => return ("DENIED".to_string(), 1.0, 4),
            };

            let embedder = match MobileFaceNet::new(mfn_path.to_str().unwrap()) {
                Ok(e) => e,
                Err(_) => return ("DENIED".to_string(), 1.0, 4),
            };

            let spoof = if minifas_path.exists() {
                SpoofDetector::new(
                    minifas_path.to_str().unwrap(),
                    "/var/lib/sentinel/minifas_calib.json",
                    config.security.spoof_threshold,
                )
                .ok()
            } else {
                None
            };

            let mut authenticator = SentinelAuthenticator::new_with_config(
                detector,
                embedder,
                gallery,
                username.clone(),
                spoof,
                config.clone(),
            );

            let mut capture = match FrameCapture::new(&config.camera.source) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[DBus Authenticate] FrameCapture init error: {}", e);
                    return ("DENIED".to_string(), 1.0, 4);
                }
            };

            if let Err(e) = capture.start() {
                eprintln!("[DBus Authenticate] FrameCapture start error: {}", e);
                return ("DENIED".to_string(), 1.0, 4);
            }

            let start_time = Instant::now();
            let timeout_secs = config.security.challenge_timeout_secs.max(20.0);

            loop {
                if start_time.elapsed().as_secs_f64() > timeout_secs {
                    capture.stop();
                    return ("TIMEOUT".to_string(), 1.0, 4);
                }

                let captured = match capture.read_captured_frame() {
                    Some(f) => f,
                    None => {
                        thread::sleep(Duration::from_millis(15));
                        continue;
                    }
                };

                // Skip initial dark camera warmup frames (auto-exposure settling)
                if captured.luma < 15.0 {
                    thread::sleep(Duration::from_millis(20));
                    continue;
                }

                match authenticator.process_frame(&captured.image) {
                    Ok(r) => match r.state {
                        AuthState::Success => {
                            capture.stop();
                            let dist = r.distance.unwrap_or(0.0) as f64;
                            let tier = match r.active_tier {
                                Some(ActiveTier::Golden) => 1,
                                Some(ActiveTier::Standard) => 2,
                                Some(ActiveTier::TwoFactor) => 3,
                                None => 4,
                            };
                            return ("GRANTED".to_string(), dist, tier);
                        }
                        AuthState::Failure => {
                            capture.stop();
                            let dist = r.distance.unwrap_or(1.0) as f64;
                            let is_spoof = r.message.to_lowercase().contains("spoof");
                            let res = if is_spoof { "SPOOF" } else { "DENIED" };
                            return (res.to_string(), dist, 4);
                        }
                        AuthState::Require2FA => {
                            capture.stop();
                            let dist = r.distance.unwrap_or(0.45) as f64;
                            return ("REQUIRE_2FA".to_string(), dist, 3);
                        }
                        _ => {}
                    },
                    Err(_) => {
                        thread::sleep(Duration::from_millis(33));
                    }
                }

                thread::sleep(Duration::from_millis(30));
            }
        })
        .await
        .map_err(|e| zbus::fdo::Error::Failed(format!("Auth task error: {}", e)))?;

        *last_auth_res.lock().unwrap() = format!("{} (d={:.4}, tier={})", res_str, dist, tier);
        let _ = Self::auth_status_changed(&ctxt, &res_str, &format!("Auth complete: {}", res_str)).await;
        Ok((res_str, dist, tier))
    }

    async fn start_enrollment(
        &self,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        username: String,
    ) -> zbus::fdo::Result<String> {
        check_polkit(conn, &header, "com.sentinel.enroll").await?;

        let session_id = format!("enroll_{}_{}", username, Instant::now().elapsed().as_millis());
        let session = EnrollmentSession {
            session_id: session_id.clone(),
            username,
            pose_index: 0,
            total_poses: 30,
            collected_embeddings: Vec::new(),
        };

        *self.active_enrollment.lock().unwrap() = Some(session);
        Ok(session_id)
    }

    async fn submit_enrollment_frame(
        &self,
        #[zbus(header)] _header: zbus::MessageHeader<'_>,
        session_id: String,
    ) -> zbus::fdo::Result<(String, i32, i32, Vec<f64>)> {
        let (_pose_idx, _total_poses) = {
            let lock = self.active_enrollment.lock().unwrap();
            match lock.as_ref() {
                Some(s) if s.session_id == session_id => {
                    if s.pose_index >= s.total_poses {
                        return Ok(("COMPLETE".to_string(), s.pose_index as i32, s.total_poses as i32, Vec::new()));
                    }
                    (s.pose_index, s.total_poses)
                }
                _ => return Ok(("NO_SESSION".to_string(), 0, 30, Vec::new())),
            }
        };

        let config = self.config.lock().unwrap().clone();
        let models_dir = self.models_dir.clone();

        let res = self.rt_handle.spawn_blocking(move || {
            let scrfd_path = models_dir.join("scrfd_500m_kps.onnx");
            let mfn_path = models_dir.join("mobile_facenet.onnx");

            let mut detector = match ScrfdDetector::new_with_input_size(
                scrfd_path.to_str().unwrap(),
                0.35,
                config.detection.nms_threshold,
                config.detection.min_face_size_px,
                640,
            ) {
                Ok(d) => d,
                Err(_) => return ("NO_FACE".to_string(), None, Vec::new()),
            };

            let mut embedder = match MobileFaceNet::new(mfn_path.to_str().unwrap()) {
                Ok(e) => e,
                Err(_) => return ("NO_FACE".to_string(), None, Vec::new()),
            };

            let mut capture = match FrameCapture::new(&config.camera.source) {
                Ok(c) => c,
                Err(_) => return ("NO_FACE".to_string(), None, Vec::new()),
            };

            if capture.start().is_err() {
                return ("NO_FACE".to_string(), None, Vec::new());
            }

            let start = Instant::now();
            let mut frame_opt: Option<RgbImage> = None;
            while start.elapsed() < Duration::from_millis(500) {
                if let Some(f) = capture.read_frame() {
                    frame_opt = Some(f);
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
            capture.stop();

            let frame = match frame_opt {
                Some(f) => f,
                None => return ("NO_FACE".to_string(), None, Vec::new()),
            };

            let det_res = match detector.detect_detailed(&frame) {
                Ok(d) => d,
                Err(_) => return ("NO_FACE".to_string(), None, Vec::new()),
            };

            if det_res.detections.len() > 1 {
                return ("MULTIPLE_FACES".to_string(), None, Vec::new());
            }

            if det_res.detections.is_empty() {
                return ("NO_FACE".to_string(), None, Vec::new());
            }

            let det = &det_res.detections[0];
            let bw = (det.bbox[2] - det.bbox[0]).max(0.0);
            if bw < config.detection.min_face_size_px as f32 {
                return ("FACE_TOO_SMALL".to_string(), None, Vec::new());
            }

            let mut lm_vec = Vec::with_capacity(10);
            for p in &det.landmarks {
                lm_vec.push(p[0] as f64);
                lm_vec.push(p[1] as f64);
            }

            if let Ok(aligned) = align_face(&frame, &det.landmarks) {
                if let Ok(emb) = embedder.embed(&aligned) {
                    return ("ACCEPTED".to_string(), Some(emb), lm_vec);
                }
            }

            ("NO_FACE".to_string(), None, lm_vec)
        })
        .await
        .map_err(|e| zbus::fdo::Error::Failed(format!("Task panic: {}", e)))?;

        let (status_str, emb_opt, lm_vec) = res;
        let mut lock = self.active_enrollment.lock().unwrap();
        if let Some(s) = lock.as_mut().filter(|s| s.session_id == session_id) {
            if let Some(emb) = emb_opt {
                // Daemon-side diversity check: cosine distance > 0.05 against existing embeddings
                let is_too_similar = s.collected_embeddings.iter().any(|existing| {
                    let dot: f32 = existing.iter().zip(emb.iter()).map(|(a, b)| a * b).sum();
                    let cos_dist = 1.0 - dot;
                    cos_dist <= 0.05
                });

                if is_too_similar {
                    return Ok(("TOO_SIMILAR".to_string(), s.pose_index as i32, s.total_poses as i32, lm_vec));
                }

                s.collected_embeddings.push(emb);
                s.pose_index += 1;
                if s.pose_index >= s.total_poses {
                    return Ok(("COMPLETE".to_string(), s.pose_index as i32, s.total_poses as i32, lm_vec));
                }
            }
            Ok((status_str, s.pose_index as i32, s.total_poses as i32, lm_vec))
        } else {
            Ok(("NO_SESSION".to_string(), 0, 30, Vec::new()))
        }
    }

    async fn submit_enrollment_frame_data(
        &self,
        #[zbus(header)] _header: zbus::MessageHeader<'_>,
        session_id: String,
        frame_data: Vec<u8>,
    ) -> zbus::fdo::Result<(String, i32, i32, Vec<f64>)> {
        let (_pose_idx, _total_poses) = {
            let lock = self.active_enrollment.lock().unwrap();
            match lock.as_ref() {
                Some(s) if s.session_id == session_id => {
                    if s.pose_index >= s.total_poses {
                        return Ok(("COMPLETE".to_string(), s.pose_index as i32, s.total_poses as i32, Vec::new()));
                    }
                    (s.pose_index, s.total_poses)
                }
                _ => return Ok(("NO_SESSION".to_string(), 0, 30, Vec::new())),
            }
        };

        let config = self.config.lock().unwrap().clone();
        let models_dir = self.models_dir.clone();

        let res = self.rt_handle.spawn_blocking(move || {
            let scrfd_path = models_dir.join("scrfd_500m_kps.onnx");
            let mfn_path = models_dir.join("mobile_facenet.onnx");

            let img = match image::load_from_memory(&frame_data) {
                Ok(i) => i.to_rgb8(),
                Err(_) => return ("DECODE_ERROR".to_string(), None, Vec::new()),
            };

            let mut detector = match ScrfdDetector::new_with_input_size(
                scrfd_path.to_str().unwrap(),
                0.5,
                config.detection.nms_threshold,
                config.detection.min_face_size_px,
                640,
            ) {
                Ok(d) => d,
                Err(_) => return ("NO_FACE".to_string(), None, Vec::new()),
            };

            let mut embedder = match MobileFaceNet::new(mfn_path.to_str().unwrap()) {
                Ok(e) => e,
                Err(_) => return ("NO_FACE".to_string(), None, Vec::new()),
            };

            let det_res = match detector.detect_detailed(&img) {
                Ok(d) => d,
                Err(_) => return ("NO_FACE".to_string(), None, Vec::new()),
            };

            if det_res.detections.len() > 1 {
                return ("MULTIPLE_FACES".to_string(), None, Vec::new());
            }

            if det_res.detections.is_empty() {
                return ("NO_FACE".to_string(), None, Vec::new());
            }

            let det = &det_res.detections[0];

            let mut bbox_lm_vec = Vec::with_capacity(14);
            bbox_lm_vec.push(det.bbox[0] as f64);
            bbox_lm_vec.push(det.bbox[1] as f64);
            bbox_lm_vec.push(det.bbox[2] as f64);
            bbox_lm_vec.push(det.bbox[3] as f64);
            for p in &det.landmarks {
                bbox_lm_vec.push(p[0] as f64);
                bbox_lm_vec.push(p[1] as f64);
            }

            if let Ok(aligned) = align_face(&img, &det.landmarks) {
                if let Ok(emb) = embedder.embed(&aligned) {
                    return ("ACCEPTED".to_string(), Some(emb), bbox_lm_vec);
                }
            }

            ("NO_FACE".to_string(), None, bbox_lm_vec)
        })
        .await
        .map_err(|e| zbus::fdo::Error::Failed(format!("Task panic: {}", e)))?;

        let (status_str, emb_opt, bbox_lm_vec) = res;
        let mut lock = self.active_enrollment.lock().unwrap();
        if let Some(s) = lock.as_mut().filter(|s| s.session_id == session_id) {
            if let Some(emb) = emb_opt {
                s.collected_embeddings.push(emb);
                s.pose_index += 1;
                if s.pose_index >= s.total_poses {
                    return Ok(("COMPLETE".to_string(), s.pose_index as i32, s.total_poses as i32, bbox_lm_vec));
                }
            }
            Ok((status_str, s.pose_index as i32, s.total_poses as i32, bbox_lm_vec))
        } else {
            Ok(("NO_SESSION".to_string(), 0, 30, Vec::new()))
        }
    }

    async fn finish_enrollment(
        &self,
        #[zbus(header)] _header: zbus::MessageHeader<'_>,
        session_id: String,
    ) -> zbus::fdo::Result<(bool, String)> {
        let session = {
            let mut lock = self.active_enrollment.lock().unwrap();
            match lock.take() {
                Some(s) if s.session_id == session_id => s,
                _ => return Ok((false, "Session not found or expired".to_string())),
            }
        };

        if session.collected_embeddings.is_empty() {
            return Ok((false, "No embeddings collected".to_string()));
        }

        let store = GalleryStore::new(&session.username);
        if let Err(e) = store.save_core(&session.collected_embeddings) {
            return Ok((false, format!("Failed to save gallery: {}", e)));
        }

        Ok((
            true,
            format!(
                "Successfully enrolled user '{}' with {} vectors",
                session.username,
                session.collected_embeddings.len()
            ),
        ))
    }

    async fn cancel_enrollment(
        &self,
        #[zbus(header)] _header: zbus::MessageHeader<'_>,
        session_id: String,
    ) -> zbus::fdo::Result<()> {
        let mut lock = self.active_enrollment.lock().unwrap();
        if let Some(ref s) = *lock {
            if s.session_id == session_id {
                *lock = None;
            }
        }
        Ok(())
    }

    async fn list_users(&self) -> zbus::fdo::Result<Vec<String>> {
        let gallery_dir = PathBuf::from("/var/lib/sentinel/users");
        let mut users = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&gallery_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        users.push(name.to_string());
                    }
                }
            }
        }
        Ok(users)
    }

    async fn remove_user(
        &self,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        username: String,
    ) -> zbus::fdo::Result<bool> {
        check_polkit(conn, &header, "com.sentinel.remove_user").await?;

        let user_dir = PathBuf::from("/var/lib/sentinel/users").join(&username);
        if user_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&user_dir) {
                eprintln!("[DBus RemoveUser] Error removing {}: {}", user_dir.display(), e);
                return Ok(false);
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn get_user_info(&self, username: String) -> zbus::fdo::Result<String> {
        let meta_path = PathBuf::from("/var/lib/sentinel/users").join(&username).join("meta.json");
        if meta_path.exists() {
            match std::fs::read_to_string(&meta_path) {
                Ok(content) => Ok(content),
                Err(e) => Err(zbus::fdo::Error::Failed(format!("Failed to read metadata: {}", e))),
            }
        } else {
            let default_meta = serde_json::json!({
                "username": username,
                "core_vector_count": 0,
                "adaptive_vector_count": 0,
                "last_adaptation_date": "N/A",
                "enrolled_at": "N/A"
            });
            Ok(default_meta.to_string())
        }
    }


    async fn get_config(&self) -> zbus::fdo::Result<String> {
        let config = self.config.lock().unwrap();
        toml::to_string(&*config)
            .map_err(|e| zbus::fdo::Error::Failed(format!("TOML serialize error: {}", e)))
    }

    async fn set_config(
        &self,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        config_toml: String,
    ) -> zbus::fdo::Result<(bool, String)> {
        check_polkit(conn, &header, "com.sentinel.set_config").await?;

        let new_config: SentinelConfig = match toml::from_str(&config_toml) {
            Ok(c) => c,
            Err(e) => return Ok((false, format!("Invalid TOML: {}", e))),
        };

        if let Err(e) = std::fs::write(&self.config_path, &config_toml) {
            return Ok((false, format!("Failed to write config file: {}", e)));
        }

        *self.config.lock().unwrap() = new_config;
        Ok((true, "Configuration updated successfully".to_string()))
    }

    async fn get_status(&self) -> zbus::fdo::Result<String> {
        let uptime = self.start_time.elapsed().as_secs();
        let scrfd_loaded = self.models_dir.join("scrfd_500m_kps.onnx").exists();
        let mfn_loaded = self.models_dir.join("mobile_facenet.onnx").exists();
        let spoof_loaded = self.models_dir.join("MiniFASNetV2.onnx").exists();

        let gallery_dir = PathBuf::from("/var/lib/sentinel/users");
        let mut enrolled_users_count = 0usize;
        if let Ok(entries) = std::fs::read_dir(&gallery_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    enrolled_users_count += 1;
                }
            }
        }

        let config = self.config.lock().unwrap();
        let last_res = self.last_auth_result.lock().unwrap().clone();

        let status_json = serde_json::json!({
            "daemon_uptime_secs": uptime,
            "models_loaded": {
                "scrfd_500m_kps": scrfd_loaded,
                "mobile_facenet": mfn_loaded,
                "minifasnetv2": spoof_loaded
            },
            "enrolled_users_count": enrolled_users_count,
            "camera_source": config.camera.source,
            "last_auth_result": last_res
        });

        Ok(status_json.to_string())
    }

    async fn get_intrusion_list(
        &self,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<Vec<String>> {
        check_polkit(conn, &header, "com.sentinel.get_intrusions").await?;

        let dir = PathBuf::from("/var/lib/sentinel/blacklist");
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.path().is_file() {
                    if let Some(name) = entry.file_name().to_str() {
                        if name.ends_with(".jpg") || name.starts_with("intrusion_") {
                            files.push(name.to_string());
                        }
                    }
                }
            }
        }
        Ok(files)
    }

    async fn dismiss_intrusion(
        &self,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        filename: String,
    ) -> zbus::fdo::Result<()> {
        check_polkit(conn, &header, "com.sentinel.get_intrusions").await?;

        let file_path = PathBuf::from("/var/lib/sentinel/blacklist").join(filename);
        if file_path.exists() {
            let _ = std::fs::remove_file(file_path);
        }
        Ok(())
    }

    /// Reset anti-spoof calibration by deleting /var/lib/sentinel/minifas_calib.json
    async fn reset_spoof_calibration(
        &self,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<bool> {
        check_polkit(conn, &header, "com.sentinel.reset_calibration").await?;

        let calib_path = std::path::Path::new("/var/lib/sentinel/minifas_calib.json");
        if calib_path.exists() {
            std::fs::remove_file(calib_path)
                .map_err(|e| zbus::fdo::Error::Failed(format!("Failed to delete calibration file: {}", e)))?;
        }
        println!("[DBus] ResetSpoofCalibration: Deleted /var/lib/sentinel/minifas_calib.json");
        Ok(true)
    }

    /// Open camera, capture ~80 frames, run MiniFASNet self-calibration loop, save result, and return JSON string
    async fn run_spoof_calibration(
        &self,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<String> {
        check_polkit(conn, &header, "com.sentinel.reset_calibration").await?;

        let config = self.config.lock().unwrap().clone();
        let models_dir = self.models_dir.clone();

        let res_json = self.rt_handle.spawn_blocking(move || {
            let scrfd_path = models_dir.join("scrfd_500m_kps.onnx");
            let minifas_path = models_dir.join("MiniFASNetV2.onnx");
            let calib_path = "/var/lib/sentinel/minifas_calib.json";

            let _ = std::fs::remove_file(calib_path);

            if !scrfd_path.exists() || !minifas_path.exists() {
                return Err(zbus::fdo::Error::Failed("Required ONNX models missing".to_string()));
            }

            let mut detector = ScrfdDetector::new_with_input_size(
                scrfd_path.to_str().unwrap(),
                config.detection.score_threshold,
                config.detection.nms_threshold,
                config.detection.min_face_size_px,
                config.detection.scrfd_input_size,
            ).map_err(|e| zbus::fdo::Error::Failed(format!("Detector init error: {}", e)))?;

            let mut spoof = SpoofDetector::new(
                minifas_path.to_str().unwrap(),
                calib_path,
                config.security.spoof_threshold,
            ).map_err(|e| zbus::fdo::Error::Failed(format!("SpoofDetector init error: {}", e)))?;

            let mut capture = FrameCapture::new(&config.camera.source)
                .map_err(|e| zbus::fdo::Error::Failed(format!("Camera init error: {}", e)))?;
            capture.start().map_err(|e| zbus::fdo::Error::Failed(format!("Camera start error: {}", e)))?;

            let start = Instant::now();
            let timeout = Duration::from_secs(40);

            while spoof.is_calibrating() && start.elapsed() < timeout {
                let captured = match capture.read_captured_frame() {
                    Some(f) => f,
                    None => {
                        thread::sleep(Duration::from_millis(15));
                        continue;
                    }
                };

                if captured.luma < 15.0 {
                    thread::sleep(Duration::from_millis(20));
                    continue;
                }

                if let Ok(detections) = detector.detect(&captured.image) {
                    if !detections.is_empty() {
                        let bbox = detections[0].bbox;
                        if let Ok(crop) = SpoofDetector::square_crop(&captured.image, bbox, 1.5) {
                            spoof.calibrate_tick(&crop);
                        }
                    }
                }
                thread::sleep(Duration::from_millis(20));
            }

            capture.stop();

            if std::path::Path::new(calib_path).exists() {
                let json_content = std::fs::read_to_string(calib_path).unwrap_or_else(|_| "{}".to_string());
                Ok(json_content)
            } else {
                Err(zbus::fdo::Error::Failed("Calibration timed out or failed to save".to_string()))
            }
        }).await.map_err(|e| zbus::fdo::Error::Failed(format!("Spawn error: {}", e)))??;

        Ok(res_json)
    }

    #[zbus(signal)]
    async fn auth_status_changed(
        ctxt: &zbus::SignalContext<'_>,
        status: &str,
        message: &str,
    ) -> zbus::Result<()>;
}
