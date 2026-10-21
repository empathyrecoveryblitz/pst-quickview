//! Volatile, least-authority broker for an explicitly approved application-only session.
//! No source/workspace access, disk persistence, shell, desktop capture, or remote URLs
//! are exposed through this module's commands.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashSet, VecDeque},
    fs::File,
    io::Read,
    net::Ipv4Addr,
    path::PathBuf,
    sync::Mutex,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{Emitter, Manager, State};

const PREFIX: &str = "support-session-";
const DOCUMENT: &str = "/support/session.html";
const MAX_FRAME_BYTES: usize = 48 * 1024;
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);
const FRAME_LIFETIME: Duration = Duration::from_secs(2);
const SESSION_UNAVAILABLE: &str = "This support session is unavailable or has stopped.";
const INVALID_INVITATION: &str = "The support invitation is invalid or expired.";
const INVALID_FRAME: &str = "The shared application frame exceeds its permitted limits.";
const INVALID_ACTION: &str = "This support action is invalid or no longer current.";
const SERVICE_UNAVAILABLE: &str = "Remote Support is not configured in this application build.";
const SERVICE_CONFIG: &str = include_str!("../../public/support/service.json");
const REVOKE_EVENT: &str = "window.dispatchEvent(new Event('pqv-support-revoked'))";
const UPDATE_RESTART_EVENT: &str = "window.dispatchEvent(new Event('pqv-support-update-restart'))";
type SupportResult<T> = Result<T, String>;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SupportConfig {
    v: u8,
    sid: String,
    key: String,
    token: String,
    expires: u64,
    expected_exit: String,
    role: String,
    #[serde(default)]
    origin: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ServiceConfig {
    v: u8,
    origin: String,
    operator_public_key: String,
    operator_label: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueueBootstrap {
    v: u8,
    role: String,
    origin: String,
    operator_public_key: String,
    operator_label: String,
    expires: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    resume: Option<crate::updater::ResumeBootstrap>,
}

#[derive(Clone, Serialize)]
#[serde(untagged)]
pub(crate) enum SessionConfig {
    Direct(SupportConfig),
    Queue(QueueBootstrap),
}

impl SessionConfig {
    fn expires(&self) -> u64 {
        match self {
            Self::Direct(config) => config.expires,
            Self::Queue(config) => config.expires,
        }
    }

    fn origin(&self) -> Option<&str> {
        match self {
            Self::Direct(config) => config.origin.as_deref(),
            Self::Queue(config) => Some(&config.origin),
        }
    }

    fn operator_label(&self) -> &str {
        match self {
            Self::Direct(_) => "Invited technician",
            Self::Queue(config) => &config.operator_label,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ServiceStatus {
    configured: bool,
    operator_label: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionStatus {
    pub(crate) session_id: String,
    expires_at: u64,
    operator_label: String,
    updating: bool,
}

pub(crate) struct UpdateParent {
    pub(crate) session_id: String,
    pub(crate) expires: u64,
    pub(crate) origin: String,
    pub(crate) operator_public_key: String,
    pub(crate) operator_label: String,
    pub(crate) remaining_ms: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum DrawItem {
    Box {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        fill: String,
    },
    Text {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        text: String,
        color: String,
        size: f64,
        weight: u16,
    },
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum ActionKind {
    Click,
    Input,
    Scroll,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ControlTarget {
    id: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    kind: ActionKind,
    label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SupportFrame {
    v: u8,
    id: String,
    width: f64,
    height: f64,
    items: Vec<DrawItem>,
    targets: Vec<ControlTarget>,
    truncated: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SupportAction {
    frame: String,
    target: String,
    kind: ActionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delta_y: Option<f64>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ActionEvent {
    session_id: String,
    action: SupportAction,
}

struct Session {
    id: String,
    label: String,
    config: SessionConfig,
    deadline: Instant,
    heartbeat: Instant,
    frames: VecDeque<(Instant, SupportFrame)>,
    published: VecDeque<Instant>,
    actions: VecDeque<Instant>,
    updating: bool,
    resume_original_deadline: Option<Instant>,
}

#[derive(Default)]
struct Broker {
    active: Option<Session>,
}

#[derive(Default)]
pub(crate) struct SupportSessionState(Mutex<Broker>);

fn now_ms() -> SupportResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .ok_or_else(|| SESSION_UNAVAILABLE.to_string())
}

fn hex_id(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn public_ipv4(value: &str) -> bool {
    let Ok(address) = value.parse::<Ipv4Addr>() else {
        return false;
    };
    let [a, b, c, _] = address.octets();
    let restricted = matches!(a, 0 | 10 | 127 | 224..=255)
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && ((b == 0 && matches!(c, 0 | 2)) || b == 168 || (b == 88 && c == 99)))
        || (a == 198 && (matches!(b, 18 | 19) || (b == 51 && c == 100)))
        || (a == 203 && b == 0 && c == 113);
    address.to_string() == value && !restricted
}

fn allowed_origin(value: &str) -> bool {
    let Some(label) = value
        .strip_prefix("https://")
        .and_then(|host| host.strip_suffix(".run.pinggy-free.link"))
    else {
        return false;
    };
    !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn allowed_queue_origin(value: &str) -> bool {
    let Some(labels) = value
        .strip_prefix("https://")
        .and_then(|host| host.strip_suffix(".workers.dev"))
    else {
        return false;
    };
    let labels: Vec<_> = labels.split('.').collect();
    labels.len() == 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn parse_service_config(raw: &str) -> SupportResult<ServiceConfig> {
    let invalid = || SERVICE_UNAVAILABLE.to_string();
    if raw.len() > 2048 {
        return Err(invalid());
    }
    let config: ServiceConfig = serde_json::from_str(raw).map_err(|_| invalid())?;
    let key = URL_SAFE_NO_PAD
        .decode(&config.operator_public_key)
        .map_err(|_| invalid())?;
    if config.v != 3
        || !allowed_queue_origin(&config.origin)
        || key.len() != 65
        || key.first() != Some(&4)
        || URL_SAFE_NO_PAD.encode(&key) != config.operator_public_key
        || config.operator_label.trim() != config.operator_label
        || config.operator_label.is_empty()
        || !bounded_text(&config.operator_label, 80)
        || config.operator_label.chars().any(char::is_control)
    {
        return Err(invalid());
    }
    Ok(config)
}

fn service_config() -> SupportResult<ServiceConfig> {
    parse_service_config(SERVICE_CONFIG)
}

fn parse_invitation(invitation: &str, now: u64) -> SupportResult<SupportConfig> {
    let invalid = || INVALID_INVITATION.to_string();
    if invitation.len() > 8192 || invitation.trim() != invitation {
        return Err(invalid());
    }
    let url = tauri::Url::parse(invitation).map_err(|_| invalid())?;
    let origin = url.origin().ascii_serialization();
    if !allowed_origin(&origin)
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.path() != "/join"
        || url.query().is_some()
        || !invitation.starts_with(&format!("{origin}/join#"))
    {
        return Err(invalid());
    }
    let encoded = url.fragment().ok_or_else(invalid)?;
    if encoded.is_empty() || encoded.len() > 6000 {
        return Err(invalid());
    }
    let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid())?;
    if bytes.len() > 4096 || URL_SAFE_NO_PAD.encode(&bytes) != encoded {
        return Err(invalid());
    }
    let mut config: SupportConfig = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let key = URL_SAFE_NO_PAD.decode(&config.key).map_err(|_| invalid())?;
    if config.v != 2
        || config.role != "guest"
        || !hex_id(&config.sid, 32)
        || !hex_id(&config.token, 64)
        || config.key.len() != 43
        || key.len() != 32
        || URL_SAFE_NO_PAD.encode(key) != config.key
        || !public_ipv4(&config.expected_exit)
        || config.expires <= now
        || config.expires > now.saturating_add(30 * 60 * 1000)
        || config.origin.as_ref().is_some_and(|value| value != &origin)
    {
        return Err(invalid());
    }
    config.origin = Some(origin);
    Ok(config)
}

fn color(value: &str) -> bool {
    if let Some(hex) = value.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8)
            && hex.bytes().all(|byte| byte.is_ascii_hexdigit());
    }
    let (components, count) = if let Some(inner) = value.strip_prefix("rgba(") {
        (inner, 4)
    } else if let Some(inner) = value.strip_prefix("rgb(") {
        (inner, 3)
    } else {
        return false;
    };
    let Some(components) = components.strip_suffix(')') else {
        return false;
    };
    if value.len() > 64 {
        return false;
    }
    let values: Vec<_> = components.split(',').map(str::trim).collect();
    values.len() == count
        && values.iter().enumerate().all(|(index, part)| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'.')
                && part.parse::<f64>().is_ok_and(|number| {
                    number.is_finite()
                        && (0.0..=if index == 3 { 1.0 } else { 255.0 }).contains(&number)
                })
        })
}

fn rectangle(x: f64, y: f64, width: f64, height: f64) -> bool {
    [x, y, width, height]
        .iter()
        .all(|number| number.is_finite())
        && (0.0..=4096.0).contains(&x)
        && (0.0..=2160.0).contains(&y)
        && (0.0..=4096.0).contains(&width)
        && (0.0..=2160.0).contains(&height)
}

fn bounded_text(value: &str, maximum: usize) -> bool {
    value.encode_utf16().count() <= maximum
        && !value
            .chars()
            .any(|character| matches!(character as u32, 0..=8 | 11 | 12 | 14..=31 | 127))
}

fn validate_frame(value: serde_json::Value) -> SupportResult<SupportFrame> {
    let invalid = || INVALID_FRAME.to_string();
    if serde_json::to_vec(&value).map_err(|_| invalid())?.len() > MAX_FRAME_BYTES {
        return Err(invalid());
    }
    let frame: SupportFrame = serde_json::from_value(value).map_err(|_| invalid())?;
    if frame.v != 1
        || !hex_id(&frame.id, 32)
        || !frame.width.is_finite()
        || !frame.height.is_finite()
        || !(1.0..=4096.0).contains(&frame.width)
        || !(1.0..=2160.0).contains(&frame.height)
        || frame.items.len() > 600
        || frame.targets.len() > 80
    {
        return Err(invalid());
    }
    let mut text_count = 0;
    for item in &frame.items {
        let valid = match item {
            DrawItem::Box { x, y, w, h, fill } => rectangle(*x, *y, *w, *h) && color(fill),
            DrawItem::Text {
                x,
                y,
                w,
                h,
                text,
                color: fill,
                size,
                weight,
            } => {
                text_count += text.encode_utf16().count();
                rectangle(*x, *y, *w, *h)
                    && bounded_text(text, 1000)
                    && color(fill)
                    && size.is_finite()
                    && (8.0..=40.0).contains(size)
                    && (100..=900).contains(weight)
            }
        };
        if !valid || text_count > 12000 {
            return Err(invalid());
        }
    }
    let mut ids = HashSet::new();
    for target in &frame.targets {
        if !hex_id(&target.id, 32)
            || !ids.insert(&target.id)
            || !rectangle(target.x, target.y, target.w, target.h)
            || !bounded_text(&target.label, 100)
            || target
                .value
                .as_ref()
                .is_some_and(|value| !bounded_text(value, 2000))
            || (target.kind != ActionKind::Input && target.value.is_some())
        {
            return Err(invalid());
        }
    }
    Ok(frame)
}

fn validate_action(action: &SupportAction) -> bool {
    hex_id(&action.frame, 32)
        && hex_id(&action.target, 32)
        && match action.kind {
            ActionKind::Click => action.value.is_none() && action.delta_y.is_none(),
            ActionKind::Input => {
                action.delta_y.is_none()
                    && action
                        .value
                        .as_ref()
                        .is_some_and(|value| bounded_text(value, 2000))
            }
            ActionKind::Scroll => {
                action.value.is_none()
                    && action
                        .delta_y
                        .is_some_and(|delta| delta.is_finite() && (-700.0..=700.0).contains(&delta))
            }
        }
}

impl Session {
    fn status(&self) -> SessionStatus {
        SessionStatus {
            session_id: self.id.clone(),
            expires_at: self.config.expires(),
            operator_label: self.config.operator_label().to_string(),
            updating: self.updating,
        }
    }

    fn alive(&self, now: Instant) -> bool {
        now < self.deadline
            && now.saturating_duration_since(self.heartbeat) < HEARTBEAT_TIMEOUT
            && now_ms().is_ok_and(|wall_clock| wall_clock < self.config.expires())
    }

    fn support_window(&self, label: &str, id: &str) -> bool {
        label == self.label && id == self.id
    }

    fn publish(&mut self, frame: SupportFrame, now: Instant) -> SupportResult<bool> {
        self.heartbeat = now;
        if self.updating {
            return Ok(false);
        }
        while self
            .published
            .front()
            .is_some_and(|time| now.saturating_duration_since(*time) >= Duration::from_secs(1))
        {
            self.published.pop_front();
        }
        if self.frames.iter().any(|(_, prior)| prior.id == frame.id) {
            return Err(INVALID_FRAME.to_string());
        }
        if self.published.len() >= 2 {
            return Ok(false);
        }
        self.published.push_back(now);
        self.frames.push_back((now, frame));
        while self.frames.len() > 3
            || self
                .frames
                .front()
                .is_some_and(|(time, _)| now.saturating_duration_since(*time) > FRAME_LIFETIME)
        {
            self.frames.pop_front();
        }
        Ok(true)
    }

    fn authorize_action(&mut self, action: &SupportAction, now: Instant) -> SupportResult<()> {
        if self.updating || !self.alive(now) || !validate_action(action) {
            return Err(INVALID_ACTION.to_string());
        }
        while self
            .actions
            .front()
            .is_some_and(|time| now.saturating_duration_since(*time) >= Duration::from_secs(1))
        {
            self.actions.pop_front();
        }
        if self.actions.len() >= 10 {
            return Err(INVALID_ACTION.to_string());
        }
        let valid_target = self.frames.iter().any(|(time, frame)| {
            now.saturating_duration_since(*time) <= FRAME_LIFETIME
                && frame.id == action.frame
                && frame
                    .targets
                    .iter()
                    .any(|target| target.id == action.target && target.kind == action.kind)
        });
        if !valid_target {
            return Err(INVALID_ACTION.to_string());
        }
        self.actions.push_back(now);
        Ok(())
    }

    fn confirm_resumed(&mut self, now: Instant) -> SupportResult<bool> {
        if !self.alive(now) || self.updating {
            return Err(SESSION_UNAVAILABLE.to_string());
        }
        let Some(original_deadline) = self.resume_original_deadline.take() else {
            return Ok(false);
        };
        if original_deadline <= now {
            return Err(SESSION_UNAVAILABLE.to_string());
        }
        self.deadline = original_deadline;
        if let SessionConfig::Queue(config) = &mut self.config {
            config.resume = None;
        }
        Ok(true)
    }
}

fn local_document(url: &tauri::Url, id: &str) -> bool {
    matches!(
        (url.scheme(), url.host_str()),
        ("tauri", Some("localhost")) | ("http" | "https", Some("tauri.localhost"))
    ) && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == DOCUMENT
        && url.query() == Some(format!("session={id}").as_str())
        && url.fragment().is_none()
}

fn support_csp(existing: &str, origin: &str) -> Option<String> {
    let pinned_origin = service_config().ok().map(|config| config.origin);
    support_csp_for_service(existing, origin, pinned_origin.as_deref())
}

fn support_csp_for_service(
    existing: &str,
    origin: &str,
    pinned_origin: Option<&str>,
) -> Option<String> {
    if !(allowed_origin(origin) || (allowed_queue_origin(origin) && pinned_origin == Some(origin)))
    {
        return None;
    }
    let mut directives: Vec<String> = existing
        .split(';')
        .map(str::trim)
        .filter(|directive| {
            !directive.is_empty() && directive.split_whitespace().next() != Some("connect-src")
        })
        .map(str::to_owned)
        .collect();
    // Preserve Tauri's generated script hashes/nonces and every other directive.
    directives.push(format!("connect-src ipc: http://ipc.localhost {origin}"));
    Some(directives.join("; "))
}

fn take_session(app: &tauri::AppHandle, id: Option<&str>) -> Option<String> {
    let state = app.state::<SupportSessionState>();
    let mut broker = state
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if broker
        .active
        .as_ref()
        .is_some_and(|session| id.map_or(true, |id| id == session.id))
    {
        broker.active.take().map(|session| session.label)
    } else {
        None
    }
}

fn stop_matching(app: &tauri::AppHandle, id: Option<&str>) {
    let revoked = crate::updater::revoke_resume_authority(app, id);
    // Revoke broker authority before asking the isolated bundled document to
    // notify the queue or touching Keychain, which may require OS interaction.
    // This fixed event contains no remote or user input.
    let stopped_label = take_session(app, id);
    if let Some(label) = stopped_label {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.eval(REVOKE_EVENT);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(150));
                let _ = window.destroy();
            });
        }
    }
    if revoked {
        crate::updater::cleanup_revoked_resume();
    }
}

pub(crate) fn handle_run_event(app: &tauri::AppHandle, event: &tauri::RunEvent) {
    if crate::updater::expected_update_restart(app)
        && matches!(
            event,
            tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. }
        )
    {
        if let Some(label) = take_session(app, None) {
            if let Some(window) = app.get_webview_window(&label) {
                let _ = window.eval(UPDATE_RESTART_EVENT);
            }
        }
        if matches!(event, tauri::RunEvent::Exit) {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.hide();
            }
            crate::updater::finish_expected_update_exit(app);
        }
        return;
    }
    match event {
        tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. } => stop_matching(app, None),
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed,
            ..
        } => {
            let relevant = label == "main" || {
                let state = app.state::<SupportSessionState>();
                let broker = state
                    .0
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                broker
                    .active
                    .as_ref()
                    .is_some_and(|session| session.label == *label)
            };
            if relevant {
                if !crate::updater::expected_update_restart(app) {
                    stop_matching(app, None);
                }
            }
        }
        _ => {}
    }
}

