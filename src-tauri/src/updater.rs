//! Owner-approved updates and one-use, build-bound support continuity.
//! No mailbox data, source paths, WebRTC keys, or old queue credentials are persisted.
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    sync::{
        atomic::{AtomicU64, Ordering},
        Condvar, Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::Manager;

type UpdateResult<T> = Result<T, String>;
const UNAVAILABLE: &str = "Verified updates are unavailable or the operation was cancelled.";
const FAILED: &str = "The verified update could not complete. Support access is closed.";
const CONFIG: &str = include_str!("../../public/support/update-service.json");
const MANIFEST_LIMIT: usize = 64 * 1024;
const ARTIFACT_LIMIT: usize = 25 * 1024 * 1024;
const RESUME_MS: u64 = 90_000;
const RESUME_CLOCK_MARGIN_MS: u64 = 5_000;
const CANDIDATE_LIFETIME: Duration = Duration::from_secs(10 * 60);
const KEYCHAIN_SERVICE: &str = "com.local.pstquickview.support-update-resume";
const KEYCHAIN_ACCOUNT: &str = "single-use-v1";
const ACK_DOMAIN: &str = "pst-quickview-update-ack-v1\n";
const LAUNCH_PROOF_ENV: &str = "PST_QUICKVIEW_UPDATE_RESUME_PROOF";
static LAUNCH_PROOF: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn update_resume_deadline_ms(now: u64, original_expires: u64) -> u64 {
    // Keep the wire deadline below the service's strict 90-second cap. Both
    // endpoints retain their unchanged maximum and original-expiry checks.
    now.saturating_add(RESUME_MS - RESUME_CLOCK_MARGIN_MS)
        .min(original_expires)
}

// First line of run(), before application threads or webviews are created.
// Only the updater's fixed child process receives this environment value.
pub(crate) fn initialize_launch_proof() {
    let proof = std::env::var(LAUNCH_PROOF_ENV)
        .ok()
        .filter(|value| hex_string(value, 64));
    std::env::remove_var(LAUNCH_PROOF_ENV);
    let _ = LAUNCH_PROOF.set(Mutex::new(proof));
}

fn take_launch_proof(proof: &Mutex<Option<String>>) -> Option<String> {
    proof.lock().ok().and_then(|mut value| value.take())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateConfig {
    v: u8,
    origin: String,
    manifest_path: String,
    artifact_prefix: String,
    public_key: String,
    channel: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Platform {
    url: String,
    signature: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    version: String,
    platforms: HashMap<String, Platform>,
    channel: String,
    artifact_sha256: String,
    size: u64,
    source_commit: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default, rename = "pub_date")]
    pub_date: Option<String>,
}

// Field order is the cross-language signed/HMAC wire format. Do not reorder.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UpdateHandoff {
    pub(crate) v: u8,
    pub(crate) parent_id: String,
    pub(crate) parent_sid: String,
    pub(crate) nonce: String,
    pub(crate) target_version: String,
    pub(crate) artifact_sha256: String,
    pub(crate) native_session_id: String,
    pub(crate) original_expires: u64,
    pub(crate) resume_deadline: u64,
    pub(crate) resume_ticket_sha256: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingUpdateHandoff {
    v: u8,
    nonce: String,
    target_version: String,
    artifact_sha256: String,
    native_session_id: String,
    original_expires: u64,
    resume_deadline: u64,
    resume_auth: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ResumeBootstrap {
    pub(crate) handoff: UpdateHandoff,
    resume_ticket: String,
    resume_auth: String,
    #[serde(skip)]
    pub(crate) native_remaining_ms: u64,
    #[serde(skip)]
    pub(crate) native_connect_remaining_ms: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResumeGrant {
    v: u8,
    armed: bool,
    origin: String,
    operator_public_key: String,
    operator_label: String,
    boot: String,
    issued_ns: u64,
    deadline_ns: u64,
    original_deadline_ns: u64,
    executable_sha256: String,
    launch_proof_sha256: String,
    resume: ResumeBootstrap,
}

struct PendingHandoff {
    public: PendingUpdateHandoff,
    grant: ResumeGrant,
    generation: u64,
    acknowledged: bool,
    saving: bool,
}

struct PendingRelaunch {
    path: std::path::PathBuf,
    proof: String,
    executable_sha256: String,
    generation: u64,
}

struct VerifiedUpdate {
    id: String,
    version: String,
    sha256: String,
    bytes: Vec<u8>,
    created: Instant,
    #[cfg(target_os = "macos")]
    update: tauri_plugin_updater::Update,
}

#[derive(Default)]
struct UpdateBroker {
    checking: bool,
    installing: bool,
    pending: Option<VerifiedUpdate>,
    handoff: Option<PendingHandoff>,
    resume_loaded: bool,
    ready_resume: Option<ResumeGrant>,
    restart_armed: bool,
    resuming_id: Option<String>,
    install_guard: Option<crate::UpdateInstallGuard>,
    relaunch: Option<PendingRelaunch>,
}

#[derive(Default)]
pub(crate) struct UpdaterState {
    inner: Mutex<UpdateBroker>,
    changed: Condvar,
    revocation_epoch: AtomicU64,
    // This independent lock is never held for Keychain, hashing, or other I/O.
    bound_id: Mutex<Option<String>>,
}

fn invalidate_epoch_for_id(
    epoch: &AtomicU64,
    binding: &Mutex<Option<String>>,
    id: Option<&str>,
) -> bool {
    let bound = binding
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if id
        .zip(bound.as_deref())
        .is_some_and(|(requested, bound)| requested != bound)
    {
        return false;
    }
    epoch.fetch_add(1, Ordering::SeqCst);
    true
}

fn epoch_fenced_io<T>(
    epoch: &AtomicU64,
    generation: u64,
    operation: impl FnOnce() -> UpdateResult<T>,
) -> UpdateResult<T> {
    if epoch.load(Ordering::SeqCst) != generation {
        return Err(fail());
    }
    let value = operation()?;
    if epoch.load(Ordering::SeqCst) != generation {
        return Err(fail());
    }
    Ok(value)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateStatus {
    available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResumeStatus {
    available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operator_label: Option<String>,
}

fn fail() -> String {
    UNAVAILABLE.to_string()
}
fn now_ms() -> UpdateResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|value| u64::try_from(value.as_millis()).ok())
        .ok_or_else(fail)
}
fn hex_string(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn random_hex(bytes: usize) -> UpdateResult<String> {
    let mut value = vec![0; bytes];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut value))
        .map_err(|_| fail())?;
    Ok(hex::encode(value))
}
fn digest(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}
fn main_only(window: &tauri::WebviewWindow) -> UpdateResult<()> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err(fail())
    }
}
fn bounded_version(value: &str) -> bool {
    value.len() <= 80
        && value.is_ascii()
        && semver::Version::parse(value).is_ok_and(|version| version.build.is_empty())
}
fn config(raw: &str, expected_origin: &str) -> UpdateResult<UpdateConfig> {
    use base64::Engine as _;
    if raw.len() > 8192 {
        return Err(fail());
    }
    let config: UpdateConfig = serde_json::from_str(raw).map_err(|_| fail())?;
    let key = base64::engine::general_purpose::STANDARD
        .decode(&config.public_key)
        .map_err(|_| fail())?;
    if config.v != 1
        || config.origin != expected_origin
        || !config.origin.starts_with("https://")
        || config.manifest_path != "/updates/development.json"
        || config.artifact_prefix != "/updates/artifacts/"
        || config.channel != "support-development"
        || !(42..=4096).contains(&key.len())
        || base64::engine::general_purpose::STANDARD.encode(&key) != config.public_key
    {
        return Err(fail());
    }
    Ok(config)
}

