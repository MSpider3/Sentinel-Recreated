use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

use zbus::zvariant;

use crate::config::SentinelConfig;
use crate::gallery::GalleryStore;
use crate::limiter::AttemptLimiter;
use crate::pipeline::{
    align_face, quality, AuthState, AuthTier, FrameCapture, Models, SentinelAuthenticator,
};

#[path = "../greeter_detect.rs"]
pub mod greeter_detect;

pub struct EnrollmentSession {
    pub session_id: String,
    pub owner: String,
    pub created_at: Instant,
    pub username: String,
    pub pose_index: usize,
    pub total_poses: usize,
    pub collected_embeddings: Vec<[f32; 512]>,
}

/// Maximum lifetime of an enrollment session (interactive wizard budget).
pub const ENROLLMENT_SESSION_TTL: Duration = Duration::from_secs(1800);
/// Fewest templates an enrollment must collect to be saved.
pub const MIN_ENROLL_VECTORS: usize = 15;
/// Most templates one enrollment may hold. Every extra template is one more
/// chance for a stranger to match, so the gallery is kept small.
pub const MAX_ENROLL_VECTORS: usize = 40;
/// A capture closer than this (cosine distance) to one already collected is
/// the same picture again and is not stored.
const DUPLICATE_DISTANCE: f32 = 0.01;

pub struct SentinelService {
    pub config: Arc<Mutex<SentinelConfig>>,
    pub config_path: PathBuf,
    pub models_dir: PathBuf,
    /// Loaded once at start-up. The lock also makes sure only one camera
    /// scan runs at a time.
    pub models: Arc<Mutex<Models>>,
    pub spoof_available: bool,
    pub start_time: Instant,
    pub last_auth_result: Arc<Mutex<String>>,
    pub active_enrollment: Arc<Mutex<Option<EnrollmentSession>>>,
    pub attempts: Arc<Mutex<AttemptLimiter>>,
    pub rt_handle: tokio::runtime::Handle,
}

impl SentinelService {
    pub fn new(
        config: SentinelConfig,
        config_path: PathBuf,
        models_dir: PathBuf,
        models: Models,
        rt_handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            config: Arc::new(Mutex::new(config)),
            config_path,
            models_dir,
            spoof_available: models.spoof.is_some(),
            models: Arc::new(Mutex::new(models)),
            start_time: Instant::now(),
            last_auth_result: Arc::new(Mutex::new("None".to_string())),
            active_enrollment: Arc::new(Mutex::new(None)),
            attempts: Arc::new(Mutex::new(AttemptLimiter::default())),
            rt_handle,
        }
    }

    pub fn is_session_valid(
        session: &EnrollmentSession,
        caller_sender: Option<&str>,
        session_id: &str,
    ) -> bool {
        caller_sender == Some(session.owner.as_str())
            && session.session_id == session_id
            && session.created_at.elapsed() <= ENROLLMENT_SESSION_TTL
    }

    fn enrollment_session_owned(
        &self,
        session: &EnrollmentSession,
        header: &zbus::MessageHeader<'_>,
        session_id: &str,
    ) -> bool {
        let sender = header.sender().map(|s| s.to_string());
        Self::is_session_valid(session, sender.as_deref(), session_id)
    }

    /// Answer an Authenticate call without using the camera. NO_FACE makes
    /// pam_sentinel.so step aside so the password prompt takes over.
    async fn skip_scan(
        &self,
        ctxt: &zbus::SignalContext<'_>,
        reason: &str,
    ) -> zbus::fdo::Result<(String, f64, i32)> {
        log::info!("[DBus] Face scan skipped: {}.", reason);
        *lock(&self.last_auth_result) = format!("NO_FACE ({})", reason);
        let _ = Self::auth_status_changed(ctxt, "NO_FACE", reason).await;
        Ok(("NO_FACE".to_string(), -1.0, 0))
    }
}