#[tauri::command]
pub(crate) async fn start_support_session(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    invitation: String,
) -> SupportResult<SessionStatus> {
    if window.label() != "main" {
        return Err(SESSION_UNAVAILABLE.to_string());
    }
    let now = now_ms()?;
    let config = SessionConfig::Direct(parse_invitation(&invitation, now)?);
    open_approved_session(app, config, now)
}

#[tauri::command]
pub(crate) fn support_service_status(window: tauri::WebviewWindow) -> SupportResult<ServiceStatus> {
    if window.label() != "main" {
        return Err(SESSION_UNAVAILABLE.to_string());
    }
    Ok(match service_config() {
        Ok(config) => ServiceStatus {
            configured: true,
            operator_label: config.operator_label,
        },
        Err(_) => ServiceStatus {
            configured: false,
            operator_label: String::new(),
        },
    })
}

#[tauri::command]
pub(crate) async fn request_support_session(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> SupportResult<SessionStatus> {
    if window.label() != "main" {
        return Err(SESSION_UNAVAILABLE.to_string());
    }
    let config = service_config()?;
    let now = now_ms()?;
    let queue = QueueBootstrap {
        v: 3,
        role: "guest".to_string(),
        origin: config.origin,
        operator_public_key: config.operator_public_key,
        operator_label: config.operator_label,
        expires: now.saturating_add(30 * 60 * 1000),
        resume: None,
    };
    open_approved_session(app, SessionConfig::Queue(queue), now)
}

fn open_approved_session(
    app: tauri::AppHandle,
    config: SessionConfig,
    now: u64,
) -> SupportResult<SessionStatus> {
    let origin = config
        .origin()
        .map(str::to_owned)
        .ok_or_else(|| INVALID_INVITATION.to_string())?;
    let mut random = [0_u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut random))
        .map_err(|_| SESSION_UNAVAILABLE.to_string())?;
    let id = hex::encode(random);
    let label = format!("{PREFIX}{id}");
    let starts_paused = matches!(
        &config,
        SessionConfig::Queue(QueueBootstrap {
            resume: Some(_),
            ..
        })
    );
    let status = SessionStatus {
        session_id: id.clone(),
        expires_at: config.expires(),
        operator_label: config.operator_label().to_string(),
        updating: starts_paused,
    };
    let instant = Instant::now();
    let resume_original_deadline = match &config {
        SessionConfig::Queue(QueueBootstrap {
            resume: Some(resume),
            ..
        }) => Some(
            instant
                + Duration::from_millis(
                    resume
                        .native_remaining_ms
                        .min(config.expires().saturating_sub(now)),
                ),
        ),
        _ => None,
    };
    let remaining_ms = match &config {
        SessionConfig::Queue(QueueBootstrap {
            resume: Some(resume),
            ..
        }) => resume
            .native_remaining_ms
            .min(resume.native_connect_remaining_ms)
            .min(config.expires().saturating_sub(now)),
        _ => config.expires().saturating_sub(now),
    };
    if remaining_ms == 0 {
        return Err(SESSION_UNAVAILABLE.to_string());
    }
    let state = app.state::<SupportSessionState>();
    {
        let mut broker = state
            .0
            .lock()
            .map_err(|_| SESSION_UNAVAILABLE.to_string())?;
        if broker.active.is_some() {
            return Err("Stop the current support session before starting another.".to_string());
        }
        broker.active = Some(Session {
            id: id.clone(),
            label: label.clone(),
            deadline: instant + Duration::from_millis(remaining_ms),
            config,
            heartbeat: instant,
            frames: VecDeque::new(),
            published: VecDeque::new(),
            actions: VecDeque::new(),
            updating: starts_paused,
            resume_original_deadline,
        });
    }
    let navigation_id = id.clone();
    let resource_id = id.clone();
    let build = tauri::WebviewWindowBuilder::new(&app, &label,
        tauri::WebviewUrl::App(PathBuf::from(format!("support/session.html?session={id}"))))
        .title("PST QuickView — approved support session")
        .inner_size(600.0, 420.0)
        .min_inner_size(520.0, 360.0)
        .incognito(true)
        .disable_drag_drop_handler()
        .on_navigation(move |url| local_document(url, &navigation_id))
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .on_download(|_, _| false)
        .on_web_resource_request(move |request, response| {
            let Ok(url) = tauri::Url::parse(&request.uri().to_string()) else { return; };
            if !local_document(&url, &resource_id) {
                return;
            }
            let existing = response.headers().get("Content-Security-Policy")
                .and_then(|value| value.to_str().ok()).unwrap_or("default-src 'none'; script-src 'self'; style-src 'self'; frame-src 'none'; frame-ancestors 'none'; object-src 'none'; media-src 'none'; base-uri 'none'; form-action 'none'");
            if let Some(csp) = support_csp(existing, &origin)
                .and_then(|value| tauri::http::HeaderValue::from_str(&value).ok())
            {
                response.headers_mut().insert("Content-Security-Policy", csp);
            }
            response.headers_mut().insert("Cache-Control", tauri::http::HeaderValue::from_static("no-store"));
            response.headers_mut().insert("Referrer-Policy", tauri::http::HeaderValue::from_static("no-referrer"));
            response.headers_mut().insert("Permissions-Policy", tauri::http::HeaderValue::from_static("camera=(), microphone=(), display-capture=(), geolocation=(), clipboard-read=(), usb=(), serial=(), hid=()"));
        })
        .build();
    let support_window = match build {
        Ok(window) => window,
        Err(_) => {
            stop_matching(&app, Some(&id));
            return Err("Could not open the approved support session.".to_string());
        }
    };
    let still_authorized = state
        .0
        .lock()
        .map(|broker| {
            broker
                .active
                .as_ref()
                .is_some_and(|session| session.id == id && session.alive(Instant::now()))
        })
        .unwrap_or(false);
    if !still_authorized {
        stop_matching(&app, Some(&id));
        let _ = support_window.destroy();
        return Err(SESSION_UNAVAILABLE.to_string());
    }
    let watchdog_app = app.clone();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(250));
        let state = watchdog_app.state::<SupportSessionState>();
        let healthy = match state.0.lock() {
            Ok(broker) => match broker.active.as_ref() {
                Some(session) if session.id == id => session.alive(Instant::now()),
                _ => return,
            },
            Err(_) => false,
        };
        if !healthy {
            stop_matching(&watchdog_app, Some(&id));
            return;
        }
    });
    Ok(status)
}

