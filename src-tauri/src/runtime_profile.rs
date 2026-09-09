use std::{
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
    sync::OnceLock,
};

use tauri::Manager;

pub(crate) const PRODUCTION_BUNDLE_IDENTIFIER: &str = "com.local.pstquickview";
pub(crate) const PROFILE_LAYOUT_GENERATION: u32 = 1;
// Keep this byte string in packaged executables so the app-only verifier can
// prove that each architecture slice contains the reviewed routing contract.
#[used]
static RUNTIME_PROFILE_CONTRACT: &[u8] =
    b"pst-quickview-runtime-profile-v1:alternate-identifier-scoped:no-production-fallback";
// tauri-build derives these values from the same merged configuration consumed
// by generate_context!(). Keep their unambiguous encoding in each packaged
// slice so static verification can bind the executable to its bundle identity.
#[used]
static RUNTIME_PROFILE_IDENTITY_CONTRACT: &[u8] = concat!(
    "pst-quickview-runtime-identity-v1:",
    env!("TAURI_ANDROID_PACKAGE_NAME_PREFIX"),
    ":",
    env!("TAURI_ANDROID_PACKAGE_NAME_APP_NAME"),
    ":identity-end"
)
.as_bytes();

// Compatibility policy: the production identifier must continue to discover
// the exact profile used by beta.3. Alternate identifiers never use this name.
const LEGACY_PRODUCTION_PROFILE_DIRECTORY: &str = "PST QuickView";
const WORKSPACES_DIRECTORY: &str = "workspaces";
const EML_EXPORTS_DIRECTORY: &str = "eml-exports";
const MSG_EXPORTS_DIRECTORY: &str = "msg-exports";
const LEGACY_LOGS_DIRECTORY: &str = "logs";
const MAX_PRODUCT_NAME_BYTES: usize = 256;
const MAX_BUNDLE_IDENTIFIER_BYTES: usize = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeProfileFlavor {
    ProductionLegacy,
    Alternate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeProfileIdentity {
    product_name: String,
    bundle_identifier: String,
    flavor: RuntimeProfileFlavor,
    profile_layout_generation: u32,
}

impl RuntimeProfileIdentity {
    pub(crate) fn from_config(config: &tauri::Config) -> Result<Self, RuntimeProfileError> {
        let product_name = config
            .product_name
            .as_deref()
            .ok_or(RuntimeProfileError::InvalidProductName)?;
        Self::from_values(product_name, &config.identifier)
    }

    pub(crate) fn from_values(
        product_name: &str,
        bundle_identifier: &str,
    ) -> Result<Self, RuntimeProfileError> {
        validate_product_name(product_name)?;
        validate_bundle_identifier(bundle_identifier)?;

        Ok(Self {
            product_name: product_name.to_string(),
            bundle_identifier: bundle_identifier.to_string(),
            flavor: if bundle_identifier == PRODUCTION_BUNDLE_IDENTIFIER {
                RuntimeProfileFlavor::ProductionLegacy
            } else {
                RuntimeProfileFlavor::Alternate
            },
            profile_layout_generation: PROFILE_LAYOUT_GENERATION,
        })
    }

    pub(crate) fn product_name(&self) -> &str {
        &self.product_name
    }

    pub(crate) fn bundle_identifier(&self) -> &str {
        &self.bundle_identifier
    }

    pub(crate) fn flavor(&self) -> RuntimeProfileFlavor {
        self.flavor
    }

    pub(crate) fn profile_layout_generation(&self) -> u32 {
        self.profile_layout_generation
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeProfilePaths {
    application_data_root: PathBuf,
    log_root: PathBuf,
    cache_root: PathBuf,
    configuration_root: PathBuf,
    workspace_registry_session_root: PathBuf,
    standalone_eml_export_root: PathBuf,
    standalone_msg_export_root: PathBuf,
    diagnostics_root: PathBuf,
}

impl RuntimeProfilePaths {
    pub(crate) fn application_data_root(&self) -> &Path {
        &self.application_data_root
    }

    pub(crate) fn log_root(&self) -> &Path {
        &self.log_root
    }

    pub(crate) fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    pub(crate) fn configuration_root(&self) -> &Path {
        &self.configuration_root
    }

    pub(crate) fn workspace_registry_session_root(&self) -> &Path {
        &self.workspace_registry_session_root
    }

    pub(crate) fn standalone_eml_export_root(&self) -> &Path {
        &self.standalone_eml_export_root
    }

    pub(crate) fn standalone_msg_export_root(&self) -> &Path {
        &self.standalone_msg_export_root
    }

    pub(crate) fn diagnostics_root(&self) -> &Path {
        &self.diagnostics_root
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeProfile {
    identity: RuntimeProfileIdentity,
    paths: RuntimeProfilePaths,
}

impl RuntimeProfile {
    pub(crate) fn resolve<R: tauri::Runtime>(
        app: &tauri::App<R>,
        expected_identity: &RuntimeProfileIdentity,
    ) -> Result<Self, RuntimeProfileError> {
        let merged_identity = RuntimeProfileIdentity::from_config(app.config())?;
        if &merged_identity != expected_identity {
            return Err(RuntimeProfileError::IdentityChanged);
        }

        let roots = ResolvedTauriRoots {
            app_data: app
                .path()
                .app_data_dir()
                .map_err(|_| RuntimeProfileError::TauriPathUnavailable)?,
            app_log: app
                .path()
                .app_log_dir()
                .map_err(|_| RuntimeProfileError::TauriPathUnavailable)?,
            app_cache: app
                .path()
                .app_cache_dir()
                .map_err(|_| RuntimeProfileError::TauriPathUnavailable)?,
            app_config: app
                .path()
                .app_config_dir()
                .map_err(|_| RuntimeProfileError::TauriPathUnavailable)?,
        };
        Self::from_resolved(merged_identity, roots)
    }

    pub(crate) fn from_resolved(
        identity: RuntimeProfileIdentity,
        roots: ResolvedTauriRoots,
    ) -> Result<Self, RuntimeProfileError> {
        roots.validate_for(&identity)?;

        let production_legacy_root = roots
            .app_data
            .parent()
            .ok_or(RuntimeProfileError::TauriPathUnavailable)?
            .join(LEGACY_PRODUCTION_PROFILE_DIRECTORY);

        let (application_data_root, log_root) = match identity.flavor {
            RuntimeProfileFlavor::ProductionLegacy => (
                production_legacy_root.clone(),
                production_legacy_root.join(LEGACY_LOGS_DIRECTORY),
            ),
            RuntimeProfileFlavor::Alternate => {
                for root in [
                    &roots.app_data,
                    &roots.app_log,
                    &roots.app_cache,
                    &roots.app_config,
                ] {
                    validate_existing_alternate_root(root)?;
                    if has_component(root, LEGACY_PRODUCTION_PROFILE_DIRECTORY) {
                        return Err(RuntimeProfileError::AlternateUsesProductionPath);
                    }
                }
                if roots.app_data == production_legacy_root {
                    return Err(RuntimeProfileError::AlternateUsesProductionPath);
                }
                (roots.app_data.clone(), roots.app_log.clone())
            }
        };

        let paths = RuntimeProfilePaths {
            workspace_registry_session_root: application_data_root.join(WORKSPACES_DIRECTORY),
            standalone_eml_export_root: application_data_root.join(EML_EXPORTS_DIRECTORY),
            standalone_msg_export_root: application_data_root.join(MSG_EXPORTS_DIRECTORY),
            diagnostics_root: log_root.clone(),
            application_data_root,
            log_root,
            cache_root: roots.app_cache,
            configuration_root: roots.app_config,
        };

        let profile = Self { identity, paths };
        profile.validate_complete_contract()?;
        Ok(profile)
    }

    pub(crate) fn identity(&self) -> &RuntimeProfileIdentity {
        &self.identity
    }

    pub(crate) fn paths(&self) -> &RuntimeProfilePaths {
        &self.paths
    }

    fn validate_complete_contract(&self) -> Result<(), RuntimeProfileError> {
        let identity = self.identity();
        validate_product_name(identity.product_name())?;
        validate_bundle_identifier(identity.bundle_identifier())?;
        if identity.profile_layout_generation() != PROFILE_LAYOUT_GENERATION
            || (identity.bundle_identifier() == PRODUCTION_BUNDLE_IDENTIFIER)
                != (identity.flavor() == RuntimeProfileFlavor::ProductionLegacy)
        {
            return Err(RuntimeProfileError::IdentityChanged);
        }

        let paths = self.paths();
        for root in [
            paths.application_data_root(),
            paths.log_root(),
            paths.cache_root(),
            paths.configuration_root(),
            paths.diagnostics_root(),
        ] {
            if !root.is_absolute() {
                return Err(RuntimeProfileError::TauriPathUnavailable);
            }
        }
        if paths.workspace_registry_session_root()
            != paths.application_data_root().join(WORKSPACES_DIRECTORY)
            || paths.standalone_eml_export_root()
                != paths.application_data_root().join(EML_EXPORTS_DIRECTORY)
            || paths.standalone_msg_export_root()
                != paths.application_data_root().join(MSG_EXPORTS_DIRECTORY)
            || paths.diagnostics_root() != paths.log_root()
        {
            return Err(RuntimeProfileError::TauriPathUnavailable);
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub(crate) struct RuntimeProfileState(OnceLock<RuntimeProfile>);

impl RuntimeProfileState {
    pub(crate) fn initialize(
        &self,
        profile: RuntimeProfile,
    ) -> Result<&RuntimeProfile, RuntimeProfileError> {
        self.0
            .set(profile)
            .map_err(|_| RuntimeProfileError::AlreadyInitialized)?;
        self.get()
    }

    pub(crate) fn get(&self) -> Result<&RuntimeProfile, RuntimeProfileError> {
        self.0.get().ok_or(RuntimeProfileError::NotInitialized)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedTauriRoots {
    pub(crate) app_data: PathBuf,
    pub(crate) app_log: PathBuf,
    pub(crate) app_cache: PathBuf,
    pub(crate) app_config: PathBuf,
}

impl ResolvedTauriRoots {
    fn validate_for(&self, identity: &RuntimeProfileIdentity) -> Result<(), RuntimeProfileError> {
        for root in [
            &self.app_data,
            &self.app_log,
            &self.app_cache,
            &self.app_config,
        ] {
            validate_identifier_scoped_root(root, identity.bundle_identifier())?;
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum RuntimeProfileError {
    #[error("The packaged product name is invalid.")]
    InvalidProductName,
    #[error("The packaged application identifier is invalid.")]
    InvalidBundleIdentifier,
    #[error("The packaged application identity changed during startup.")]
    IdentityChanged,
    #[error("Identifier-scoped application storage is unavailable.")]
    TauriPathUnavailable,
    #[error("An alternate application profile resolved into production storage.")]
    AlternateUsesProductionPath,
    #[error("An alternate application profile resolved through an unsafe storage root.")]
    UnsafeAlternateRoot,
    #[error("The runtime profile was already initialized.")]
    AlreadyInitialized,
    #[error("The runtime profile has not been initialized.")]
    NotInitialized,
}

fn validate_product_name(product_name: &str) -> Result<(), RuntimeProfileError> {
    if product_name.trim().is_empty()
        || product_name != product_name.trim()
        || product_name.len() > MAX_PRODUCT_NAME_BYTES
        || product_name.chars().any(char::is_control)
    {
        return Err(RuntimeProfileError::InvalidProductName);
    }
    Ok(())
}

fn validate_bundle_identifier(identifier: &str) -> Result<(), RuntimeProfileError> {
    if identifier.is_empty()
        || identifier.len() > MAX_BUNDLE_IDENTIFIER_BYTES
        || !identifier.is_ascii()
    {
        return Err(RuntimeProfileError::InvalidBundleIdentifier);
    }

    let mut segments = identifier.split('.');
    let mut segment_count = 0_usize;
    for segment in segments.by_ref() {
        segment_count += 1;
        if segment.is_empty()
            || !segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || !segment
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !segment
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
        {
            return Err(RuntimeProfileError::InvalidBundleIdentifier);
        }
    }
    if segment_count < 2 {
        return Err(RuntimeProfileError::InvalidBundleIdentifier);
    }
    Ok(())
}

fn validate_identifier_scoped_root(
    root: &Path,
    identifier: &str,
) -> Result<(), RuntimeProfileError> {
    if !root.is_absolute()
        || root.file_name() != Some(OsStr::new(identifier))
        || root
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(RuntimeProfileError::TauriPathUnavailable);
    }
    Ok(())
}

fn validate_existing_alternate_root(root: &Path) -> Result<(), RuntimeProfileError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(RuntimeProfileError::UnsafeAlternateRoot)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(RuntimeProfileError::UnsafeAlternateRoot),
    }
}

fn has_component(path: &Path, expected: &str) -> bool {
    path.components()
        .any(|component| component.as_os_str() == OsStr::new(expected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink, time::SystemTime};

    const PRODUCTION_NAME: &str = "PST QuickView";
    const ALTERNATE_NAME: &str = "PST QuickView Beta4 Runtime Test";
    const ALTERNATE_IDENTIFIER: &str = "com.local.pstquickview.beta4runtimetest.static";

    fn macos_roots(identifier: &str) -> ResolvedTauriRoots {
        let home = Path::new("/synthetic/home/Library");
        ResolvedTauriRoots {
            app_data: home.join("Application Support").join(identifier),
            app_log: home.join("Logs").join(identifier),
            app_cache: home.join("Caches").join(identifier),
            app_config: home.join("Application Support").join(identifier),
        }
    }

    fn alternate_profile(identifier: &str) -> RuntimeProfile {
        RuntimeProfile::from_resolved(
            RuntimeProfileIdentity::from_values(ALTERNATE_NAME, identifier).unwrap(),
            macos_roots(identifier),
        )
        .unwrap()
    }

    #[test]
    fn production_identity_uses_exact_beta3_legacy_profile_layout() {
        let identity =
            RuntimeProfileIdentity::from_values(PRODUCTION_NAME, PRODUCTION_BUNDLE_IDENTIFIER)
                .unwrap();
        let profile =
            RuntimeProfile::from_resolved(identity, macos_roots(PRODUCTION_BUNDLE_IDENTIFIER))
                .unwrap();
        let expected = Path::new("/synthetic/home/Library/Application Support/PST QuickView");

        assert_eq!(
            profile.identity().flavor(),
            RuntimeProfileFlavor::ProductionLegacy
        );
        assert_eq!(profile.paths().application_data_root(), expected);
        assert_eq!(profile.paths().log_root(), expected.join("logs"));
        assert_eq!(
            profile.paths().workspace_registry_session_root(),
            expected.join("workspaces")
        );
        assert_eq!(
            profile.paths().standalone_eml_export_root(),
            expected.join("eml-exports")
        );
        assert_eq!(
            profile.paths().standalone_msg_export_root(),
            expected.join("msg-exports")
        );
        assert_eq!(profile.paths().diagnostics_root(), expected.join("logs"));
    }

    #[test]
    fn alternate_identity_uses_identifier_scoped_roots() {
        let roots = macos_roots(ALTERNATE_IDENTIFIER);
        let profile = alternate_profile(ALTERNATE_IDENTIFIER);

        assert_eq!(profile.identity().flavor(), RuntimeProfileFlavor::Alternate);
        assert_eq!(profile.identity().product_name(), ALTERNATE_NAME);
        assert_eq!(profile.identity().bundle_identifier(), ALTERNATE_IDENTIFIER);
        assert_eq!(
            profile.identity().profile_layout_generation(),
            PROFILE_LAYOUT_GENERATION
        );
        assert_eq!(profile.paths().application_data_root(), roots.app_data);
        assert_eq!(profile.paths().log_root(), roots.app_log);
        assert_eq!(profile.paths().cache_root(), roots.app_cache);
        assert_eq!(profile.paths().configuration_root(), roots.app_config);
        for root in [
            profile.paths().application_data_root(),
            profile.paths().log_root(),
            profile.paths().cache_root(),
            profile.paths().configuration_root(),
        ] {
            assert!(!has_component(root, LEGACY_PRODUCTION_PROFILE_DIRECTORY));
        }
    }

    #[test]
    fn alternate_identities_are_disjoint() {
        let first = alternate_profile(ALTERNATE_IDENTIFIER);
        let second_identifier = "com.local.pstquickview.beta4runtimetest.second";
        let second = alternate_profile(second_identifier);

        assert_ne!(
            first.paths().application_data_root(),
            second.paths().application_data_root()
        );
        assert_ne!(first.paths().log_root(), second.paths().log_root());
        assert_ne!(first.paths().cache_root(), second.paths().cache_root());
        assert_ne!(
            first.paths().configuration_root(),
            second.paths().configuration_root()
        );
    }

    #[test]
    fn profile_identifier_rejects_malformed_and_traversal_values() {
        for invalid in [
            "",
            " ",
            "single",
            ".com.local",
            "com.local.",
            "com..local",
            "com/local.test",
            "com\\local.test",
            "com.local/../production",
            "com.local.-test",
            "com.local.test-",
            "com.local.\nvalue",
        ] {
            assert_eq!(
                RuntimeProfileIdentity::from_values(ALTERNATE_NAME, invalid),
                Err(RuntimeProfileError::InvalidBundleIdentifier),
                "identifier should fail closed: {invalid:?}"
            );
        }
        assert_eq!(
            RuntimeProfileIdentity::from_values(ALTERNATE_NAME, &"a".repeat(256)),
            Err(RuntimeProfileError::InvalidBundleIdentifier)
        );
    }

    #[test]
    fn alternate_resolution_failure_never_falls_back_to_production() {
        let identity =
            RuntimeProfileIdentity::from_values(ALTERNATE_NAME, ALTERNATE_IDENTIFIER).unwrap();
        let root = Path::new("/synthetic/home/Library/Application Support/PST QuickView");
        let hostile = ResolvedTauriRoots {
            app_data: root.join(ALTERNATE_IDENTIFIER),
            app_log: PathBuf::from("relative/log/root").join(ALTERNATE_IDENTIFIER),
            app_cache: root.join("cache").join(ALTERNATE_IDENTIFIER),
            app_config: root.join("config").join(ALTERNATE_IDENTIFIER),
        };

        assert!(RuntimeProfile::from_resolved(identity, hostile).is_err());
    }

    #[test]
    fn alternate_resolution_rejects_a_symlinked_identifier_root_without_following_it() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pst-quickview-runtime-profile-symlink-{}-{unique}",
            std::process::id()
        ));
        let data_parent = root.join("data");
        fs::create_dir_all(&data_parent).unwrap();
        let app_data = data_parent.join(ALTERNATE_IDENTIFIER);
        let nonexistent_legacy_target = data_parent.join(LEGACY_PRODUCTION_PROFILE_DIRECTORY);
        symlink(&nonexistent_legacy_target, &app_data).unwrap();
        let identity =
            RuntimeProfileIdentity::from_values(ALTERNATE_NAME, ALTERNATE_IDENTIFIER).unwrap();
        let roots = ResolvedTauriRoots {
            app_data,
            app_log: root.join("logs").join(ALTERNATE_IDENTIFIER),
            app_cache: root.join("cache").join(ALTERNATE_IDENTIFIER),
            app_config: root.join("config").join(ALTERNATE_IDENTIFIER),
        };

        assert_eq!(
            RuntimeProfile::from_resolved(identity, roots),
            Err(RuntimeProfileError::UnsafeAlternateRoot)
        );
        assert!(!nonexistent_legacy_target.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn profile_resolution_has_no_filesystem_side_effects() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pst-quickview-runtime-profile-resolution-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let identity =
            RuntimeProfileIdentity::from_values(ALTERNATE_NAME, ALTERNATE_IDENTIFIER).unwrap();
        let roots = ResolvedTauriRoots {
            app_data: root.join("data").join(ALTERNATE_IDENTIFIER),
            app_log: root.join("logs").join(ALTERNATE_IDENTIFIER),
            app_cache: root.join("cache").join(ALTERNATE_IDENTIFIER),
            app_config: root.join("config").join(ALTERNATE_IDENTIFIER),
        };

        let before = fs::read_dir(&root).unwrap().count();
        let profile = RuntimeProfile::from_resolved(identity, roots).unwrap();
        assert_eq!(fs::read_dir(&root).unwrap().count(), before);
        assert!(!profile.paths().application_data_root().exists());
        assert!(!profile.paths().log_root().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn all_profile_consumers_share_one_resolved_profile() {
        let profile = alternate_profile(ALTERNATE_IDENTIFIER);
        let data = profile.paths().application_data_root();
        assert_eq!(
            profile.paths().workspace_registry_session_root(),
            data.join(WORKSPACES_DIRECTORY)
        );
        assert_eq!(
            profile.paths().standalone_eml_export_root(),
            data.join(EML_EXPORTS_DIRECTORY)
        );
        assert_eq!(
            profile.paths().standalone_msg_export_root(),
            data.join(MSG_EXPORTS_DIRECTORY)
        );
        assert_eq!(
            profile.paths().diagnostics_root(),
            profile.paths().log_root()
        );
    }

    #[test]
    fn runtime_profile_identity_is_immutable() {
        let state = RuntimeProfileState::default();
        let first = alternate_profile(ALTERNATE_IDENTIFIER);
        let second = alternate_profile("com.local.pstquickview.beta4runtimetest.second");
        state.initialize(first.clone()).unwrap();
        assert_eq!(state.get().unwrap(), &first);
        assert_eq!(
            state.initialize(second),
            Err(RuntimeProfileError::AlreadyInitialized)
        );
        assert_eq!(state.get().unwrap(), &first);
    }
}