fn validate_manifest(raw: &[u8], config: &UpdateConfig, current: &str) -> UpdateResult<Manifest> {
    if raw.is_empty() || raw.len() > MANIFEST_LIMIT {
        return Err(fail());
    }
    let manifest: Manifest = serde_json::from_slice(raw).map_err(|_| fail())?;
    if !bounded_version(&manifest.version)
        || manifest.channel != config.channel
        || !hex_string(&manifest.artifact_sha256, 64)
        || !hex_string(&manifest.source_commit, 40)
        || manifest.size == 0
        || manifest.size > ARTIFACT_LIMIT as u64
        || manifest.platforms.len() != 2
        || !manifest.platforms.contains_key("darwin-aarch64")
        || !manifest.platforms.contains_key("darwin-x86_64")
        || manifest
            .notes
            .as_ref()
            .is_some_and(|value| value.len() > 8000)
        || manifest
            .pub_date
            .as_ref()
            .is_some_and(|value| value.len() > 80)
        || semver::Version::parse(&manifest.version).map_err(|_| fail())?
            <= semver::Version::parse(current).map_err(|_| fail())?
    {
        return Err(fail());
    }
    let expected = format!(
        "{}{}{}/{}.app.tar.gz",
        config.origin, config.artifact_prefix, manifest.version, manifest.artifact_sha256
    );
    let first = manifest.platforms.get("darwin-aarch64").ok_or_else(fail)?;
    for platform in manifest.platforms.values() {
        if platform.url != expected
            || platform.signature.is_empty()
            || platform.signature.len() > 8192
            || platform.signature != first.signature
        {
            return Err(fail());
        }
    }
    Ok(manifest)
}

fn valid_handoff(handoff: &UpdateHandoff, now: u64) -> bool {
    handoff.v == 1
        && [
            &handoff.parent_id,
            &handoff.parent_sid,
            &handoff.nonce,
            &handoff.native_session_id,
        ]
        .iter()
        .all(|value| hex_string(value, 32))
        && [&handoff.artifact_sha256, &handoff.resume_ticket_sha256]
            .iter()
            .all(|value| hex_string(value, 64))
        && bounded_version(&handoff.target_version)
        && handoff.original_expires > now
        && handoff.original_expires <= now.saturating_add(30 * 60 * 1000)
        && handoff.resume_deadline > now
        && handoff.resume_deadline <= handoff.original_expires
        && handoff.resume_deadline <= now.saturating_add(RESUME_MS)
}
fn ack_valid(secret: &str, handoff: &UpdateHandoff, proof: &str) -> bool {
    if !hex_string(secret, 64) || !hex_string(proof, 64) {
        return false;
    }
    let (Ok(key), Ok(proof), Ok(wire)) = (
        hex::decode(secret),
        hex::decode(proof),
        serde_json::to_vec(handoff),
    ) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(&key) else {
        return false;
    };
    mac.update(ACK_DOMAIN.as_bytes());
    mac.update(&wire);
    mac.verify_slice(&proof).is_ok()
}
fn handoff_matches(
    pending: &PendingHandoff,
    handoff: &UpdateHandoff,
    ticket: &str,
    proof: &str,
    now: u64,
) -> bool {
    let public = &pending.public;
    !pending.acknowledged
        && valid_handoff(handoff, now)
        && hex_string(ticket, 64)
        && handoff.nonce == public.nonce
        && handoff.target_version == public.target_version
        && handoff.artifact_sha256 == public.artifact_sha256
        && handoff.native_session_id == public.native_session_id
        && handoff.original_expires == public.original_expires
        && handoff.resume_deadline == public.resume_deadline
        && handoff.resume_ticket_sha256 == digest(ticket.as_bytes())
        && ack_valid(&public.resume_auth, handoff, proof)
}

#[cfg(target_os = "macos")]
fn clock_binding() -> UpdateResult<(String, u64)> {
    use std::ffi::CString;
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_continuous_time() -> u64;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    let name = CString::new("kern.bootsessionuuid").map_err(|_| fail())?;
    let mut boot = [0_u8; 128];
    let mut length = boot.len();
    let mut timebase = Timebase { numer: 0, denom: 0 };
    // Fixed, read-only OS identity and continuous uptime. No subprocess or supplied path.
    let status = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            boot.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0
        || length == 0
        || length > boot.len()
        || unsafe { mach_timebase_info(&mut timebase) } != 0
        || timebase.denom == 0
    {
        return Err(fail());
    }
    let boot = std::str::from_utf8(&boot[..length])
        .map_err(|_| fail())?
        .trim_end_matches('\0')
        .to_string();
    if boot.len() != 36
        || !boot
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Err(fail());
    }
    let ticks = unsafe { mach_continuous_time() };
    let nanos = (u128::from(ticks) * u128::from(timebase.numer)) / u128::from(timebase.denom);
    Ok((boot, u64::try_from(nanos).map_err(|_| fail())?))
}
#[cfg(not(target_os = "macos"))]
fn clock_binding() -> UpdateResult<(String, u64)> {
    Err(fail())
}

