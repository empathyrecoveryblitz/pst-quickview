//! Independent, offline verification of the public development update artifact.
//! This does not install/extract an app or prove packaged GUI readiness.
use base64::{engine::general_purpose::STANDARD, Engine};
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{env, fs::File, io::Read, path::Path, process::ExitCode};

const CONFIG_LIMIT: usize = 8192;
const MANIFEST_LIMIT: usize = 64 * 1024;
const ARCHIVE_LIMIT: usize = 25 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
enum Failure {
    Arguments,
    File,
    Configuration,
    Manifest,
    Provenance,
    Archive,
    Signature,
    SignedVersion,
}
impl Failure {
    fn category(&self) -> &'static str {
        match self {
            Self::Arguments => "argument_count",
            Self::File => "input_file_unavailable",
            Self::Configuration => "invalid_public_configuration",
            Self::Manifest => "invalid_update_manifest",
            Self::Provenance => "source_metadata_mismatch",
            Self::Archive => "archive_integrity_mismatch",
            Self::Signature => "artifact_signature_invalid",
            Self::SignedVersion => "signed_version_missing_or_mismatched",
        }
    }
}
type Checked<T> = Result<T, Failure>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Configuration {
    v: u8,
    origin: String,
    manifest_path: String,
    artifact_prefix: String,
    public_key: String,
    channel: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Platform {
    url: String,
    signature: String,
}
// Typed fields reject duplicated targets instead of overwriting map entries.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Platforms {
    #[serde(rename = "darwin-aarch64")]
    arm64: Platform,
    #[serde(rename = "darwin-x86_64")]
    x86_64: Platform,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    version: String,
    platforms: Platforms,
    channel: String,
    artifact_sha256: String,
    size: u64,
    source_commit: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default, rename = "pub_date")]
    pub_date: Option<String>,
}