#[tauri::command]
pub(crate) fn bootstrap_support_session(
    window: tauri::WebviewWindow,
    state: State<SupportSessionState>,
    session_id: String,
) -> SupportResult<SessionConfig> {
    let broker = state
        .0
        .lock()
        .map_err(|_| SESSION_UNAVAILABLE.to_string())?;
    let session = broker
        .active
        .as_ref()
        .ok_or_else(|| SESSION_UNAVAILABLE.to_string())?;
    if !session.support_window(window.label(), &session_id) || !session.alive(Instant::now()) {
        return Err(SESSION_UNAVAILABLE.to_string());
    }
    Ok(session.config.clone())
}

#[tauri::command]
pub(crate) fn support_session_status(
    window: tauri::WebviewWindow,
    state: State<SupportSessionState>,
) -> SupportResult<Option<SessionStatus>> {
    if window.label() != "main" {
        return Err(SESSION_UNAVAILABLE.to_string());
    }
    let mut broker = state
        .0
        .lock()
        .map_err(|_| SESSION_UNAVAILABLE.to_string())?;
    let now = Instant::now();
    Ok(broker
        .active
        .as_mut()
        .filter(|session| session.alive(now))
        .map(|session| {
            session.heartbeat = now;
            session.status()
        }))
}

#[tauri::command]
pub(crate) fn publish_support_frame(
    window: tauri::WebviewWindow,
    state: State<SupportSessionState>,
    session_id: String,
    frame: serde_json::Value,
) -> SupportResult<bool> {
    if window.label() != "main" {
        return Err(SESSION_UNAVAILABLE.to_string());
    }
    let frame = validate_frame(frame)?;
    let mut broker = state
        .0
        .lock()
        .map_err(|_| SESSION_UNAVAILABLE.to_string())?;
    let session = broker
        .active
        .as_mut()
        .ok_or_else(|| SESSION_UNAVAILABLE.to_string())?;
    let now = Instant::now();
    if session.id != session_id || !session.alive(now) {
        return Err(SESSION_UNAVAILABLE.to_string());
    }
    session.publish(frame, now)
}