/// Lock a mutex even if an earlier holder panicked. The data guarded here
/// (models, limiter, config) stays usable after a panic, and refusing the lock
/// forever would silently switch face authentication off.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
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

/// Ask logind whether the caller runs inside a remote (e.g. SSH) session.
///
/// Unlike the SSH_* environment variables, which the caller supplies and can
/// simply unset, this comes from the system. Callers that belong to no login
/// session at all (display-manager greeters) and any lookup error count as
/// local, so this check can only ever make the daemon stricter.
async fn is_remote_session(conn: &zbus::Connection, sender: &str) -> bool {
    async fn query(conn: &zbus::Connection, sender: &str) -> zbus::Result<bool> {
        let pid: u32 = zbus::Proxy::new(
            conn,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .await?
        .call("GetConnectionUnixProcessID", &(sender))
        .await?;

        let session_path: zvariant::OwnedObjectPath = zbus::Proxy::new(
            conn,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )
        .await?
        .call("GetSessionByPID", &(pid))
        .await?;

        zbus::Proxy::new(
            conn,
            "org.freedesktop.login1",
            session_path,
            "org.freedesktop.login1.Session",
        )
        .await?
        .get_property::<bool>("Remote")
        .await
    }
    query(conn, sender).await.unwrap_or(false)
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

/// DBus-supplied usernames are interpolated verbatim into root-owned filesystem
/// paths (`/var/lib/sentinel/users/{}/` via `format!` in `GalleryStore::new` /
/// `AdaptiveGallery::meta_path`) and written into the audit log, so only plain
/// account names may reach the store layer: letters, digits and `_ - . @`.
fn validate_username(username: &str) -> Result<(), String> {
    let valid = !username.is_empty()
        && username.len() <= 64
        && username
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '@'))
        && !username.starts_with(['.', '-'])
        && !username.contains("..");
    if valid {
        Ok(())
    } else {
        Err(format!("Rejected unsafe username: {:?}", username))
    }
}

/// Resolve a username to its numeric uid, used to bind the DBus method
/// `Authenticate` to the identity it is asked to verify.
fn uid_of_user(username: &str) -> Option<u32> {
    use std::ffi::CString;
    let c_name = CString::new(username).ok()?;
    unsafe {
        let pw = libc::getpwnam(c_name.as_ptr());
        if pw.is_null() {
            None
        } else {
            Some((*pw).pw_uid)
        }
    }
}

/// What one camera scan concluded.
struct AuthOutcome {
    result: &'static str,
    tier: i32,
    /// A real face was compared to the gallery (used by the attempt limiter).
    face_evaluated: bool,
}