fn hex_string(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn cloudflare_origin(origin: &str) -> bool {
    let Some(host) = origin.strip_prefix("https://") else {
        return false;
    };
    let Some(prefix) = host.strip_suffix(".workers.dev") else {
        return false;
    };
    let labels: Vec<_> = prefix.split('.').collect();
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
fn encoded_text(value: &str, maximum: usize) -> Checked<String> {
    if value.is_empty() || value.len() > maximum {
        return Err(Failure::Signature);
    }
    let decoded = STANDARD.decode(value).map_err(|_| Failure::Signature)?;
    if STANDARD.encode(&decoded) != value {
        return Err(Failure::Signature);
    }
    String::from_utf8(decoded).map_err(|_| Failure::Signature)
}
fn configuration(raw: &[u8]) -> Checked<Configuration> {
    if raw.is_empty() || raw.len() > CONFIG_LIMIT {
        return Err(Failure::Configuration);
    }
    let config: Configuration = serde_json::from_slice(raw).map_err(|_| Failure::Configuration)?;
    if config.v != 1
        || !cloudflare_origin(&config.origin)
        || config.manifest_path != "/updates/development.json"
        || config.artifact_prefix != "/updates/artifacts/"
        || config.channel != "support-development"
    {
        return Err(Failure::Configuration);
    }
    let key = encoded_text(&config.public_key, CONFIG_LIMIT).map_err(|_| Failure::Configuration)?;
    if !(42..=4096).contains(&key.len()) || PublicKey::decode(&key).is_err() {
        return Err(Failure::Configuration);
    }
    Ok(config)
}
fn manifest(
    raw: &[u8],
    config: &Configuration,
    expected_commit: Option<&str>,
) -> Checked<Manifest> {
    if raw.is_empty() || raw.len() > MANIFEST_LIMIT {
        return Err(Failure::Manifest);
    }
    let value: Manifest = serde_json::from_slice(raw).map_err(|_| Failure::Manifest)?;
    let version = Version::parse(&value.version).map_err(|_| Failure::Manifest)?;
    if value.version.len() > 80
        || !value.version.is_ascii()
        || !version.build.is_empty()
        || value.channel != config.channel
        || !hex_string(&value.artifact_sha256, 64)
        || !hex_string(&value.source_commit, 40)
        || value.size == 0
        || value.size > ARCHIVE_LIMIT as u64
        || value.notes.as_ref().is_some_and(|notes| notes.len() > 8000)
        || value.pub_date.as_ref().is_some_and(|date| {
            date.len() > 80
                || time::OffsetDateTime::parse(date, &time::format_description::well_known::Rfc3339)
                    .is_err()
        })
    {
        return Err(Failure::Manifest);
    }
    if expected_commit
        .is_some_and(|expected| !hex_string(expected, 40) || value.source_commit != expected)
    {
        return Err(Failure::Provenance);
    }
    let expected_url = format!(
        "{}{}{}/{}.app.tar.gz",
        config.origin, config.artifact_prefix, value.version, value.artifact_sha256
    );
    for platform in [&value.platforms.arm64, &value.platforms.x86_64] {
        if platform.url != expected_url
            || platform.signature.is_empty()
            || platform.signature.len() > 8192
            || platform.signature != value.platforms.arm64.signature
        {
            return Err(Failure::Manifest);
        }
    }
    Ok(value)
}

fn verify_signed_version(trusted_comment: &str, announced: &str) -> Checked<()> {
    // Match Tauri 2.14.0's signed_version / verify_signed_version semantics.
    // This function MUST run only after the library verified the global signature.
    let signed = trusted_comment
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))
        .ok_or(Failure::SignedVersion)?;
    let matches = match (
        Version::parse(signed.trim_start_matches('v')),
        Version::parse(announced.trim_start_matches('v')),
    ) {
        (Ok(signed), Ok(announced)) => signed == announced,
        _ => signed == announced,
    };
    if matches {
        Ok(())
    } else {
        Err(Failure::SignedVersion)
    }
}
fn verify_signature(
    archive: &[u8],
    signature: &str,
    public_key: &str,
    version: &str,
) -> Checked<()> {
    // Official verifier and ordering, not custom cryptography:
    // https://github.com/tauri-apps/plugins-workspace/blob/updater-v2.14.0/plugins/updater/src/updater.rs
    let public_key = PublicKey::decode(&encoded_text(public_key, CONFIG_LIMIT)?)
        .map_err(|_| Failure::Signature)?;
    let signature =
        Signature::decode(&encoded_text(signature, 8192)?).map_err(|_| Failure::Signature)?;
    public_key
        .verify(archive, &signature, true)
        .map_err(|_| Failure::Signature)?;
    verify_signed_version(signature.trusted_comment(), version)
}
fn verify_archive(archive: &[u8], config: &Configuration, manifest: &Manifest) -> Checked<()> {
    if archive.is_empty()
        || archive.len() > ARCHIVE_LIMIT
        || archive.len() as u64 != manifest.size
        || digest(archive) != manifest.artifact_sha256
        || archive.len() < 20
        || archive[..3] != [0x1f, 0x8b, 8]
        || archive[3] & 0xe0 != 0
    {
        return Err(Failure::Archive);
    }
    verify_signature(
        archive,
        &manifest.platforms.arm64.signature,
        &config.public_key,
        &manifest.version,
    )
}
fn read_bounded(path: &Path, maximum: usize) -> Checked<Vec<u8>> {
    let before = std::fs::symlink_metadata(path).map_err(|_| Failure::File)?;
    if !before.is_file() || before.file_type().is_symlink() || before.len() > maximum as u64 {
        return Err(Failure::File);
    }
    let file = File::open(path).map_err(|_| Failure::File)?;
    let opened = file.metadata().map_err(|_| Failure::File)?;
    let after = std::fs::symlink_metadata(path).map_err(|_| Failure::File)?;
    if !opened.is_file() || after.file_type().is_symlink() || opened.len() > maximum as u64 {
        return Err(Failure::File);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != opened.dev()
            || before.ino() != opened.ino()
            || after.dev() != opened.dev()
            || after.ino() != opened.ino()
        {
            return Err(Failure::File);
        }
    }
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Failure::File)?;
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(Failure::File);
    }
    Ok(bytes)
}
fn run(arguments: &[std::ffi::OsString]) -> Checked<()> {
    if !matches!(arguments.len(), 4 | 5) {
        return Err(Failure::Arguments);
    }
    let expected_commit = arguments
        .get(4)
        .map(|value| value.to_str().ok_or(Failure::Provenance))
        .transpose()?;
    let config = configuration(&read_bounded(Path::new(&arguments[1]), CONFIG_LIMIT)?)?;
    let manifest = manifest(
        &read_bounded(Path::new(&arguments[2]), MANIFEST_LIMIT)?,
        &config,
        expected_commit,
    )?;
    let archive = read_bounded(Path::new(&arguments[3]), ARCHIVE_LIMIT)?;
    verify_archive(&archive, &config, &manifest)
}
fn main() -> ExitCode {
    match run(&env::args_os().collect::<Vec<_>>()) {
        Ok(()) => {
            println!("PASS: trusted_update_artifact_verified");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("FAIL: {}", error.category());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Published throwaway vectors from Tauri's updater 2.14.0 offline_tests.
    // They authenticate the synthetic bytes `test`; no private key is present.
    const PUBLIC_KEY: &str = "untrusted comment: minisign public key: A49EF4746FA6E6D7\nRWTX5qZvdPSepDGWg2bz8n6ArQnBMwgQK1geVosorcbLzcunD+KJ+cYe";
    const SIGNATURE: &str = "untrusted comment: signature from tauri secret key\nRUTX5qZvdPSepPF2zsjvP/L/TQtBLRnWRvaq+4omK1UEugtQp/USrGiI1v762sDy3id3aor7bRC9ETyeot6lMHHrWoZ5hjUM1Qc=\ntrusted comment: timestamp:1791649051\tfile:payload\tversion:2.0.0\nDDq2CLOEl5cQmC2eJRHlnZPVViOZsu1CjpPKQoei4A/XBfkDqGO0B+QMrcrHgEQ5qV3mne75tdFWhJpqUXh9AA==";
    const UNVERSIONED_KEY: &str = "untrusted comment: minisign public key E7620F1842B4E81F\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const UNVERSIONED_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1633700835\tfile:test\tprehashed\nwLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==";
    fn config() -> Configuration {
        configuration(
            &serde_json::to_vec(
                &json!({ "v":1,"origin":"https://synthetic.example.workers.dev",
            "manifestPath":"/updates/development.json","artifactPrefix":"/updates/artifacts/",
            "publicKey":STANDARD.encode(PUBLIC_KEY),"channel":"support-development" }),
            )
            .unwrap(),
        )
        .unwrap()
    }
    fn metadata() -> serde_json::Value {
        let url = format!(
            "https://synthetic.example.workers.dev/updates/artifacts/2.0.0/{}.app.tar.gz",
            digest(b"test")
        );
        json!({"version":"2.0.0","channel":"support-development","artifactSha256":digest(b"test"),"size":4,
            "sourceCommit":"a".repeat(40),"notes":"Synthetic test","pub_date":"2026-01-01T00:00:00Z",
            "platforms":{"darwin-aarch64":{"url":url,"signature":STANDARD.encode(SIGNATURE)},
            "darwin-x86_64":{"url":url,"signature":STANDARD.encode(SIGNATURE)}}})
    }
    #[test]
    fn official_signature_vector_and_authenticated_version_pass() {
        assert_eq!(
            verify_signature(
                b"test",
                &STANDARD.encode(SIGNATURE),
                &STANDARD.encode(PUBLIC_KEY),
                "2.0.0"
            ),
            Ok(())
        );
        assert_eq!(
            verify_signed_version("timestamp:1\tfile:test\tversion:v2.0.0", "2.0.0"),
            Ok(())
        );
    }
    #[test]
    fn artifact_comment_signature_key_and_announced_version_tampering_fail() {
        for (bytes, signature, key, version) in [
            (
                &b"tampered"[..],
                SIGNATURE.to_string(),
                PUBLIC_KEY.to_string(),
                "2.0.0",
            ),
            (
                &b"test"[..],
                SIGNATURE.replace("file:payload", "file:changed"),
                PUBLIC_KEY.to_string(),
                "2.0.0",
            ),
            (
                &b"test"[..],
                SIGNATURE.to_string(),
                PUBLIC_KEY.replace("RWTX", "RWTY"),
                "2.0.0",
            ),
            (
                &b"test"[..],
                SIGNATURE.to_string(),
                PUBLIC_KEY.to_string(),
                "3.0.0",
            ),
        ] {
            assert!(verify_signature(
                bytes,
                &STANDARD.encode(signature),
                &STANDARD.encode(key),
                version
            )
            .is_err());
        }
        assert_eq!(
            verify_signature(
                b"test",
                &STANDARD.encode(UNVERSIONED_SIGNATURE),
                &STANDARD.encode(UNVERSIONED_KEY),
                "2.0.0"
            ),
            Err(Failure::SignedVersion)
        );
        assert_eq!(
            verify_signature(
                b"test",
                &STANDARD.encode(SIGNATURE.replace("version:2.0.0", "version:3.0.0")),
                &STANDARD.encode(PUBLIC_KEY),
                "2.0.0"
            ),
            Err(Failure::Signature)
        );
    }
    #[test]
    fn manifest_matches_both_architectures_exact_hash_path_and_optional_expected_commit() {
        let config = config();
        assert!(manifest(
            &serde_json::to_vec(&metadata()).unwrap(),
            &config,
            Some(&"a".repeat(40))
        )
        .is_ok());
        assert!(matches!(
            manifest(
                &serde_json::to_vec(&metadata()).unwrap(),
                &config,
                Some(&"b".repeat(40))
            ),
            Err(Failure::Provenance)
        ));
        for changed in [
            json!({"version":"2.0.0+unreviewed"}),
            json!({"size":0}),
            json!({"size":ARCHIVE_LIMIT+1}),
            json!({"channel":"other"}),
            json!({"artifactSha256":"A".repeat(64)}),
            json!({"sourceCommit":"not-a-commit"}),
            json!({"pub_date":"not-a-date"}),
            json!({"extra":"forbidden"}),
        ] {
            let mut value = metadata();
            value
                .as_object_mut()
                .unwrap()
                .extend(changed.as_object().unwrap().clone());
            assert!(manifest(&serde_json::to_vec(&value).unwrap(), &config, None).is_err());
        }
        for url in [
            "http://synthetic.example.workers.dev/archive",
            "https://untrusted.example.workers.dev/archive",
            "https://synthetic.example.workers.dev/updates/artifacts/2.0.0/wrong.app.tar.gz",
        ] {
            let mut value = metadata();
            value["platforms"]["darwin-aarch64"]["url"] = url.into();
            assert!(manifest(&serde_json::to_vec(&value).unwrap(), &config, None).is_err());
        }
        let mut different_signature = metadata();
        different_signature["platforms"]["darwin-x86_64"]["signature"] = "different".into();
        assert!(manifest(
            &serde_json::to_vec(&different_signature).unwrap(),
            &config,
            None
        )
        .is_err());
        let mut extra_platform = metadata();
        extra_platform["platforms"]["other"] =
            extra_platform["platforms"]["darwin-aarch64"].clone();
        assert!(manifest(&serde_json::to_vec(&extra_platform).unwrap(), &config, None).is_err());
    }
    #[test]
    fn strict_schema_rejects_duplicate_fields_and_targets() {
        let config = config();
        let raw = serde_json::to_string(&metadata()).unwrap();
        assert!(manifest(
            raw.replacen("\"version\":", "\"version\":\"2.0.0\",\"version\":", 1)
                .as_bytes(),
            &config,
            None
        )
        .is_err());
        let duplicate = format!(
            "\"darwin-aarch64\":{},\"darwin-aarch64\":",
            metadata()["platforms"]["darwin-aarch64"]
        );
        assert!(manifest(
            raw.replacen("\"darwin-aarch64\":", &duplicate, 1)
                .as_bytes(),
            &config,
            None
        )
        .is_err());
    }
    #[test]
    fn bounded_files_and_archive_format_cannot_be_substituted_for_signature_validation() {
        let config = config();
        let value = manifest(&serde_json::to_vec(&metadata()).unwrap(), &config, None).unwrap();
        assert_eq!(
            verify_archive(b"test", &config, &value),
            Err(Failure::Archive)
        );
        assert_eq!(
            verify_archive(&vec![0; ARCHIVE_LIMIT + 1], &config, &value),
            Err(Failure::Archive)
        );
        assert_eq!(run(&[]), Err(Failure::Arguments));
        assert_eq!(
            read_bounded(
                Path::new("/synthetic/nonexistent/private-sentinel"),
                CONFIG_LIMIT
            ),
            Err(Failure::File)
        );
        for origin in [
            "https://example.workers.dev",
            "https://a.b.workers.dev/",
            "https://a.b.workers.dev:443",
            "http://a.b.workers.dev",
            "https://user@a.b.workers.dev",
            "https://a.b.workers.dev.example",
        ] {
            assert!(!cloudflare_origin(origin));
        }
    }
}