#[cfg(target_os = "macos")]
fn keychain_options() -> security_framework::passwords::PasswordOptions {
    let mut options = security_framework::passwords::PasswordOptions::new_generic_password(
        KEYCHAIN_SERVICE,
        KEYCHAIN_ACCOUNT,
    );
    options.set_access_synchronized(Some(false));
    options
}
#[cfg(target_os = "macos")]
fn delete_grant() -> UpdateResult<()> {
    match security_framework::passwords::delete_generic_password_options(keychain_options()) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == -25300 => Ok(()),
        Err(_) => Err(fail()),
    }
}
#[cfg(not(target_os = "macos"))]
fn delete_grant() -> UpdateResult<()> {
    Ok(())
}
#[cfg(target_os = "macos")]
fn store_grant(grant: &ResumeGrant) -> UpdateResult<()> {
    let mut bytes = serde_json::to_vec(grant).map_err(|_| fail())?;
    let result = if bytes.len() <= 8192 {
        security_framework::passwords::set_generic_password_options(&bytes, keychain_options())
            .map_err(|_| fail())
    } else {
        Err(fail())
    };
    bytes.fill(0);
    result
}
#[cfg(not(target_os = "macos"))]
fn store_grant(_grant: &ResumeGrant) -> UpdateResult<()> {
    Err(fail())
}
#[cfg(target_os = "macos")]
fn consume_grant() -> UpdateResult<Option<ResumeGrant>> {
    let mut bytes = match security_framework::passwords::generic_password(keychain_options()) {
        Ok(bytes) => bytes,
        Err(error) if error.code() == -25300 => return Ok(None),
        Err(_) => return Err(fail()),
    };
    // A failed deletion must never authorize a connection. No filesystem fallback.
    // NOT_FOUND after a successful read means another process won the consume
    // race; unlike ordinary cleanup it must not authorize a second consumer.
    let deletion =
        security_framework::passwords::delete_generic_password_options(keychain_options())
            .map_err(|_| fail());
    let parsed = if deletion.is_ok() && bytes.len() <= 8192 {
        serde_json::from_slice(&bytes).ok()
    } else {
        None
    };
    bytes.fill(0);
    deletion?;
    Ok(parsed)
}
#[cfg(not(target_os = "macos"))]
fn consume_grant() -> UpdateResult<Option<ResumeGrant>> {
    Ok(None)
}

fn grant_valid(
    grant: &ResumeGrant,
    boot: &str,
    nanos: u64,
    now: u64,
    version: &str,
    executable_sha256: &str,
    identity: &(String, String, String),
) -> bool {
    grant.v == 1
        && grant.armed
        && valid_handoff(&grant.resume.handoff, now)
        && grant.boot == boot
        && grant.issued_ns <= nanos
        && nanos < grant.deadline_ns
        && nanos < grant.original_deadline_ns
        && grant.deadline_ns.saturating_sub(grant.issued_ns) <= RESUME_MS * 1_000_000
        && grant.resume.handoff.target_version == version
        && grant.executable_sha256 == executable_sha256
        && hex_string(executable_sha256, 64)
        && hex_string(&grant.resume.resume_ticket, 64)
        && hex_string(&grant.resume.resume_auth, 64)
        && grant.resume.handoff.resume_ticket_sha256
            == digest(grant.resume.resume_ticket.as_bytes())
        && (
            &grant.origin,
            &grant.operator_public_key,
            &grant.operator_label,
        ) == (&identity.0, &identity.1, &identity.2)
}

fn launch_proof_valid(grant: &ResumeGrant, proof: &str) -> bool {
    hex_string(proof, 64)
        && hex_string(&grant.launch_proof_sha256, 64)
        && digest(proof.as_bytes()) == grant.launch_proof_sha256
}

fn executable_digest(app: &tauri::AppHandle) -> UpdateResult<String> {
    let path = tauri::process::current_binary(&app.env()).map_err(|_| fail())?;
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| fail())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 128 * 1024 * 1024 {
        return Err(fail());
    }
    let mut file = File::open(path).map_err(|_| fail())?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let count = file.read(&mut buffer).map_err(|_| fail())?;
        if count == 0 {
            break;
        }
        total = total.checked_add(count as u64).ok_or_else(fail)?;
        if total > 128 * 1024 * 1024 {
            return Err(fail());
        }
        digest.update(&buffer[..count]);
    }
    if total != metadata.len() {
        return Err(fail());
    }
    Ok(hex::encode(digest.finalize()))
}

pub(crate) fn resume_matches(app: &tauri::AppHandle, id: &str) -> bool {
    app.try_state::<UpdaterState>().is_some_and(|state| {
        state.inner.lock().is_ok_and(|broker| {
            broker
                .handoff
                .as_ref()
                .is_some_and(|value| value.public.native_session_id == id)
                || broker
                    .ready_resume
                    .as_ref()
                    .is_some_and(|value| value.resume.handoff.native_session_id == id)
                || broker.resuming_id.as_deref() == Some(id)
        })
    })
}
pub(crate) fn revoke_resume_authority(app: &tauri::AppHandle, id: Option<&str>) -> bool {
    let Some(state) = app.try_state::<UpdaterState>() else {
        return false;
    };
    // Revocation does not wait for the broker lock, let alone a Keychain prompt.
    if !invalidate_epoch_for_id(&state.revocation_epoch, &state.bound_id, id) {
        return false;
    }
    state.changed.notify_all();
    {
        let mut broker = state
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        broker.handoff = None;
        broker.ready_resume = None;
        broker.restart_armed = false;
        broker.resuming_id = None;
        broker.relaunch = None;
    }
    *state
        .bound_id
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    true
}
pub(crate) fn cleanup_revoked_resume() {
    // Close/Exit and Stop must never wait for a Keychain permission prompt.
    // Authority has already been revoked independently of this best-effort I/O.
    std::thread::spawn(|| {
        let _ = delete_grant();
    });
}
pub(crate) fn revoke_resume(app: &tauri::AppHandle, id: Option<&str>) {
    if revoke_resume_authority(app, id) {
        cleanup_revoked_resume();
    }
}
pub(crate) fn expected_update_restart(app: &tauri::AppHandle) -> bool {
    app.try_state::<UpdaterState>().is_some_and(|state| {
        state.inner.lock().is_ok_and(|broker| {
            broker.restart_armed
                && broker.handoff.as_ref().is_some_and(|value| {
                    value.acknowledged
                        && value.grant.armed
                        && value.generation == state.revocation_epoch.load(Ordering::SeqCst)
                        && now_ms().is_ok_and(|now| now < value.public.resume_deadline)
                })
        })
    })
}