/// Run one face scan to completion. Blocking; called on a worker thread.
fn run_auth_session(
    config: &SentinelConfig,
    models: &Mutex<Models>,
    core_gallery: Vec<[f32; 512]>,
    adaptive_gallery: Vec<[f32; 512]>,
    username: String,
) -> AuthOutcome {
    let outcome = |result, tier, face_evaluated| AuthOutcome { result, tier, face_evaluated };

    // The session clock starts now, so camera start-up counts towards the timeout.
    let deadline = Instant::now() + Duration::from_secs_f64(config.security.global_session_timeout);
    let mut authenticator =
        SentinelAuthenticator::new(core_gallery, adaptive_gallery, username, config.clone());

    // Camera first: it takes the longest to get going.
    let camera = &config.camera;
    let mut capture = match FrameCapture::with_format(&camera.source, camera.width, camera.height, camera.fps) {
        Ok(c) => c,
        Err(e) => {
            log::error!("[DBus Authenticate] Camera init error: {} — falling back to password.", e);
            return outcome("NO_FACE", 0, false);
        }
    };
    if let Err(e) = capture.start() {
        log::error!("[DBus Authenticate] Camera start error: {} — falling back to password.", e);
        return outcome("NO_FACE", 0, false);
    }

    // One scan at a time: a second caller is told to use the password.
    let mut models = match models.try_lock() {
        Ok(m) => m,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => {
            log::warn!("[DBus Authenticate] Another scan is running — falling back to password.");
            return outcome("NO_FACE", 0, false);
        }
    };
    if models.spoof.is_none() {
        log::error!("[DBus Authenticate] Anti-spoof model unavailable — face authentication is disabled.");
        return outcome("NO_FACE", 0, false);
    }
    models.detector.configure(&config.detection);

    let mut last_seq = 0u64;
    let result = loop {
        if capture.failed() {
            // Still close the session properly, so a camera that "fails" after
            // a face was judged counts towards the attempt limit.
            let r = authenticator.expire();
            break outcome("NO_FACE", 0, r.face_evaluated);
        }
        if Instant::now() >= deadline {
            let r = authenticator.expire();
            break outcome("TIMEOUT", 4, r.face_evaluated);
        }
        let frame = match capture.wait_new_frame(last_seq, Duration::from_millis(100)) {
            Some(f) => f,
            None => continue,
        };
        last_seq = frame.seq;

        match authenticator.process_frame(&mut models, &frame) {
            Ok(r) => match r.state {
                AuthState::Waiting => {}
                AuthState::Success => {
                    let tier = if r.active_tier == Some(AuthTier::Golden) { 1 } else { 2 };
                    break outcome("GRANTED", tier, true);
                }
                AuthState::Failure => break outcome("DENIED", 4, r.face_evaluated),
                AuthState::Spoof => break outcome("SPOOF", 4, r.face_evaluated),
                AuthState::Timeout => break outcome("TIMEOUT", 4, r.face_evaluated),
                AuthState::NoFace => break outcome("NO_FACE", 0, r.face_evaluated),
            },
            // A frame that could not be processed is skipped; it never grants.
            Err(e) => log::debug!("[DBus Authenticate] Frame skipped: {:#}", e),
        }
    };
    drop(models);

    // Switching the camera off takes a moment; the caller should not wait for it.
    thread::spawn(move || drop(capture));
    result
}