#[tauri::command]
pub(crate) fn poll_support_frame(
    window: tauri::WebviewWindow,
    state: State<SupportSessionState>,
    session_id: String,
) -> SupportResult<Option<SupportFrame>> {
    let broker = state
        .0
        .lock()
        .map_err(|_| SESSION_UNAVAILABLE.to_string())?;
    let session = broker
        .active
        .as_ref()
        .ok_or_else(|| SESSION_UNAVAILABLE.to_string())?;
    let now = Instant::now();
    if !session.support_window(window.label(), &session_id) || !session.alive(now) {
        return Err(SESSION_UNAVAILABLE.to_string());
    }
    if session.updating {
        return Ok(None);
    }
    Ok(session
        .frames
        .back()
        .filter(|(time, _)| now.saturating_duration_since(*time) <= FRAME_LIFETIME)
        .map(|(_, frame)| frame.clone()))
}

#[tauri::command]
pub(crate) fn support_submit_action(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<SupportSessionState>,
    session_id: String,
    action: SupportAction,
) -> SupportResult<bool> {
    if !validate_action(&action) {
        return Err(INVALID_ACTION.to_string());
    }
    {
        let mut broker = state
            .0
            .lock()
            .map_err(|_| SESSION_UNAVAILABLE.to_string())?;
        let session = broker
            .active
            .as_mut()
            .ok_or_else(|| SESSION_UNAVAILABLE.to_string())?;
        if !session.support_window(window.label(), &session_id) || !session.alive(Instant::now()) {
            return Err(SESSION_UNAVAILABLE.to_string());
        }
        if session.authorize_action(&action, Instant::now()).is_err() {
            // Ordinary display latency or rate limiting must not terminate a
            // healthy session. The main window independently revalidates it.
            return Ok(false);
        }
    }
    app.emit_to("main", "support-action", ActionEvent { session_id, action })
        .map(|()| true)
        .map_err(|_| SESSION_UNAVAILABLE.to_string())
}