#[tauri::command]
pub(crate) fn poll_support_update_handoff(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    session_id: String,
) -> UpdateResult<Option<PendingUpdateHandoff>> {
    if !crate::support_session::update_window_authorized(&app, window.label(), &session_id) {
        return Err(fail());
    }
    let state = app.state::<UpdaterState>();
    let broker = state.inner.lock().map_err(|_| fail())?;
    Ok(broker
        .handoff
        .as_ref()
        .filter(|value| {
            value.public.native_session_id == session_id
                && value.generation == state.revocation_epoch.load(Ordering::SeqCst)
                && !value.acknowledged
                && !value.saving
                && now_ms().is_ok_and(|now| now < value.public.resume_deadline)
        })
        .map(|value| value.public.clone()))
}

#[tauri::command(async)]
pub(crate) fn complete_support_update_handoff(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    session_id: String,
    handoff: UpdateHandoff,
    resume_ticket: String,
    proof: String,
) -> UpdateResult<bool> {
    if !crate::support_session::update_window_authorized(&app, window.label(), &session_id) {
        return Err(fail());
    }
    let state = app.state::<UpdaterState>();
    let generation = state.revocation_epoch.load(Ordering::SeqCst);
    let grant = {
        let mut broker = state.inner.lock().map_err(|_| fail())?;
        let pending = broker.handoff.as_mut().ok_or_else(fail)?;
        if pending.generation != generation
            || pending.saving
            || pending.public.native_session_id != session_id
            || !handoff_matches(pending, &handoff, &resume_ticket, &proof, now_ms()?)
        {
            return Err(fail());
        }
        pending.saving = true;
        pending.grant.resume.handoff = handoff;
        pending.grant.resume.resume_ticket = resume_ticket;
        pending.grant.clone()
    };
    if epoch_fenced_io(&state.revocation_epoch, generation, || store_grant(&grant)).is_err() {
        cleanup_revoked_resume();
        return Err(fail());
    }
    let mut broker = state.inner.lock().map_err(|_| fail())?;
    if state.revocation_epoch.load(Ordering::SeqCst) != generation {
        drop(broker);
        cleanup_revoked_resume();
        return Err(fail());
    }
    let pending = broker.handoff.as_mut().ok_or_else(fail)?;
    if pending.generation != generation || !pending.saving {
        return Err(fail());
    }
    pending.saving = false;
    pending.acknowledged = true;
    state.changed.notify_all();
    Ok(true)
}

#[cfg(target_os = "macos")]
async fn bounded_get(client: &reqwest::Client, url: &str, maximum: usize) -> UpdateResult<Vec<u8>> {
    let mut response = client.get(url).send().await.map_err(|_| fail())?;
    if !response.status().is_success()
        || response.url().as_str() != url
        || response
            .headers()
            .get("Content-Encoding")
            .is_some_and(|value| value != "identity")
        || response
            .content_length()
            .is_some_and(|value| value > maximum as u64)
    {
        return Err(fail());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| fail())? {
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|size| size > maximum)
        {
            return Err(fail());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(target_os = "macos")]
async fn checked_download(app: &tauri::AppHandle) -> UpdateResult<Option<VerifiedUpdate>> {
    use tauri_plugin_updater::UpdaterExt;
    let identity = crate::support_session::update_service_identity()?;
    let config = config(CONFIG, &identity.0)?;
    let plugin = app.config().plugins.0.get("updater").ok_or_else(fail)?;
    if plugin.get("pubkey").and_then(serde_json::Value::as_str) != Some(&config.public_key)
        || plugin
            .get("requireSignedVersion")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        || plugin
            .get("allowDowngrades")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        || [
            "dangerousInsecureTransportProtocol",
            "dangerousAcceptInvalidCerts",
            "dangerousAcceptInvalidHostnames",
        ]
        .iter()
        .any(|name| plugin.get(name).and_then(serde_json::Value::as_bool) == Some(true))
    {
        return Err(fail());
    }
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .user_agent("PST-QuickView-Verified-Update")
        .build()
        .map_err(|_| fail())?;
    let raw = bounded_get(
        &client,
        &format!("{}{}", config.origin, config.manifest_path),
        MANIFEST_LIMIT,
    )
    .await?;
    if raw.is_empty() {
        return Ok(None);
    }
    let value: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| fail())?;
    let announced: Manifest = serde_json::from_slice(&raw).map_err(|_| fail())?;
    let current = app.package_info().version.to_string();
    if bounded_version(&announced.version)
        && semver::Version::parse(&announced.version).map_err(|_| fail())?
            <= semver::Version::parse(&current).map_err(|_| fail())?
    {
        return Ok(None);
    }
    let manifest = validate_manifest(&raw, &config, &current)?;
    let updater = app.updater_builder().build().map_err(|_| fail())?;
    let Some(update) = updater.restore_update(value).map_err(|_| fail())? else {
        return Ok(None);
    };
    let bytes = bounded_get(&client, update.download_url.as_ref(), ARTIFACT_LIMIT).await?;
    if bytes.len() as u64 != manifest.size || digest(&bytes) != manifest.artifact_sha256 {
        return Err(fail());
    }
    update.verify(&bytes).map_err(|_| fail())?;
    Ok(Some(VerifiedUpdate {
        id: random_hex(16)?,
        version: manifest.version,
        sha256: manifest.artifact_sha256,
        bytes,
        created: Instant::now(),
        update,
    }))
}