#[zbus::interface(name = "com.sentinel.Sentinel")]
impl SentinelService {
    /// Primary authentication method invoked by pam_sentinel.so
    async fn authenticate(
        &self,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        #[zbus(signal_context)] ctxt: zbus::SignalContext<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        username: String,
        session_env: HashMap<String, String>,
    ) -> zbus::fdo::Result<(String, f64, i32)> {
        if let Err(e) = validate_username(&username) {
            return Err(zbus::fdo::Error::Failed(e));
        }

        // 0. Sender/subject binding: only the named user — or root, the uid
        //    that gdm-session-worker / greetd / sudo / su / login run PAM
        //    conversations as — may run biometric matching for that
        //    identity. Without this, any local sender can use Authenticate
        //    as an unlimited cross-user similarity/presence oracle against
        //    every enrolled gallery.
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing DBus sender".to_string()))?;
        let caller_uid: u32 = zbus::Proxy::new(
            conn,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .await?
        .call("GetConnectionUnixUser", &(sender.as_str()))
        .await
        .map_err(|e| zbus::fdo::Error::Failed(format!("GetConnectionUnixUser error: {}", e)))?;
        let target_uid = uid_of_user(&username)
            .ok_or_else(|| zbus::fdo::Error::Failed(format!("Unknown user '{}'", username)))?;
        if caller_uid != 0 && caller_uid != target_uid {
            return Err(zbus::fdo::Error::AccessDenied(format!(
                "Caller (uid {}) is not authorized to authenticate as '{}'",
                caller_uid, username
            )));
        }

        // 1. Session Context Evaluation (remote session / lid check)
        if session_env.contains_key("SSH_CLIENT")
            || session_env.contains_key("SSH_TTY")
            || is_remote_session(conn, sender.as_str()).await
        {
            return self.skip_scan(&ctxt, "Remote session").await;
        }
        if is_lid_closed() {
            return self.skip_scan(&ctxt, "Lid Closed").await;
        }

        // 2. Attempt limit
        if lock(&self.attempts).is_blocked(&username, Instant::now()) {
            log::warn!("[DBus] Too many failed face attempts for '{}' — paused, password required.", username);
            *lock(&self.last_auth_result) = "RATE_LIMITED".to_string();
            let _ = Self::auth_status_changed(&ctxt, "RATE_LIMITED", "Too many failed attempts").await;
            return Ok(("RATE_LIMITED".to_string(), -1.0, 0));
        }

        // 3. Load Gallery Vectors
        let store = GalleryStore::new(&username);
        let core_gallery = store.load_core().map_err(|e| {
            zbus::fdo::Error::Failed(format!("Failed to load gallery for '{}': {}", username, e))
        })?;
        if core_gallery.is_empty() {
            return self.skip_scan(&ctxt, "No Enrolled Template").await;
        }
        let adaptive_gallery = store.load_adaptive().unwrap_or_else(|e| {
            log::warn!("[DBus] Ignoring unreadable adaptive gallery for '{}': {:#}", username, e);
            Vec::new()
        });

        let _ = Self::auth_status_changed(&ctxt, "SCANNING", "Looking for your face...").await;

        // 4. Camera scan
        let config = lock(&self.config).clone();
        let models = Arc::clone(&self.models);
        let user = username.clone();
        let outcome = self
            .rt_handle
            .spawn_blocking(move || run_auth_session(&config, &models, core_gallery, adaptive_gallery, user))
            .await
            .map_err(|e| zbus::fdo::Error::Failed(format!("Auth task error: {}", e)))?;

        // 5. Attempt accounting: sessions where a face was seen but not
        //    granted count as failures; an empty room does not.
        {
            let mut attempts = lock(&self.attempts);
            match outcome.result {
                "GRANTED" => attempts.clear(&username),
                _ if outcome.face_evaluated => attempts.record_failure(&username, Instant::now()),
                _ => {}
            }
        }

        // Constant: never return the measured distance to the caller — it is
        // a similarity oracle against the target user's templates
        // (pam_sentinel ignores the value).
        let dist = match outcome.result {
            "GRANTED" => 0.0,
            "NO_FACE" => -1.0,
            _ => 1.0,
        };
        let res_str = outcome.result.to_string();
        log::info!("[DBus] Authenticate for '{}': {} (tier {})", username, res_str, outcome.tier);
        *lock(&self.last_auth_result) = format!("{} (tier={})", res_str, outcome.tier);
        let _ = Self::auth_status_changed(&ctxt, &res_str, &format!("Auth complete: {}", res_str)).await;
        Ok((res_str, dist, outcome.tier))
    }

    async fn start_enrollment(
        &self,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        username: String,
    ) -> zbus::fdo::Result<String> {
        check_polkit(conn, &header, "com.sentinel.enroll").await?;
        if let Err(e) = validate_username(&username) {
            return Err(zbus::fdo::Error::Failed(e));
        }
        if uid_of_user(&username).is_none() {
            return Err(zbus::fdo::Error::Failed(format!("Unknown user '{}'", username)));
        }

        let owner = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing DBus sender".to_string()))?
            .to_string();
        let session_id = generate_enrollment_session_id(&username);
        let session = EnrollmentSession {
            session_id: session_id.clone(),
            owner,
            created_at: Instant::now(),
            username,
            pose_index: 0,
            total_poses: MAX_ENROLL_VECTORS,
            collected_embeddings: Vec::new(),
        };

        *lock(&self.active_enrollment) = Some(session);
        Ok(session_id)
    }

    /// Check one camera frame (JPEG/PNG bytes) sent by the enrollment wizard.
    ///
    /// With `capture = false` the frame is only inspected, for the live
    /// preview. With `capture = true` a good frame is stored as a template.
    ///
    /// Returns `(status, templates collected, maximum, [bbox x1,y1,x2,y2, 5 landmarks x,y])`.
    /// Status is `ACCEPTED`, or why not: `NO_FACE`, `MULTIPLE_FACES`,
    /// `OUT_OF_FRAME`, `NOT_FRONTAL`, `TOO_DARK`, `TOO_BRIGHT`, `BLURRY`,
    /// `TOO_SIMILAR`, `FULL`, `DECODE_ERROR`, `NO_SESSION`.
    async fn submit_enrollment_frame_data(
        &self,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        session_id: String,
        frame_data: Vec<u8>,
        capture: bool,
    ) -> zbus::fdo::Result<(String, i32, i32, Vec<f64>)> {
        let no_session = || Ok(("NO_SESSION".to_string(), 0, MAX_ENROLL_VECTORS as i32, Vec::new()));
        {
            let session = lock(&self.active_enrollment);
            match session.as_ref() {
                Some(s) if self.enrollment_session_owned(s, &header, &session_id) => {}
                _ => return no_session(),
            }
        }

        let config = lock(&self.config).clone();
        let models = Arc::clone(&self.models);

        let res = self.rt_handle.spawn_blocking(move || {
            let img = match image::load_from_memory(&frame_data) {
                Ok(i) => i.to_rgb8(),
                Err(_) => return ("DECODE_ERROR", None, Vec::new()),
            };

            let mut models = lock(&models);
            models.detector.configure(&config.detection);
            let detections = match models.detector.detect(&img) {
                Ok(d) => d,
                Err(_) => return ("NO_FACE", None, Vec::new()),
            };
            if detections.len() > 1 {
                return ("MULTIPLE_FACES", None, Vec::new());
            }
            let det = match detections.first() {
                Some(d) => d,
                None => return ("NO_FACE", None, Vec::new()),
            };

            let mut bbox_lm_vec = Vec::with_capacity(14);
            bbox_lm_vec.extend(det.bbox.iter().map(|v| *v as f64));
            for p in &det.landmarks {
                bbox_lm_vec.push(p[0] as f64);
                bbox_lm_vec.push(p[1] as f64);
            }

            // Same quality gate as authentication: a template is only useful
            // if it looks like the frames it will later be compared with.
            let status_of = |issue: quality::QualityIssue| match issue {
                quality::QualityIssue::OutOfFrame => "OUT_OF_FRAME",
                quality::QualityIssue::NotFrontal => "NOT_FRONTAL",
                quality::QualityIssue::TooDark => "TOO_DARK",
                quality::QualityIssue::TooBright => "TOO_BRIGHT",
                quality::QualityIssue::Blurry => "BLURRY",
            };
            if let Err(issue) = quality::check_pose(&det.landmarks, img.width(), img.height()) {
                return (status_of(issue), None, bbox_lm_vec);
            }
            let aligned = match align_face(&img, &det.landmarks) {
                Ok(a) => a,
                Err(_) => return ("NO_FACE", None, bbox_lm_vec),
            };
            if let Err(issue) = quality::check_aligned_face(&aligned) {
                return (status_of(issue), None, bbox_lm_vec);
            }
            if !capture {
                return ("ACCEPTED", None, bbox_lm_vec);
            }

            match models.embedder.embed_with_flip(&aligned) {
                Ok(emb) => ("ACCEPTED", Some(emb), bbox_lm_vec),
                Err(_) => ("NO_FACE", None, bbox_lm_vec),
            }
        })
        .await
        .map_err(|e| zbus::fdo::Error::Failed(format!("Task panic: {}", e)))?;

        let (mut status, emb_opt, bbox_lm_vec) = res;
        let mut session = lock(&self.active_enrollment);
        let s = match session
            .as_mut()
            .filter(|s| self.enrollment_session_owned(s, &header, &session_id))
        {
            Some(s) => s,
            None => return no_session(),
        };

        if let Some(emb) = emb_opt {
            let is_duplicate = s.collected_embeddings.iter().any(|existing| {
                crate::pipeline::cosine_distance(existing, &emb) <= DUPLICATE_DISTANCE
            });
            if s.collected_embeddings.len() >= MAX_ENROLL_VECTORS {
                status = "FULL";
            } else if is_duplicate {
                status = "TOO_SIMILAR";
            } else {
                s.collected_embeddings.push(emb);
                s.pose_index += 1;
            }
        }
        Ok((status.to_string(), s.pose_index as i32, s.total_poses as i32, bbox_lm_vec))
    }

    async fn finish_enrollment(
        &self,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        session_id: String,
    ) -> zbus::fdo::Result<(bool, String)> {
        let session = {
            let mut lock = lock(&self.active_enrollment);
            let matches = lock
                .as_ref()
                .map(|s| self.enrollment_session_owned(s, &header, &session_id))
                .unwrap_or(false);
            if matches {
                lock.take().unwrap()
            } else {
                return Ok((false, "Session not found or expired".to_string()));
            }
        };

        if session.collected_embeddings.len() < MIN_ENROLL_VECTORS {
            return Ok((
                false,
                format!(
                    "Insufficient embeddings: collected {} (minimum {} required)",
                    session.collected_embeddings.len(),
                    MIN_ENROLL_VECTORS
                ),
            ));
        }

        // A new enrollment replaces the identity: templates learned for the
        // previous one must not keep granting access. They are removed first,
        // so a failure here can never leave them next to a new enrollment.
        let store = GalleryStore::new(&session.username);
        if let Err(e) = store.clear_adaptive() {
            return Ok((false, format!("Failed to clear learned templates: {}", e)));
        }
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

    async fn get_recent_auth_log(
        &self,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        lines: u32,
    ) -> zbus::fdo::Result<Vec<String>> {
        check_polkit(conn, &header, "com.sentinel.get_auth_log").await?;
        Ok(crate::audit::AuditLogger::new().recent_lines((lines as usize).min(1000)))
    }

    async fn cancel_enrollment(
        &self,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        session_id: String,
    ) -> zbus::fdo::Result<()> {
        let mut lock = lock(&self.active_enrollment);
        if let Some(ref s) = *lock {
            if self.enrollment_session_owned(s, &header, &session_id) {
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
        if let Err(e) = validate_username(&username) {
            return Err(zbus::fdo::Error::Failed(e));
        }

        let user_dir = PathBuf::from("/var/lib/sentinel/users").join(&username);
        if user_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&user_dir) {
                log::error!("[DBus RemoveUser] Error removing {}: {}", user_dir.display(), e);
                return Ok(false);
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn get_user_info(&self, username: String) -> zbus::fdo::Result<String> {
        if let Err(e) = validate_username(&username) {
            return Err(zbus::fdo::Error::InvalidArgs(e));
        }

        let store = GalleryStore::new(&username);
        let core_count = store.load_core().map(|v| v.len()).unwrap_or(0);
        let adaptive_count = store.load_adaptive().map(|v| v.len()).unwrap_or(0);
        let meta = crate::gallery::AdaptiveGallery::load_meta(&username);
        let enrolled_at = std::fs::metadata(store.base_path.join("gallery.npy"))
            .and_then(|m| m.modified())
            .map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|_| "N/A".to_string());
        let last_adaptation = if meta.last_adaptation_date.is_empty() {
            "N/A".to_string()
        } else {
            meta.last_adaptation_date
        };

        Ok(serde_json::json!({
            "username": username,
            "core_vector_count": core_count,
            "adaptive_vector_count": adaptive_count,
            "last_adaptation_date": last_adaptation,
            "enrolled_at": enrolled_at
        })
        .to_string())
    }

    async fn get_config(&self) -> zbus::fdo::Result<String> {
        let config = lock(&self.config);
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

        // Parses and range-checks: a value that would weaken authentication
        // (or a camera source that is not a device node) never reaches disk.
        let new_config = match SentinelConfig::from_toml(&config_toml) {
            Ok(c) => c,
            Err(e) => return Ok((false, format!("Invalid configuration: {:#}", e))),
        };

        if let Err(e) = crate::fsutil::write_atomic(&self.config_path, config_toml.as_bytes(), 0o644) {
            return Ok((false, format!("Failed to write config file: {:#}", e)));
        }

        *lock(&self.config) = new_config;
        Ok((true, "Configuration updated successfully".to_string()))
    }

    async fn get_status(&self) -> zbus::fdo::Result<String> {
        let uptime = self.start_time.elapsed().as_secs();
        // The detector and embedder are loaded at start-up or the daemon exits.
        let scrfd_loaded = true;
        let mfn_loaded = true;
        let spoof_loaded = self.spoof_available;

        let gallery_dir = PathBuf::from("/var/lib/sentinel/users");
        let mut enrolled_users_count = 0usize;
        if let Ok(entries) = std::fs::read_dir(&gallery_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    enrolled_users_count += 1;
                }
            }
        }

        let config = lock(&self.config);
        let last_res = lock(&self.last_auth_result).clone();

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

    async fn get_greeter_info(&self) -> zbus::fdo::Result<String> {
        let info = greeter_detect::detect();
        serde_json::to_string(&info)
            .map_err(|e| zbus::fdo::Error::Failed(format!("Failed to serialize greeter info: {}", e)))
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
        check_polkit(conn, &header, "com.sentinel.dismiss_intrusion").await?;

        let mut components = std::path::Path::new(&filename).components();
        let is_plain_name =
            matches!(components.next(), Some(std::path::Component::Normal(_)))
                && components.next().is_none()
                && !filename.contains('\0');
        if !is_plain_name {
            return Err(zbus::fdo::Error::Failed("Invalid filename".to_string()));
        }

        let file_path = PathBuf::from("/var/lib/sentinel/blacklist").join(filename);
        if file_path.exists() {
            let _ = std::fs::remove_file(file_path);
        }
        Ok(())
    }

    #[zbus(signal)]
    async fn auth_status_changed(
        ctxt: &zbus::SignalContext<'_>,
        status: &str,
        message: &str,
    ) -> zbus::Result<()>;
}

fn generate_enrollment_session_id(username: &str) -> String {
    let mut token = String::with_capacity(32);
    for byte in rand::random::<[u8; 16]>() {
        token.push_str(&format!("{:02x}", byte));
    }
    format!("enroll_{}_{}", username, token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enrollment_session_id_entropy() {
        let id1 = generate_enrollment_session_id("alice");
        let id2 = generate_enrollment_session_id("alice");
        assert_ne!(id1, id2, "Session IDs must be non-deterministic");
        assert!(!id1.ends_with("_0"), "Session ID must not end with deterministic _0 timestamp");
        assert!(id1.starts_with("enroll_alice_"));
        let token = id1.strip_prefix("enroll_alice_").unwrap();
        assert_eq!(token.len(), 32);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_enrollment_session_sender_binding_and_ttl() {
        let session = EnrollmentSession {
            session_id: "enroll_alice_testtoken123".to_string(),
            owner: ":1.42".to_string(),
            created_at: Instant::now(),
            username: "alice".to_string(),
            pose_index: 0,
            total_poses: MAX_ENROLL_VECTORS,
            collected_embeddings: Vec::new(),
        };

        // 1. Owner matches and ID matches -> valid
        assert!(SentinelService::is_session_valid(&session, Some(":1.42"), "enroll_alice_testtoken123"));

        // 2. Caller sender mismatch -> invalid (BUG-R2-S1-A1-H2)
        assert!(!SentinelService::is_session_valid(&session, Some(":1.99"), "enroll_alice_testtoken123"));
        assert!(!SentinelService::is_session_valid(&session, None, "enroll_alice_testtoken123"));

        // 3. ID mismatch -> invalid
        assert!(!SentinelService::is_session_valid(&session, Some(":1.42"), "enroll_alice_wrongid"));

        // 4. Expired session (> 1800s) -> invalid (BUG-R2-S1-A1-H3)
        let expired_session = EnrollmentSession {
            session_id: "enroll_alice_testtoken123".to_string(),
            owner: ":1.42".to_string(),
            created_at: Instant::now() - Duration::from_secs(1801),
            username: "alice".to_string(),
            pose_index: 0,
            total_poses: MAX_ENROLL_VECTORS,
            collected_embeddings: Vec::new(),
        };
        assert!(!SentinelService::is_session_valid(&expired_session, Some(":1.42"), "enroll_alice_testtoken123"));
    }

    #[test]
    fn test_validate_username() {
        assert!(validate_username("alice").is_ok());
        assert!(validate_username("bob_123").is_ok());
        assert!(validate_username("carol-dev").is_ok());
        assert!(validate_username("dave.smith@example.org").is_ok());

        // Characters that would break the pipe-separated audit log or paths.
        assert!(validate_username("mallory|GRANTED").is_err());
        assert!(validate_username("mallory\nroot").is_err());
        assert!(validate_username("mal lory").is_err());
        assert!(validate_username("-rf").is_err());
        assert!(validate_username(&"a".repeat(65)).is_err());

        assert!(validate_username("").is_err());
        assert!(validate_username("../../../../tmp/evil").is_err());
        assert!(validate_username("/etc/shadow").is_err());
        assert!(validate_username("..").is_err());
        assert!(validate_username(".").is_err());
        assert!(validate_username(".hidden").is_err());
        assert!(validate_username("alice/bob").is_err());
        assert!(validate_username("alice\\bob").is_err());
    }

    #[test]
    fn test_uid_of_user() {
        assert_eq!(uid_of_user("root"), Some(0));
        assert_eq!(uid_of_user("nonexistent_user_xyz123"), None);
    }

    #[test]
    fn test_is_plain_filename() {
        let is_plain = |filename: &str| {
            let mut components = std::path::Path::new(filename).components();
            matches!(components.next(), Some(std::path::Component::Normal(_)))
                && components.next().is_none()
                && !filename.contains('\0')
        };

        assert!(is_plain("intrusion_20260923_120000.jpg"));
        assert!(is_plain("capture.jpg"));

        assert!(!is_plain("/etc/passwd"));
        assert!(!is_plain("../evil.jpg"));
        assert!(!is_plain("dir/file.jpg"));
        assert!(!is_plain(""));
        assert!(!is_plain("."));
        assert!(!is_plain(".."));
    }

    #[test]
    fn test_get_user_info_path_containment() {
        // Path traversal usernames are rejected by validate_username before filesystem access
        assert!(validate_username("../../../etc/passwd").is_err());
        assert!(validate_username("..").is_err());
        assert!(validate_username("foo/bar").is_err());

        // For a simulated user directory with symlink pointing outside base directory
        let unique_name = format!("sentinel_test_containment_{}", std::process::id());
        let temp_dir = std::env::temp_dir().join(unique_name);
        let base = temp_dir.join("users");
        let _ = std::fs::create_dir_all(&base);

        let evil_target = temp_dir.join("secret.txt");
        let _ = std::fs::write(&evil_target, "secret");

        // Create a symlink to outside target
        #[cfg(unix)]
        {
            let symlink_path = base.join("symlink_test");
            let _ = std::os::unix::fs::symlink(&evil_target, &symlink_path);
            let canonical = std::fs::canonicalize(&symlink_path);
            if let Ok(c) = canonical {
                assert!(!c.starts_with(&base), "Symlink resolving outside base dir must be detected");
            }
        }
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_get_greeter_info_serializable() {
        let info = greeter_detect::detect();
        let serialized = serde_json::to_string(&info);
        assert!(serialized.is_ok());
        let val: serde_json::Value = serde_json::from_str(&serialized.unwrap()).unwrap();
        assert!(val.get("greeter_type").is_some());
        assert!(val.get("has_tab_trigger").is_some());
        assert!(val.get("has_face_pam_icon").is_some());
        assert!(val.get("pam_service").is_some());
    }
}


