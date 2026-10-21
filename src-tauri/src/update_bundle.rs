//! Read-only, bounded protection for user data accidentally stored in the app.
//! Only the running executable supplies the install target. No data is moved.
use std::{
    fs::{self, File, Metadata},
    io::Read,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

const REFUSED: &str = "The application bundle cannot safely be replaced. No update was installed.";
const MAX_ENTRIES: usize = 16_384;
const MAX_DEPTH: usize = 32;
const MAX_DURATION: Duration = Duration::from_secs(2);

struct Budget {
    entries: usize,
    maximum: usize,
    depth: usize,
    deadline: Instant,
}

impl Budget {
    fn check(&mut self, depth: usize) -> Result<(), &'static str> {
        self.entries = self.entries.checked_add(1).ok_or(REFUSED)?;
        if self.entries > self.maximum || depth > self.depth || Instant::now() >= self.deadline {
            return Err(REFUSED);
        }
        Ok(())
    }
}

fn data_name(path: &Path) -> Result<bool, &'static str> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(REFUSED)?
        .to_ascii_lowercase();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    Ok(matches!(
        extension.as_str(),
        "pst" | "eml" | "msg" | "sqlite" | "sqlite3" | "sqlite-wal" | "sqlite-shm" | "db"
    ) || name.starts_with(".pst-quickview")
        || name.starts_with("import-manifest")
        || matches!(
            name.as_str(),
            "metadata.json"
                | "import-state.json"
                | "last-verification.json"
                | "local-state.control"
        ))
}

fn readable(metadata: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        mode & 0o444 != 0 && (!metadata.is_dir() || mode & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

fn same(before: &Metadata, after: &Metadata) -> bool {
    let same = before.is_file() == after.is_file()
        && before.is_dir() == after.is_dir()
        && !after.file_type().is_symlink()
        && before.len() == after.len()
        && before
            .modified()
            .ok()
            .zip(after.modified().ok())
            .is_some_and(|(a, b)| a == b);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        same && before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.mode() == after.mode()
    }
    #[cfg(not(unix))]
    same
}

fn checked_metadata(path: &Path) -> Result<Metadata, &'static str> {
    let metadata = fs::symlink_metadata(path).map_err(|_| REFUSED)?;
    if metadata.file_type().is_symlink()
        || (!metadata.is_file() && !metadata.is_dir())
        || !readable(&metadata)
    {
        return Err(REFUSED);
    }
    Ok(metadata)
}

fn bundle_root(executable: &Path) -> Result<PathBuf, &'static str> {
    if !executable.is_absolute()
        || executable
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(REFUSED);
    }
    let macos = executable.parent().ok_or(REFUSED)?;
    let contents = macos.parent().ok_or(REFUSED)?;
    let bundle = contents.parent().ok_or(REFUSED)?;
    if macos.file_name() != Some(std::ffi::OsStr::new("MacOS"))
        || contents.file_name() != Some(std::ffi::OsStr::new("Contents"))
        || bundle.extension() != Some(std::ffi::OsStr::new("app"))
        || !checked_metadata(bundle)?.is_dir()
        || !checked_metadata(contents)?.is_dir()
        || !checked_metadata(macos)?.is_dir()
        || !checked_metadata(executable)?.is_file()
    {
        return Err(REFUSED);
    }
    let root = bundle.canonicalize().map_err(|_| REFUSED)?;
    let expected = root
        .join("Contents")
        .join("MacOS")
        .join(executable.file_name().ok_or(REFUSED)?);
    if executable.canonicalize().map_err(|_| REFUSED)? != expected {
        return Err(REFUSED);
    }
    Ok(root)
}

fn inspect(
    root: &Path,
    path: &Path,
    depth: usize,
    budget: &mut Budget,
) -> Result<(), &'static str> {
    budget.check(depth)?;
    let before = checked_metadata(path)?;
    if data_name(path)? || !path.canonicalize().map_err(|_| REFUSED)?.starts_with(root) {
        return Err(REFUSED);
    }
    if before.is_dir() {
        for entry in fs::read_dir(path).map_err(|_| REFUSED)? {
            let entry = entry.map_err(|_| REFUSED)?;
            inspect(root, &entry.path(), depth + 1, budget)?;
        }
    } else {
        // Check effective readability without loading or recording resource data.
        let mut file = File::open(path).map_err(|_| REFUSED)?;
        let mut byte = [0_u8; 1];
        file.read(&mut byte).map_err(|_| REFUSED)?;
        if !same(&before, &file.metadata().map_err(|_| REFUSED)?) {
            return Err(REFUSED);
        }
    }
    if Instant::now() >= budget.deadline || !same(&before, &checked_metadata(path)?) {
        return Err(REFUSED);
    }
    Ok(())
}