#[tauri::command]
pub(crate) async fn check_verified_update(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> UpdateResult<UpdateStatus> {
    main_only(&window)?;
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err(fail())
    }
    #[cfg(target_os = "macos")]
    {
        let state = app.state::<UpdaterState>();
        let generation = {
            let mut broker = state.inner.lock().map_err(|_| fail())?;
            if broker.checking || broker.installing {
                return Err(fail());
            }
            broker.checking = true;
            broker.pending = None;
            state.revocation_epoch.load(Ordering::SeqCst)
        };
        let result = checked_download(&app).await;
        let mut broker = state.inner.lock().map_err(|_| fail())?;
        broker.checking = false;
        if state.revocation_epoch.load(Ordering::SeqCst) != generation {
            return Err(fail());
        }
        broker.pending = result?;
        Ok(match &broker.pending {
            Some(value) => UpdateStatus {
                available: true,
                pending_id: Some(value.id.clone()),
                version: Some(value.version.clone()),
                size: Some(value.bytes.len() as u64),
            },
            None => UpdateStatus {
                available: false,
                pending_id: None,
                version: None,
                size: None,
            },
        })
    }
}

#[cfg(target_os = "macos")]
fn prepare_handoff(
    app: &tauri::AppHandle,
    pending: &VerifiedUpdate,
    generation: u64,
) -> UpdateResult<()> {
    let parent = crate::support_session::update_parent(app)?;
    let now = now_ms()?;
    let (boot, nanos) = clock_binding()?;
    let deadline = update_resume_deadline_ms(now, parent.expires);
    if deadline <= now.saturating_add(3000) {
        return Err(fail());
    }
    let resume_auth = random_hex(32)?;
    let public = PendingUpdateHandoff {
        v: 1,
        nonce: random_hex(16)?,
        target_version: pending.version.clone(),
        artifact_sha256: pending.sha256.clone(),
        native_session_id: parent.session_id.clone(),
        original_expires: parent.expires,
        resume_deadline: deadline,
        resume_auth: resume_auth.clone(),
    };
    let handoff = UpdateHandoff {
        v: 1,
        parent_id: String::new(),
        parent_sid: String::new(),
        nonce: public.nonce.clone(),
        target_version: public.target_version.clone(),
        artifact_sha256: public.artifact_sha256.clone(),
        native_session_id: parent.session_id.clone(),
        original_expires: parent.expires,
        resume_deadline: deadline,
        resume_ticket_sha256: String::new(),
    };
    let grant = ResumeGrant {
        v: 1,
        armed: false,
        origin: parent.origin,
        operator_public_key: parent.operator_public_key,
        operator_label: parent.operator_label,
        boot,
        issued_ns: nanos,
        deadline_ns: nanos.saturating_add((deadline - now) * 1_000_000),
        original_deadline_ns: nanos.saturating_add(parent.remaining_ms * 1_000_000),
        executable_sha256: String::new(),
        launch_proof_sha256: String::new(),
        resume: ResumeBootstrap {
            handoff,
            resume_ticket: String::new(),
            resume_auth,
            native_remaining_ms: 0,
            native_connect_remaining_ms: 0,
        },
    };
    crate::support_session::pause_for_update(app, &parent.session_id)?;
    let state = app.state::<UpdaterState>();
    let mut broker = state.inner.lock().map_err(|_| fail())?;
    if state.revocation_epoch.load(Ordering::SeqCst) != generation {
        return Err(fail());
    }
    broker.resume_loaded = true;
    *state.bound_id.lock().map_err(|_| fail())? = Some(public.native_session_id.clone());
    broker.handoff = Some(PendingHandoff {
        public,
        grant,
        generation,
        acknowledged: false,
        saving: false,
    });
    Ok(())
}

#[cfg(target_os = "macos")]
async fn wait_for_handoff(app: tauri::AppHandle, generation: u64) -> UpdateResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<UpdaterState>();
        let broker = state.inner.lock().map_err(|_| fail())?;
        let (broker, _) = state
            .changed
            .wait_timeout_while(broker, Duration::from_secs(20), |broker| {
                state.revocation_epoch.load(Ordering::SeqCst) == generation
                    && broker
                        .handoff
                        .as_ref()
                        .is_some_and(|value| !value.acknowledged)
            })
            .map_err(|_| fail())?;
        if state.revocation_epoch.load(Ordering::SeqCst) != generation
            || !broker.handoff.as_ref().is_some_and(|value| {
                value.acknowledged && now_ms().is_ok_and(|now| now < value.public.resume_deadline)
            })
        {
            return Err(fail());
        }
        Ok(())
    })
    .await
    .map_err(|_| fail())?
}