#[tauri::command(async)]
pub(crate) fn stop_support_session(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<SupportSessionState>,
    session_id: String,
) -> SupportResult<()> {
    let allowed = {
        let broker = state
            .0
            .lock()
            .map_err(|_| SESSION_UNAVAILABLE.to_string())?;
        broker.active.as_ref().is_some_and(|session| {
            session.id == session_id
                && (window.label() == "main" || session.support_window(window.label(), &session_id))
        })
    };
    if !allowed {
        // Stop remains authoritative during the brief interval after the live
        // broker was closed and before an approved update has relaunched.
        if window.label() == "main" && crate::updater::resume_matches(&app, &session_id) {
            crate::updater::revoke_resume(&app, Some(&session_id));
            return Ok(());
        }
        return Err(SESSION_UNAVAILABLE.to_string());
    }
    stop_matching(&app, Some(&session_id));
    Ok(())
}

pub(crate) fn update_service_identity() -> SupportResult<(String, String, String)> {
    let config = service_config()?;
    Ok((
        config.origin,
        config.operator_public_key,
        config.operator_label,
    ))
}

pub(crate) fn update_parent(app: &tauri::AppHandle) -> SupportResult<UpdateParent> {
    let state = app.state::<SupportSessionState>();
    let broker = state
        .0
        .lock()
        .map_err(|_| SESSION_UNAVAILABLE.to_string())?;
    let session = broker
        .active
        .as_ref()
        .ok_or_else(|| SESSION_UNAVAILABLE.to_string())?;
    let SessionConfig::Queue(config) = &session.config else {
        return Err(SESSION_UNAVAILABLE.to_string());
    };
    if !session.alive(Instant::now()) || session.updating {
        return Err(SESSION_UNAVAILABLE.to_string());
    }
    Ok(UpdateParent {
        session_id: session.id.clone(),
        expires: config.expires,
        origin: config.origin.clone(),
        operator_public_key: config.operator_public_key.clone(),
        operator_label: config.operator_label.clone(),
        remaining_ms: u64::try_from(
            session
                .deadline
                .saturating_duration_since(Instant::now())
                .as_millis(),
        )
        .map_err(|_| SESSION_UNAVAILABLE.to_string())?,
    })
}

pub(crate) fn update_window_authorized(app: &tauri::AppHandle, label: &str, id: &str) -> bool {
    let state = app.state::<SupportSessionState>();
    let authorized = state.0.lock().is_ok_and(|broker| {
        broker.active.as_ref().is_some_and(|session| {
            session.support_window(label, id) && session.alive(Instant::now())
        })
    });
    authorized
}

pub(crate) fn pause_for_update(app: &tauri::AppHandle, id: &str) -> SupportResult<()> {
    let state = app.state::<SupportSessionState>();
    let mut broker = state
        .0
        .lock()
        .map_err(|_| SESSION_UNAVAILABLE.to_string())?;
    let session = broker
        .active
        .as_mut()
        .ok_or_else(|| SESSION_UNAVAILABLE.to_string())?;
    if session.id != id || !session.alive(Instant::now()) || session.updating {
        return Err(SESSION_UNAVAILABLE.to_string());
    }
    session.updating = true;
    session.frames.clear();
    session.actions.clear();
    session.published.clear();
    Ok(())
}

pub(crate) fn stop_for_update_failure(app: &tauri::AppHandle, id: Option<&str>) {
    stop_matching(app, id);
}

pub(crate) fn open_resumed_session(
    app: tauri::AppHandle,
    resume: crate::updater::ResumeBootstrap,
) -> SupportResult<SessionStatus> {
    let config = service_config()?;
    let now = now_ms()?;
    let expires = resume.handoff.original_expires;
    if expires <= now || expires > now.saturating_add(30 * 60 * 1000) {
        return Err(SESSION_UNAVAILABLE.to_string());
    }
    open_approved_session(
        app,
        SessionConfig::Queue(QueueBootstrap {
            v: 3,
            role: "guest".to_string(),
            origin: config.origin,
            operator_public_key: config.operator_public_key,
            operator_label: config.operator_label,
            expires,
            resume: Some(resume),
        }),
        now,
    )
}

