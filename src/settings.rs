//! Working-directory persistence. Failed reads are protected from automatic writes.
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct Store {
    pub root: PathBuf,
    blocked: HashSet<PathBuf>,
    observed: BTreeMap<PathBuf, Value>,
    pending: BTreeMap<PathBuf, (Value, Instant)>,
}
impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            blocked: HashSet::new(),
            observed: BTreeMap::new(),
            pending: BTreeMap::new(),
        }
    }
    pub fn read<T: DeserializeOwned + Serialize>(
        &mut self,
        name: &str,
        defaults: T,
    ) -> Result<T, String> {
        let path = self.root.join(name);
        if let Some((value, _)) = self.pending.get(&path) {
            return serde_json::from_value(value.clone()).map_err(|e| e.to_string());
        }
        let result = (|| {
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(defaults),
                Err(e) => return Err(e.to_string()),
            };
            let stored: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if stored.get("version").and_then(Value::as_u64) != Some(1) {
                return Err("unsupported settings version".into());
            }
            let mut value = serde_json::to_value(&defaults).map_err(|e| e.to_string())?;
            let object = stored
                .get("data")
                .and_then(Value::as_object)
                .ok_or("missing settings data")?;
            let target = value.as_object_mut().ok_or("settings must be an object")?;
            for (key, value) in object {
                target.insert(key.clone(), value.clone());
            }
            serde_json::from_value(value).map_err(|e| e.to_string())
        })();
        match result {
            Ok(value) => {
                self.observed.insert(
                    path,
                    serde_json::to_value(&value).map_err(|e| e.to_string())?,
                );
                Ok(value)
            }
            Err(e) => {
                self.blocked.insert(path.clone());
                Err(format!(
                    "Cannot load {}: {e}. File preserved; automatic saving disabled for this file.",
                    path.display()
                ))
            }
        }
    }
    pub fn latest<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        self.observed
            .get(&self.root.join(name))
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    }
    pub fn queue(&mut self, name: &str, data: &impl Serialize) {
        let path = self.root.join(name);
        if self.blocked.contains(&path) {
            return;
        }
        if let Ok(value) = serde_json::to_value(data)
            && self.observed.get(&path) != Some(&value)
        {
            self.observed.insert(path.clone(), value.clone());
            self.pending.insert(path, (value, Instant::now()));
        }
    }
    pub fn flush(&mut self, force: bool) -> Vec<String> {
        let paths: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, (_, at))| force || at.elapsed() >= Duration::from_millis(500))
            .map(|(p, _)| p.clone())
            .collect();
        let mut errors = Vec::new();
        for path in paths {
            let (value, _) = self.pending.remove(&path).unwrap();
            if let Err(e) = write_atomic(&path, &serde_json::json!({"version": 1, "data": value})) {
                errors.push(format!("Cannot save {}: {e}", path.display()));
                // Retain pending data and retry at a bounded rate, including on shutdown.
                self.pending
                    .insert(path, (value, Instant::now() + Duration::from_secs(5)));
            }
        }
        errors
    }
}
fn write_atomic(path: &Path, value: &Value) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(path.parent().ok_or("missing settings parent")?)
        .map_err(|e| e.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&temporary).map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        replace(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}
#[cfg(not(target_os = "windows"))]
fn replace(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::rename(from, to).map_err(|e| e.to_string())
}
#[cfg(target_os = "windows")]
fn replace(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "miditoroblox-store-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Example {
        enabled: bool,
        delay: u64,
    }
    fn defaults() -> Example {
        Example {
            enabled: true,
            delay: 100,
        }
    }

    #[test]
    fn debounced_roundtrip_replacement_and_pending_reads_use_the_fixed_root() {
        let directory = Directory::new();
        let root = directory.0.join("working-directory/miditoroblox");
        let mut store = Store::new(root.clone());
        assert_eq!(store.read("settings.json", defaults()).unwrap(), defaults());
        let edited = Example {
            enabled: false,
            delay: 80,
        };
        store.queue("settings.json", &edited);
        assert_eq!(store.read("settings.json", defaults()).unwrap(), edited);
        assert!(store.flush(false).is_empty());
        assert!(!root.join("settings.json").exists());
        store
            .pending
            .get_mut(&root.join("settings.json"))
            .unwrap()
            .1 = Instant::now() - Duration::from_secs(1);
        assert!(store.flush(false).is_empty());
        let mut reopened = Store::new(root.clone());
        assert_eq!(reopened.read("settings.json", defaults()).unwrap(), edited);
        store.queue("settings.json", &defaults());
        assert!(store.flush(true).is_empty());
        assert_eq!(reopened.read("settings.json", edited).unwrap(), defaults());
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 1);
    }

    #[test]
    fn missing_fields_use_defaults_but_malformed_and_unknown_versions_are_preserved() {
        let directory = Directory::new();
        let path = directory.0.join("settings.json");
        std::fs::write(&path, br#"{"version":1,"data":{"enabled":false}}"#).unwrap();
        let mut store = Store::new(directory.0.clone());
        assert_eq!(
            store.read("settings.json", defaults()).unwrap(),
            Example {
                enabled: false,
                delay: 100
            }
        );
        for bytes in [
            b"invalid json".as_slice(),
            br#"{"version":2,"data":{}}"#,
            br#"{"version":1,"data":{"delay":"invalid"}}"#,
        ] {
            std::fs::write(&path, bytes).unwrap();
            let mut store = Store::new(directory.0.clone());
            assert!(store.read("settings.json", defaults()).is_err());
            store.queue("settings.json", &defaults());
            assert!(store.flush(true).is_empty());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn failed_writes_are_reported_and_remain_pending_for_shutdown_retry() {
        let directory = Directory::new();
        let root = directory.0.join("not-a-directory");
        std::fs::write(&root, b"keep").unwrap();
        let mut store = Store::new(root.clone());
        store.queue("settings.json", &defaults());
        assert_eq!(store.flush(true).len(), 1);
        assert_eq!(store.pending.len(), 1);
        assert!(store.flush(false).is_empty());
        std::fs::remove_file(&root).unwrap();
        assert!(store.flush(true).is_empty());
        assert_eq!(store.read("settings.json", defaults()).unwrap(), defaults());
    }
}