#[tauri::command]
pub(crate) async fn approve_verified_update(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    pending_id: String,
    resume: bool,
) -> UpdateResult<UpdateStatus> {
    main_only(&window)?;
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, pending_id, resume);
        Err(fail())
    }
    #[cfg(target_os = "macos")]
    {
        let state = app.state::<UpdaterState>();
        let original_session_id = crate::support_session::update_parent(&app)
            .ok()
            .map(|parent| parent.session_id);
        let (pending, generation) = {
            let mut broker = state.inner.lock().map_err(|_| fail())?;
            if broker.installing || broker.checking || !hex_string(&pending_id, 32) {
                return Err(fail());
            }
            let pending = broker.pending.take().ok_or_else(fail)?;
            if pending.id != pending_id || pending.created.elapsed() > CANDIDATE_LIFETIME {
                return Err(fail());
            }
            broker.installing = true;
            broker.resume_loaded = true;
            (pending, state.revocation_epoch.load(Ordering::SeqCst))
        };
        let result = async {
            let install_guard = crate::try_begin_update_install(&app)?;
            let restart_path = tauri::process::current_binary(&app.env()).map_err(|_| fail())?;
            let label = if resume { "Update and resume this session" } else { "Update and restart" };
            let description = format!("Install the verified PST QuickView build {}?\n\n{}\n\nOriginal mail files and workspaces are preserved. Each future update needs a new approval.",
                pending.version, if resume { "Only the same approved helper may reconnect once, within 90 seconds and before this session's original expiry. Stop support cancels that permission." }
                else { "Remote support will stop and will not reconnect automatically." });
            let approved = rfd::AsyncMessageDialog::new().set_title("Verified PST QuickView update")
                .set_description(description).set_buttons(rfd::MessageButtons::OkCancelCustom(label.to_string(), "Cancel".to_string())).show().await;
            if approved != rfd::MessageDialogResult::Custom(label.to_string()) { return Err(fail()); }
            if state.revocation_epoch.load(Ordering::SeqCst) != generation { return Err(fail()); }
            if resume { prepare_handoff(&app, &pending, generation)?; wait_for_handoff(app.clone(), generation).await?; }
            else { crate::support_session::stop_for_update_failure(&app, None); }
            // The installer replaces this fixed bundle, so user mail/workspaces
            // accidentally stored inside it must block replacement altogether.
            let install_executable = tauri::process::current_binary(&app.env()).map_err(|_| fail())?;
            crate::update_bundle::preflight(&install_executable).map_err(|_| FAILED.to_string())?;
            if resume && state.revocation_epoch.load(Ordering::SeqCst) != generation { return Err(fail()); }
            // Installation re-verifies the same immutable bytes and announced signed version.
            pending.update.install(&pending.bytes).map_err(|_| FAILED.to_string())?;
            if resume {
                let executable_sha256 = epoch_fenced_io(&state.revocation_epoch, generation, || executable_digest(&app))?;
                let launch_proof = random_hex(32)?;
                let grant = {
                    let broker = state.inner.lock().map_err(|_| fail())?;
                    if state.revocation_epoch.load(Ordering::SeqCst) != generation { return Err(fail()); }
                    let handoff = broker.handoff.as_ref().ok_or_else(fail)?;
                    if !handoff.acknowledged || now_ms()? >= handoff.public.resume_deadline { return Err(fail()); }
                    let mut grant = handoff.grant.clone();
                    grant.executable_sha256 = executable_sha256.clone(); grant.armed = true;
                    grant.launch_proof_sha256 = digest(launch_proof.as_bytes()); grant
                }
                ;
                // No broker mutex is held during Keychain or filesystem I/O.
                if epoch_fenced_io(&state.revocation_epoch, generation, || store_grant(&grant)).is_err() {
                    cleanup_revoked_resume(); return Err(fail());
                }
                if epoch_fenced_io(&state.revocation_epoch, generation, || executable_digest(&app))? != executable_sha256 {
                    cleanup_revoked_resume(); return Err(fail());
                }
                {
                    let mut broker = state.inner.lock().map_err(|_| fail())?;
                    if state.revocation_epoch.load(Ordering::SeqCst) != generation { drop(broker); cleanup_revoked_resume(); return Err(fail()); }
                    let handoff = broker.handoff.as_mut().ok_or_else(fail)?;
                    if !handoff.acknowledged || handoff.generation != generation { return Err(fail()); }
                    handoff.grant = grant;
                    broker.relaunch = Some(PendingRelaunch { path: restart_path, proof: launch_proof, executable_sha256, generation });
                    broker.restart_armed = true; broker.install_guard = Some(install_guard);
                }
                // Actual launch happens only in Exit, after old window authority
                // is closed and a final independent revocation check succeeds.
                app.exit(0);
            } else {
                state.inner.lock().map_err(|_| fail())?.install_guard = Some(install_guard);
                app.request_restart();
            }
            Ok(UpdateStatus { available: false, pending_id: None, version: Some(pending.version.clone()), size: None })
        }.await;
        if let Ok(mut broker) = state.inner.lock() {
            broker.installing = false;
        }
        if result.is_err() {
            if let Some(id) = original_session_id.as_deref() {
                crate::support_session::stop_for_update_failure(&app, Some(id));
            } else {
                revoke_resume(&app, None);
            }
        }
        result
    }
}

pub(crate) fn finish_expected_update_exit(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<UpdaterState>() else {
        return;
    };
    let relaunch = {
        let mut broker = state
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        broker.relaunch.take()
    };
    let Some(relaunch) = relaunch else {
        return;
    };
    let result = epoch_fenced_io(&state.revocation_epoch, relaunch.generation, || {
        executable_digest(app)
    });
    if result.is_ok_and(|hash| hash == relaunch.executable_sha256)
        && state.revocation_epoch.load(Ordering::SeqCst) == relaunch.generation
        && now_ms().is_ok_and(|now| {
            state.inner.lock().is_ok_and(|broker| {
                broker.handoff.as_ref().is_some_and(|pending| {
                    pending.acknowledged && now < pending.public.resume_deadline
                })
            })
        })
        && state.revocation_epoch.load(Ordering::SeqCst) == relaunch.generation
        && std::process::Command::new(&relaunch.path)
            .env(LAUNCH_PROOF_ENV, &relaunch.proof)
            .spawn()
            .is_ok()
    {
        return;
    }
    revoke_resume(app, None);
}

fn load_resume_once(app: &tauri::AppHandle) -> UpdateResult<()> {
    let state = app.state::<UpdaterState>();
    let generation = {
        let mut broker = state.inner.lock().map_err(|_| fail())?;
        if broker.resume_loaded {
            return Ok(());
        }
        broker.resume_loaded = true;
        state.revocation_epoch.load(Ordering::SeqCst)
    };
    let proof = LAUNCH_PROOF.get().and_then(take_launch_proof);
    let Some(proof) = proof else {
        delete_grant()?;
        return Ok(());
    };
    let Some(mut grant) = consume_grant()? else {
        return Ok(());
    };
    if !launch_proof_valid(&grant, &proof) {
        return Ok(());
    }
    let identity = crate::support_session::update_service_identity()?;
    let (boot, nanos) = clock_binding()?;
    if !grant_valid(
        &grant,
        &boot,
        nanos,
        now_ms()?,
        &app.package_info().version.to_string(),
        &executable_digest(app)?,
        &identity,
    ) {
        return Ok(());
    }
    grant.resume.native_remaining_ms = grant.original_deadline_ns.saturating_sub(nanos) / 1_000_000;
    grant.resume.native_connect_remaining_ms = grant.deadline_ns.saturating_sub(nanos) / 1_000_000;
    let mut broker = state.inner.lock().map_err(|_| fail())?;
    if state.revocation_epoch.load(Ordering::SeqCst) == generation {
        *state.bound_id.lock().map_err(|_| fail())? =
            Some(grant.resume.handoff.native_session_id.clone());
        broker.ready_resume = Some(grant);
    }
    Ok(())
}