pub(crate) fn preflight(executable: &Path) -> Result<(), &'static str> {
    let root = bundle_root(executable)?;
    let mut budget = Budget {
        entries: 0,
        maximum: MAX_ENTRIES,
        depth: MAX_DEPTH,
        deadline: Instant::now() + MAX_DURATION,
    };
    inspect(&root, &root, 0, &mut budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::hash_map::DefaultHasher,
        hash::{Hash, Hasher},
        sync::atomic::{AtomicU64, Ordering},
        time::SystemTime,
    };
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        root: PathBuf,
        executable: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let token = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "pqv-update-preflight-{}-{token}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::SeqCst)
            ));
            let executable = root.join("Synthetic.app/Contents/MacOS/viewer");
            fs::create_dir_all(executable.parent().unwrap()).unwrap();
            fs::write(&executable, b"synthetic executable").unwrap();
            Self { root, executable }
        }
        fn resource(&self, name: &str) -> PathBuf {
            let path = self
                .root
                .join("Synthetic.app/Contents/Resources")
                .join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn snapshot(path: &Path) -> (u64, u64, SystemTime, Vec<u8>) {
        let bytes = fs::read(path).unwrap();
        let metadata = fs::metadata(path).unwrap();
        let mut hash = DefaultHasher::new();
        bytes.hash(&mut hash);
        (
            hash.finish(),
            metadata.len(),
            metadata.modified().unwrap(),
            bytes,
        )
    }
    #[test]
    fn embedded_mail_and_workspace_data_are_preserved_and_refuse_installation() {
        for name in [
            "fixture.EML",
            "fixture.pst",
            "fixture.Msg",
            ".pst-quickview-workspace",
            ".pst-quickview.noindex/index.sqlite",
            ".pst-quickview/metadata.json",
            "cache/local-state.sqlite",
            "cache/import-manifest.json",
        ] {
            let fixture = Fixture::new();
            let path = fixture.resource(name);
            fs::write(&path, b"synthetic preservation fixture").unwrap();
            let before = snapshot(&path);
            assert!(
                preflight(&fixture.executable).is_err(),
                "embedded data must refuse installation"
            );
            assert_eq!(snapshot(&path), before);
        }
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.resource(".pst-quickview.noindex")).unwrap();
        assert!(preflight(&fixture.executable).is_err());
    }
    #[test]
    fn ordinary_resources_and_external_or_sibling_prefix_data_are_untouched() {
        let fixture = Fixture::new();
        let resource = fixture.resource("style.css");
        fs::write(&resource, b"synthetic style").unwrap();
        let sibling = fixture.root.join("Synthetic.app-extra/fixture.eml");
        fs::create_dir_all(sibling.parent().unwrap()).unwrap();
        fs::write(&sibling, b"external synthetic data").unwrap();
        let before = snapshot(&sibling);
        let normal = snapshot(&resource);
        assert!(preflight(&fixture.executable).is_ok());
        assert_eq!(snapshot(&sibling), before);
        assert_eq!(snapshot(&resource), normal);
        assert!(preflight(&sibling).is_err());
    }
    #[test]
    fn entry_depth_and_duration_bounds_and_invalid_executable_layout_fail_closed() {
        let fixture = Fixture::new();
        let root = bundle_root(&fixture.executable).unwrap();
        for (maximum, depth, duration) in [
            (1, 32, Duration::from_secs(2)),
            (100, 0, Duration::from_secs(2)),
            (100, 32, Duration::ZERO),
        ] {
            let mut budget = Budget {
                entries: 0,
                maximum,
                depth,
                deadline: Instant::now() + duration,
            };
            assert!(inspect(&root, &root, 0, &mut budget).is_err());
        }
        assert!(preflight(Path::new("relative.app/Contents/MacOS/viewer")).is_err());
        assert!(preflight(&fixture.root.join("Synthetic.app/viewer")).is_err());
        assert!(preflight(&fixture.root.join("Synthetic.app/Contents/MacOS/missing")).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_unreadability_and_special_files_refuse_without_following_or_changing_data() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let fixture = Fixture::new();
        let outside = fixture.root.join("external.eml");
        fs::write(&outside, b"synthetic external source").unwrap();
        let before = snapshot(&outside);
        let link = fixture.resource("linked-resource");
        symlink(&outside, &link).unwrap();
        assert!(preflight(&fixture.executable).is_err());
        assert_eq!(snapshot(&outside), before);
        fs::remove_file(&link).unwrap();
        let unreadable = fixture.resource("unreadable.bin");
        fs::write(&unreadable, b"synthetic resource").unwrap();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();
        assert!(preflight(&fixture.executable).is_err());
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o600)).unwrap();
        let directory = fixture.resource("unreadable-directory");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o0)).unwrap();
        assert!(preflight(&fixture.executable).is_err());
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        // macOS's long per-user temporary root can exceed its UNIX socket
        // pathname bound; the file-kind rule is platform-independent.
        #[cfg(target_os = "linux")]
        {
            let socket = fixture.resource("resource.socket");
            let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
            assert!(preflight(&fixture.executable).is_err());
        }
    }
}