pub(crate) fn activate_resumed_session(
    app: &tauri::AppHandle,
    id: &str,
) -> SupportResult<SessionStatus> {
    let state = app.state::<SupportSessionState>();
    let mut broker = state
        .0
        .lock()
        .map_err(|_| SESSION_UNAVAILABLE.to_string())?;
    let session = broker
        .active
        .as_mut()
        .ok_or_else(|| SESSION_UNAVAILABLE.to_string())?;
    if session.id != id
        || !session.alive(Instant::now())
        || !session.updating
        || !matches!(
            &session.config,
            SessionConfig::Queue(QueueBootstrap {
                resume: Some(_),
                ..
            })
        )
    {
        return Err(SESSION_UNAVAILABLE.to_string());
    }
    session.updating = false;
    Ok(session.status())
}

pub(crate) fn confirm_update_resume(
    app: &tauri::AppHandle,
    label: &str,
    id: &str,
) -> SupportResult<bool> {
    let state = app.state::<SupportSessionState>();
    let mut broker = state
        .0
        .lock()
        .map_err(|_| SESSION_UNAVAILABLE.to_string())?;
    let session = broker
        .active
        .as_mut()
        .ok_or_else(|| SESSION_UNAVAILABLE.to_string())?;
    let now = Instant::now();
    if !session.support_window(label, id) {
        return Err(SESSION_UNAVAILABLE.to_string());
    }
    session.confirm_resumed(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue_service() -> serde_json::Value {
        let public_key = hex::decode("046b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c2964fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5").unwrap();
        serde_json::json!({"v":3,"origin":"https://synthetic.synthetic.workers.dev",
            "operatorPublicKey":URL_SAFE_NO_PAD.encode(public_key),"operatorLabel":"Fictional helper"})
    }

    #[test]
    fn queue_service_requires_exact_bundled_origin_identity_and_canonical_public_key() {
        let valid = queue_service();
        assert!(parse_service_config(&valid.to_string()).is_ok());
        for origin in [
            "",
            "http://synthetic.synthetic.workers.dev",
            "https://synthetic.workers.dev",
            "https://nested.synthetic.synthetic.workers.dev",
            "https://synthetic.synthetic.workers.dev/",
            "https://synthetic.synthetic.workers.dev:443",
            "https://user@synthetic.synthetic.workers.dev",
            "https://synthetic.synthetic.workers.dev.invalid",
            "https://synthetic.synthetic.workers.dev; connect-src *",
        ] {
            let mut invalid = valid.clone();
            invalid["origin"] = origin.into();
            assert!(parse_service_config(&invalid.to_string()).is_err());
        }
        for label in ["", " label", "label ", "helper\nidentity"] {
            let mut invalid = valid.clone();
            invalid["operatorLabel"] = label.into();
            assert!(parse_service_config(&invalid.to_string()).is_err());
        }
        let mut invalid = valid.clone();
        invalid["operatorPublicKey"] = "".into();
        assert!(parse_service_config(&invalid.to_string()).is_err());
        invalid = valid.clone();
        invalid["operatorPublicKey"] =
            format!("{}=", valid["operatorPublicKey"].as_str().unwrap()).into();
        assert!(parse_service_config(&invalid.to_string()).is_err());
        invalid = valid;
        invalid["unexpected"] = true.into();
        assert!(parse_service_config(&invalid.to_string()).is_err());
        assert!(parse_service_config(
            r#"{"v":3,"origin":"","operatorPublicKey":"","operatorLabel":"PST QuickView support"}"#
        )
        .is_err());
    }

    #[test]
    fn queue_csp_pins_only_the_compiled_service_without_wildcards() {
        let baseline = "default-src 'none'; script-src 'self' 'nonce-generated'; connect-src ipc: http://ipc.localhost; object-src 'none'";
        let origin = "https://synthetic.synthetic.workers.dev";
        assert!(support_csp_for_service(baseline, origin, None).is_none());
        assert!(support_csp_for_service(
            baseline,
            origin,
            Some("https://other.synthetic.workers.dev")
        )
        .is_none());
        let csp = support_csp_for_service(baseline, origin, Some(origin)).unwrap();
        assert!(csp.contains("script-src 'self' 'nonce-generated'"));
        assert!(csp.contains(
            "connect-src ipc: http://ipc.localhost https://synthetic.synthetic.workers.dev"
        ));
        assert!(!csp.contains("*.workers.dev"));
    }

    #[test]
    fn queue_bootstrap_is_public_identity_only_and_bound_to_local_session_lifetime() {
        let service = parse_service_config(&queue_service().to_string()).unwrap();
        let expires = now_ms().unwrap() + 30_000;
        let bootstrap = SessionConfig::Queue(QueueBootstrap {
            v: 3,
            role: "guest".to_string(),
            origin: service.origin,
            operator_public_key: service.operator_public_key,
            operator_label: service.operator_label,
            expires,
            resume: None,
        });
        let wire = serde_json::to_value(&bootstrap).unwrap();
        assert_eq!(wire["v"], 3);
        assert_eq!(wire["role"], "guest");
        assert_eq!(bootstrap.expires(), expires);
        assert_eq!(bootstrap.operator_label(), "Fictional helper");
        for secret in ["sid", "token", "key", "privateKey", "expectedExit"] {
            assert!(wire.get(secret).is_none());
        }
        let mut record = session(Instant::now());
        record.config = bootstrap;
        assert_eq!(record.status().operator_label, "Fictional helper");
        assert_eq!(record.status().expires_at, expires);
        assert!(record.alive(Instant::now()));
        assert!(!record.alive(Instant::now() + HEARTBEAT_TIMEOUT));
    }

    #[test]
    fn queue_revocation_event_is_fixed_local_code_without_session_data() {
        assert_eq!(
            REVOKE_EVENT,
            "window.dispatchEvent(new Event('pqv-support-revoked'))"
        );
        assert!(!REVOKE_EVENT.contains("fetch"));
        assert!(!REVOKE_EVENT.contains("token"));
    }

    #[test]
    fn updating_revokes_old_frames_and_controls_without_extending_the_session() {
        let now = Instant::now();
        let mut session = session(now);
        let deadline = session.deadline;
        let value = frame('a');
        let action = SupportAction {
            frame: value.id.clone(),
            target: "c".repeat(32),
            kind: ActionKind::Click,
            value: None,
            delta_y: None,
        };
        assert!(session.publish(value, now).unwrap());
        session.updating = true;
        session.frames.clear();
        session.actions.clear();
        session.published.clear();
        assert!(!session
            .publish(frame('b'), now + Duration::from_millis(10))
            .unwrap());
        assert!(session.frames.is_empty());
        assert!(session
            .authorize_action(&action, now + Duration::from_millis(10))
            .is_err());
        assert_eq!(session.deadline, deadline);
        assert!(session.status().updating);
    }

    #[test]
    fn resumed_connection_must_finish_within_grace_and_can_restore_only_its_original_deadline_once()
    {
        let now = Instant::now();
        let mut timely = session(now);
        let original = timely.deadline;
        let service = parse_service_config(&queue_service().to_string()).unwrap();
        let expires = now_ms().unwrap() + 30_000;
        let resume = serde_json::from_value::<crate::updater::ResumeBootstrap>(serde_json::json!({
            "handoff":{"v":1,"parentId":"1".repeat(32),"parentSid":"2".repeat(32),"nonce":"3".repeat(32),
                "targetVersion":"0.2.0-beta.4-support.2","artifactSha256":"4".repeat(64),"nativeSessionId":"5".repeat(32),
                "originalExpires":expires,"resumeDeadline":expires,"resumeTicketSha256":"6".repeat(64)},
            "resumeTicket":"7".repeat(64),"resumeAuth":"8".repeat(64)})).unwrap();
        timely.config = SessionConfig::Queue(QueueBootstrap {
            v: 3,
            role: "guest".to_string(),
            origin: service.origin,
            operator_public_key: service.operator_public_key,
            operator_label: service.operator_label,
            expires,
            resume: Some(resume),
        });
        timely.deadline = now + Duration::from_secs(2);
        timely.resume_original_deadline = Some(original);
        assert!(timely
            .confirm_resumed(now + Duration::from_millis(10))
            .unwrap());
        assert_eq!(timely.deadline, original);
        assert!(matches!(
            &timely.config,
            SessionConfig::Queue(QueueBootstrap { resume: None, .. })
        ));
        assert!(!timely
            .confirm_resumed(now + Duration::from_millis(20))
            .unwrap());
        let mut late = session(now);
        late.deadline = now + Duration::from_secs(2);
        late.resume_original_deadline = Some(original);
        assert!(late.confirm_resumed(now + Duration::from_secs(2)).is_err());
        assert_eq!(late.deadline, now + Duration::from_secs(2));
        let mut paused = session(now);
        paused.updating = true;
        paused.resume_original_deadline = Some(original);
        assert!(paused.confirm_resumed(now).is_err());
        assert!(!session(now).confirm_resumed(now).unwrap());
    }

    fn config(expires: u64) -> SupportConfig {
        SupportConfig {
            v: 2,
            sid: "a".repeat(32),
            key: URL_SAFE_NO_PAD.encode([0_u8; 32]),
            token: "b".repeat(64),
            expires,
            expected_exit: "1.1.1.1".to_string(),
            role: "guest".to_string(),
            origin: Some("https://synthetic.run.pinggy-free.link".to_string()),
        }
    }

    fn invitation(config: &SupportConfig) -> String {
        format!(
            "https://synthetic.run.pinggy-free.link/join#{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(config).unwrap())
        )
    }

    fn frame(id: char) -> SupportFrame {
        validate_frame(serde_json::json!({"v":1,"id":id.to_string().repeat(32),"width":1280,"height":820,
            "items":[{"kind":"text","x":1,"y":1,"w":100,"h":20,"text":"Synthetic","color":"rgb(10, 20, 30)","size":14,"weight":400}],
            "targets":[{"id":"c".repeat(32),"x":0,"y":0,"w":100,"h":20,"kind":"click","label":"Synthetic"}],"truncated":false})).unwrap()
    }

    fn session(now: Instant) -> Session {
        Session {
            id: "d".repeat(32),
            label: format!("{PREFIX}{}", "d".repeat(32)),
            config: SessionConfig::Direct(config(now_ms().unwrap() + 30_000)),
            deadline: now + Duration::from_secs(30),
            heartbeat: now,
            frames: VecDeque::new(),
            published: VecDeque::new(),
            actions: VecDeque::new(),
            updating: false,
            resume_original_deadline: None,
        }
    }

    #[test]
    fn invitations_require_fresh_guest_credentials_and_exact_https_origin() {
        let valid = config(120_000);
        assert!(parse_invitation(&invitation(&valid), 100_000).is_ok());
        for malformed in [
            invitation(&valid).replace("https://", "http://"),
            invitation(&valid).replace("/join#", "/other#"),
            invitation(&valid).replace("/join#", "/join?arbitrary=1#"),
            invitation(&valid).replace("https://", "https://user@"),
            invitation(&valid).replace("/join#", ":443/join#"),
            invitation(&valid).replace("synthetic.run.", "nested.synthetic.run."),
        ] {
            assert!(parse_invitation(&malformed, 100_000).is_err());
        }
        let mut invalid_role = config(120_000);
        invalid_role.role = "controller".to_string();
        let mut invalid_sid = config(120_000);
        invalid_sid.sid = "A".repeat(32);
        let mut invalid_token = config(120_000);
        invalid_token.token = "b".repeat(63);
        let mut invalid_key = config(120_000);
        invalid_key.key = "x".repeat(43);
        for bad in [
            config(100_000),
            config(1_900_001),
            invalid_role,
            invalid_sid,
            invalid_token,
            invalid_key,
        ] {
            assert!(parse_invitation(&invitation(&bad), 100_000).is_err());
        }
        let mut wrong = config(120_000);
        wrong.origin = Some("https://different.run.pinggy-free.link".to_string());
        assert!(parse_invitation(&invitation(&wrong), 100_000).is_err());
        wrong = config(120_000);
        wrong.expected_exit = "127.0.0.1".to_string();
        assert!(parse_invitation(&invitation(&wrong), 100_000).is_err());
    }

    #[test]
    fn public_routes_reject_nonpublic_and_noncanonical_ipv4() {
        for rejected in [
            "0.0.0.0",
            "10.0.0.1",
            "127.0.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "100.64.0.1",
            "100.127.255.255",
            "169.254.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.1",
            "192.0.2.1",
            "192.168.0.1",
            "192.88.99.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.1",
            "203.0.113.1",
            "01.1.1.1",
            "1.1.1.1 ",
            "1.1.1",
            "::1",
        ] {
            assert!(!public_ipv4(rejected));
        }
        for accepted in [
            "1.1.1.1",
            "100.63.255.255",
            "100.128.0.1",
            "169.253.0.1",
            "172.15.255.255",
            "172.32.0.1",
            "192.0.1.1",
            "192.88.98.1",
            "198.17.255.255",
            "198.20.0.1",
            "198.51.99.1",
            "203.0.112.1",
        ] {
            assert!(public_ipv4(accepted));
        }
    }

    #[test]
    fn support_window_authority_is_exact_and_navigation_never_leaves_local_document() {
        let record = session(Instant::now());
        assert!(record.support_window(&record.label, &record.id));
        for label in ["main", "message-preview-abc", "support-session-other"] {
            assert!(!record.support_window(label, &record.id));
        }
        assert!(!record.support_window(&record.label, &"e".repeat(32)));
        let url = format!(
            "tauri://localhost/support/session.html?session={}",
            record.id
        );
        assert!(local_document(
            &tauri::Url::parse(&url).unwrap(),
            &record.id
        ));
        for bad in [
            url.replace("tauri://localhost", "https://example.invalid"),
            url.replace("session.html", "session.js"),
            format!("{url}#remote"),
            format!("{url}&extra=1"),
        ] {
            assert!(!local_document(
                &tauri::Url::parse(&bad).unwrap(),
                &record.id
            ));
        }
    }

    #[test]
    fn vector_frames_reject_html_unknown_keys_oversize_and_unsafe_colors() {
        let valid = serde_json::to_value(frame('a')).unwrap();
        let mut bad = valid.clone();
        bad["html"] = "<script>arbitrary</script>".into();
        assert!(validate_frame(bad).is_err());
        let mut bad = valid.clone();
        bad["items"][0]["color"] = "url(https://example.invalid)".into();
        assert!(validate_frame(bad).is_err());
        let mut bad = valid.clone();
        bad["items"][0]["text"] = "x".repeat(1001).into();
        assert!(validate_frame(bad).is_err());
        let mut bad = valid.clone();
        bad["targets"][0]["command"] = "arbitrary".into();
        assert!(validate_frame(bad).is_err());
        let mut bad = valid.clone();
        bad["items"] = serde_json::Value::Array(vec![valid["items"][0].clone(); 601]);
        assert!(validate_frame(bad).is_err());
        let mut bad = valid;
        bad["width"] = 5000.into();
        assert!(validate_frame(bad).is_err());
    }

    #[test]
    fn lifecycle_rejects_stalled_heartbeat_and_expiry_and_rate_limits_frames_actions() {
        let now = Instant::now();
        let mut record = session(now);
        assert!(record.alive(now));
        assert!(!record.alive(now + HEARTBEAT_TIMEOUT));
        assert!(!record.alive(now + Duration::from_secs(31)));
        record.publish(frame('a'), now).unwrap();
        record
            .publish(frame('b'), now + Duration::from_millis(500))
            .unwrap();
        assert!(!record
            .publish(frame('c'), now + Duration::from_millis(600))
            .unwrap());
        let mut action = SupportAction {
            frame: "a".repeat(32),
            target: "c".repeat(32),
            kind: ActionKind::Click,
            value: None,
            delta_y: None,
        };
        for _ in 0..10 {
            record
                .authorize_action(&action, now + Duration::from_millis(700))
                .unwrap();
        }
        assert!(record
            .authorize_action(&action, now + Duration::from_millis(701))
            .is_err());
        action.frame = "f".repeat(32);
        assert!(record
            .authorize_action(&action, now + Duration::from_millis(1800))
            .is_err());
        action.frame = "a".repeat(32);
        action.target = "e".repeat(32);
        assert!(record
            .authorize_action(&action, now + Duration::from_millis(1800))
            .is_err());
        action.target = "c".repeat(32);
        action.kind = ActionKind::Input;
        action.value = Some("Synthetic".into());
        assert!(record
            .authorize_action(&action, now + Duration::from_millis(1800))
            .is_err());
        action.kind = ActionKind::Click;
        action.value = None;
        assert!(record
            .authorize_action(&action, now + Duration::from_millis(2100))
            .is_err());
    }

    #[test]
    fn csp_exception_preserves_generated_script_policy_and_is_origin_scoped() {
        let baseline = "default-src 'none'; script-src 'self' 'nonce-generated'; connect-src ipc: http://ipc.localhost; object-src 'none'; frame-src 'none'";
        let csp = support_csp(baseline, "https://synthetic.run.pinggy-free.link").unwrap();
        assert!(csp.contains("script-src 'self' 'nonce-generated'"));
        assert!(csp.contains(
            "connect-src ipc: http://ipc.localhost https://synthetic.run.pinggy-free.link"
        ));
        assert!(!csp.contains("script-src 'unsafe-inline'"));
        assert!(support_csp(
            baseline,
            "https://synthetic.run.pinggy-free.link; script-src *"
        )
        .is_none());
        assert!(support_csp(baseline, "https://nested.synthetic.run.pinggy-free.link").is_none());
    }

    #[test]
    fn delayed_frame_publication_throttles_without_revoking_current_controls() {
        let now = Instant::now();
        let mut record = session(now);
        for (elapsed, id) in [(0, 'a'), (850, 'b'), (1100, 'c')] {
            assert!(record
                .publish(frame(id), now + Duration::from_millis(elapsed))
                .unwrap());
        }
        let delayed = now + Duration::from_millis(1650);
        assert!(!record.publish(frame('d'), delayed).unwrap());
        assert!(record.alive(delayed));
        assert_eq!(record.frames.back().unwrap().1.id, "c".repeat(32));
        let action = SupportAction {
            frame: "c".repeat(32),
            target: "c".repeat(32),
            kind: ActionKind::Click,
            value: None,
            delta_y: None,
        };
        assert!(record.authorize_action(&action, delayed).is_ok());
        assert!(record.publish(frame('c'), delayed).is_err());
    }

    #[test]
    fn actions_match_protocol_text_and_scroll_bounds() {
        let mut action = SupportAction {
            frame: "a".repeat(32),
            target: "c".repeat(32),
            kind: ActionKind::Scroll,
            value: None,
            delta_y: Some(700.0),
        };
        assert!(validate_action(&action));
        action.delta_y = Some(700.1);
        assert!(!validate_action(&action));
        action.kind = ActionKind::Input;
        action.delta_y = None;
        action.value = Some("Synthetic\ttext\nline\rreturn".to_string());
        assert!(validate_action(&action));
        action.value = Some("Synthetic\u{7f}".to_string());
        assert!(!validate_action(&action));
        action.value = Some("\u{1f600}".repeat(1001));
        assert!(!validate_action(&action));
    }
}