#[tauri::command(async)]
pub(crate) fn support_resume_status(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> UpdateResult<ResumeStatus> {
    main_only(&window)?;
    load_resume_once(&app)?;
    let state = app.state::<UpdaterState>();
    let broker = state.inner.lock().map_err(|_| fail())?;
    Ok(match &broker.ready_resume {
        Some(grant) if now_ms().is_ok_and(|now| now < grant.resume.handoff.resume_deadline) => {
            ResumeStatus {
                available: true,
                session_id: Some(grant.resume.handoff.native_session_id.clone()),
                version: Some(grant.resume.handoff.target_version.clone()),
                expires_at: Some(grant.resume.handoff.original_expires),
                operator_label: Some(grant.operator_label.clone()),
            }
        }
        _ => ResumeStatus {
            available: false,
            session_id: None,
            version: None,
            expires_at: None,
            operator_label: None,
        },
    })
}

#[tauri::command(async)]
pub(crate) fn resume_support_after_update(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> UpdateResult<crate::support_session::SessionStatus> {
    main_only(&window)?;
    if !window.is_visible().unwrap_or(false) || !window.is_focused().unwrap_or(false) {
        return Err(fail());
    }
    load_resume_once(&app)?;
    let state = app.state::<UpdaterState>();
    let (mut grant, generation) = {
        let mut broker = state.inner.lock().map_err(|_| fail())?;
        let grant = broker.ready_resume.take().ok_or_else(fail)?;
        broker.resuming_id = Some(grant.resume.handoff.native_session_id.clone());
        (grant, state.revocation_epoch.load(Ordering::SeqCst))
    };
    let (boot, nanos) = clock_binding()?;
    if !grant_valid(
        &grant,
        &boot,
        nanos,
        now_ms()?,
        &app.package_info().version.to_string(),
        &executable_digest(&app)?,
        &crate::support_session::update_service_identity()?,
    ) {
        return Err(fail());
    }
    grant.resume.native_remaining_ms = grant.original_deadline_ns.saturating_sub(nanos) / 1_000_000;
    grant.resume.native_connect_remaining_ms = grant.deadline_ns.saturating_sub(nanos) / 1_000_000;
    let status = crate::support_session::open_resumed_session(app.clone(), grant.resume)?;
    let result = {
        let mut broker = state.inner.lock().map_err(|_| fail())?;
        let result = if state.revocation_epoch.load(Ordering::SeqCst) == generation {
            *state.bound_id.lock().map_err(|_| fail())? = Some(status.session_id.clone());
            crate::support_session::activate_resumed_session(&app, &status.session_id)
        } else {
            Err(fail())
        };
        broker.resuming_id = None;
        result
    };
    if result.is_err() {
        crate::support_session::stop_for_update_failure(&app, Some(&status.session_id));
    }
    result
}

#[tauri::command]
pub(crate) fn support_confirm_update_resume(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    session_id: String,
) -> UpdateResult<bool> {
    crate::support_session::confirm_update_resume(&app, window.label(), &session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn update_deadline_has_clock_margin_without_extending_restart_or_session() {
        let service_now: u64 = 1_000_000;
        let device_now = service_now + 2_000;
        let deadline = update_resume_deadline_ms(device_now, service_now + 200_000);
        assert_eq!(deadline - device_now, 85_000);
        assert!(deadline <= service_now + RESUME_MS);
        assert!(device_now + RESUME_MS > service_now + RESUME_MS);
        assert_eq!(
            update_resume_deadline_ms(device_now, device_now + 1000),
            device_now + 1000
        );
        assert_eq!(update_resume_deadline_ms(u64::MAX, u64::MAX), u64::MAX);
    }
    #[test]
    fn held_keychain_io_cannot_delay_stop_or_commit_after_revocation() {
        use std::sync::{atomic::AtomicBool, mpsc, Arc};
        let epoch = Arc::new(AtomicU64::new(7));
        let binding = Mutex::new(Some("a".repeat(32)));
        let committed = Arc::new(AtomicBool::new(false));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker_epoch = Arc::clone(&epoch);
        let worker_committed = Arc::clone(&committed);
        let worker = std::thread::spawn(move || {
            let result = epoch_fenced_io(&worker_epoch, 7, || {
                entered_tx.send(()).map_err(|_| fail())?;
                release_rx.recv().map_err(|_| fail())?;
                Ok(())
            });
            if result.is_ok() {
                worker_committed.store(true, Ordering::SeqCst);
            }
            result
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        // A stale different-session Stop does not revoke this session.
        assert!(!invalidate_epoch_for_id(
            &epoch,
            &binding,
            Some(&"b".repeat(32))
        ));
        assert_eq!(epoch.load(Ordering::SeqCst), 7);
        // The authorized Stop completes while the injected storage operation is
        // still held; it never needs that operation's broker mutex or OS result.
        assert!(invalidate_epoch_for_id(
            &epoch,
            &binding,
            Some(&"a".repeat(32))
        ));
        assert_eq!(epoch.load(Ordering::SeqCst), 8);
        assert!(!committed.load(Ordering::SeqCst));
        release_tx.send(()).unwrap();
        assert!(worker.join().unwrap().is_err());
        assert!(!committed.load(Ordering::SeqCst));
    }
    fn handoff() -> UpdateHandoff {
        UpdateHandoff {
            v: 1,
            parent_id: "1".repeat(32),
            parent_sid: "2".repeat(32),
            nonce: "3".repeat(32),
            target_version: "0.2.0-beta.4-support.2".to_string(),
            artifact_sha256: "4".repeat(64),
            native_session_id: "5".repeat(32),
            original_expires: 1_900_000,
            resume_deadline: 1_080_000,
            resume_ticket_sha256: digest("6".repeat(64).as_bytes()),
        }
    }
    fn proof(secret: &str, value: &UpdateHandoff) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(&hex::decode(secret).unwrap()).unwrap();
        mac.update(ACK_DOMAIN.as_bytes());
        mac.update(&serde_json::to_vec(value).unwrap());
        hex::encode(mac.finalize().into_bytes())
    }
    #[test]
    fn update_ack_is_canonical_bound_and_not_a_generic_permission() {
        let secret = "a".repeat(64);
        let value = handoff();
        let signature = proof(&secret, &value);
        assert!(valid_handoff(&value, 1_000_000));
        assert!(ack_valid(&secret, &value, &signature));
        let wire = serde_json::to_string(&value).unwrap();
        assert!(wire.starts_with("{\"v\":1,\"parentId\":"));
        assert!(wire.find("\"parentSid\"").unwrap() < wire.find("\"nonce\"").unwrap());
        assert!(wire.ends_with(&format!(
            "\"resumeTicketSha256\":\"{}\"}}",
            value.resume_ticket_sha256
        )));
        for change in ["parent", "nonce", "version", "artifact", "expiry", "ticket"] {
            let mut changed = value.clone();
            match change {
                "parent" => changed.parent_id = "b".repeat(32),
                "nonce" => changed.nonce = "b".repeat(32),
                "version" => changed.target_version = "0.2.0-beta.4-support.3".to_string(),
                "artifact" => changed.artifact_sha256 = "b".repeat(64),
                "expiry" => changed.original_expires += 1,
                _ => changed.resume_ticket_sha256 = "b".repeat(64),
            }
            assert!(!ack_valid(&secret, &changed, &signature));
        }
        assert!(!ack_valid(&"b".repeat(64), &value, &signature));
        assert!(!ack_valid(&secret, &value, &signature.to_uppercase()));
        assert!(!valid_handoff(&value, value.resume_deadline));
    }
    #[test]
    fn update_manifest_requires_next_build_exact_channel_origin_hash_and_bounded_size() {
        let config = UpdateConfig {
            v: 1,
            origin: "https://synthetic.synthetic.workers.dev".to_string(),
            manifest_path: "/updates/development.json".to_string(),
            artifact_prefix: "/updates/artifacts/".to_string(),
            public_key: String::new(),
            channel: "support-development".to_string(),
        };
        let mut manifest = serde_json::json!({"version":"0.2.0-beta.4-support.2","channel":"support-development","artifactSha256":"a".repeat(64),
            "size":1024,"sourceCommit":"b".repeat(40),"platforms":{}});
        let url = format!(
            "{}/updates/artifacts/0.2.0-beta.4-support.2/{}.app.tar.gz",
            config.origin,
            "a".repeat(64)
        );
        for key in ["darwin-aarch64", "darwin-x86_64"] {
            manifest["platforms"][key] = serde_json::json!({"url":url,"signature":"synthetic"});
        }
        let valid = |value: &serde_json::Value| {
            validate_manifest(
                &serde_json::to_vec(value).unwrap(),
                &config,
                "0.2.0-beta.4-support.1",
            )
            .is_ok()
        };
        assert!(valid(&manifest));
        for (key, value) in [
            ("version", serde_json::json!("0.2.0-beta.4-support.1")),
            ("channel", serde_json::json!("release")),
            ("size", serde_json::json!(ARTIFACT_LIMIT + 1)),
            ("artifactSha256", serde_json::json!("x".repeat(64))),
            ("sourceCommit", serde_json::json!("a")),
        ] {
            let mut changed = manifest.clone();
            changed[key] = value;
            assert!(!valid(&changed));
        }
        manifest["platforms"]["darwin-aarch64"]["url"] =
            serde_json::json!("https://untrusted.invalid/archive");
        assert!(!valid(&manifest));
    }
    #[test]
    fn resumed_grant_requires_same_boot_exact_build_same_helper_and_unexpired_monotonic_deadline() {
        let identity = (
            "https://synthetic.synthetic.workers.dev".to_string(),
            "synthetic-operator-pin".to_string(),
            "Fictional helper".to_string(),
        );
        let value = handoff();
        let version = value.target_version.clone();
        let executable = "b".repeat(64);
        let grant = ResumeGrant {
            v: 1,
            armed: true,
            origin: identity.0.clone(),
            operator_public_key: identity.1.clone(),
            operator_label: identity.2.clone(),
            boot: "synthetic-boot".to_string(),
            issued_ns: 1_000,
            deadline_ns: 80_000_001_000,
            original_deadline_ns: 900_000_001_000,
            executable_sha256: executable.clone(),
            launch_proof_sha256: digest("c".repeat(64).as_bytes()),
            resume: ResumeBootstrap {
                handoff: value,
                resume_ticket: "6".repeat(64),
                resume_auth: "a".repeat(64),
                native_remaining_ms: 0,
                native_connect_remaining_ms: 0,
            },
        };
        assert!(grant_valid(
            &grant,
            "synthetic-boot",
            1_001,
            1_000_000,
            &version,
            &executable,
            &identity
        ));
        assert!(launch_proof_valid(&grant, &"c".repeat(64)));
        assert!(!launch_proof_valid(&grant, ""));
        assert!(!launch_proof_valid(&grant, &"d".repeat(64)));
        let one_use = Mutex::new(Some("c".repeat(64)));
        assert!(take_launch_proof(&one_use).is_some());
        assert!(take_launch_proof(&one_use).is_none());
        assert!(!grant_valid(
            &grant,
            "different-boot",
            1_001,
            1_000_000,
            &version,
            &executable,
            &identity
        ));
        assert!(!grant_valid(
            &grant,
            "synthetic-boot",
            grant.deadline_ns,
            1_000_000,
            &version,
            &executable,
            &identity
        ));
        assert!(!grant_valid(
            &grant,
            "synthetic-boot",
            1_001,
            1_000_000,
            "0.2.0-beta.4-support.3",
            &executable,
            &identity
        ));
        assert!(!grant_valid(
            &grant,
            "synthetic-boot",
            1_001,
            1_000_000,
            &version,
            &"c".repeat(64),
            &identity
        ));
        let other = (
            identity.0.clone(),
            "different-pin".to_string(),
            identity.2.clone(),
        );
        assert!(!grant_valid(
            &grant,
            "synthetic-boot",
            1_001,
            1_000_000,
            &version,
            &executable,
            &other
        ));
        let mut prepared = grant.clone();
        prepared.armed = false;
        assert!(!grant_valid(
            &prepared,
            "synthetic-boot",
            1_001,
            1_000_000,
            &version,
            &executable,
            &identity
        ));
    }
}
