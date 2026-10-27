//! Explicit operator-owned read/write roots for v0.5 file operations.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) const MAX_DXF_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const MAX_LAB_BYTES: u64 = 128 * 1024 * 1024;
pub(super) const MAX_PROJECT_BYTES: u64 = 1024 * 1024 * 1024;

pub(super) struct FilePolicy {
    outputs: Vec<PathBuf>,
    inputs: Vec<PathBuf>,
    relative_root: PathBuf,
}

impl FilePolicy {
    pub(super) fn from_environment() -> Result<Self, String> {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let output = std::env::var_os("AI_REVIT_FILE_ROOTS")
            .map(|s| std::env::split_paths(&s).collect::<Vec<_>>())
            .or_else(|| std::env::var_os("AI_REVIT_FILE_ROOT").map(|s| vec![PathBuf::from(s)]))
            .unwrap_or_else(|| vec![cwd]);
        let mut inputs = output.clone();
        if let Some(value) = std::env::var_os("AI_REVIT_IMPORT_ROOTS") {
            inputs.extend(std::env::split_paths(&value));
        }
        Self::new(output, inputs)
    }

    fn new(outputs: Vec<PathBuf>, inputs: Vec<PathBuf>) -> Result<Self, String> {
        if outputs.is_empty() {
            return Err("at least one permitted file root is required".into());
        }
        let normalize = |roots: Vec<PathBuf>| -> Result<Vec<PathBuf>, String> {
            roots
                .into_iter()
                .map(|root| {
                    let canonical = root
                        .canonicalize()
                        .map_err(|e| format!("file root {}: {e}", root.display()))?;
                    if !canonical.is_dir() {
                        return Err("file roots must be existing directories".into());
                    }
                    Ok(canonical)
                })
                .collect()
        };
        let outputs = normalize(outputs)?;
        let inputs = normalize(inputs)?;
        Ok(Self {
            relative_root: outputs[0].clone(),
            outputs,
            inputs,
        })
    }

    fn absolute(&self, input: &Path) -> Result<PathBuf, String> {
        if input.as_os_str().is_empty() || input.file_name().is_none() {
            return Err("a nonempty file path is required".into());
        }
        if input
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return Err("parent traversal is not allowed in file paths".into());
        }
        #[cfg(windows)]
        for component in input.components() {
            if let Component::Normal(value) = component {
                if value.to_string_lossy().contains(':') {
                    return Err("alternate data streams are not allowed".into());
                }
            }
        }
        if input.is_absolute() {
            Ok(input.to_owned())
        } else if input
            .components()
            .any(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
        {
            Err("drive-relative or root-relative file paths are not allowed".into())
        } else {
            Ok(self.relative_root.join(input))
        }
    }

    pub(super) fn input(&self, input: &Path, limit: u64) -> Result<PathBuf, String> {
        let path = self
            .absolute(input)?
            .canonicalize()
            .map_err(|e| format!("input path: {e}"))?;
        if !self.inputs.iter().any(|root| path.starts_with(root)) {
            return Err("input resolves outside permitted import roots".into());
        }
        let metadata = path.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() {
            return Err("input must be a regular file".into());
        }
        if metadata.len() > limit {
            return Err(format!("input exceeds {limit}-byte budget"));
        }
        Ok(path)
    }

    pub(super) fn read(&self, input: &Path, limit: u64) -> Result<(PathBuf, Vec<u8>), String> {
        let path = self.input(input, limit)?;
        let file = File::open(&path).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > limit {
            return Err("input grew beyond the byte budget".into());
        }
        Ok((path, bytes))
    }

    pub(super) fn output(&self, output: &Path) -> Result<PathBuf, String> {
        let path = self.absolute(output)?;
        if path.exists() {
            let actual = path.canonicalize().map_err(|e| e.to_string())?;
            if !actual.is_file() || !self.outputs.iter().any(|root| actual.starts_with(root)) {
                return Err(
                    "output resolves outside permitted roots or is not a regular file".into(),
                );
            }
            return Ok(actual);
        }
        let parent = path.parent().ok_or("output has no parent directory")?;
        let mut nearest = parent;
        while !nearest.exists() {
            nearest = nearest.parent().ok_or("output has no existing ancestor")?;
        }
        let actual = nearest.canonicalize().map_err(|e| e.to_string())?;
        if !self.outputs.iter().any(|root| actual.starts_with(root)) {
            return Err("output parent resolves outside permitted roots".into());
        }
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let parent = parent.canonicalize().map_err(|e| e.to_string())?;
        if !self.outputs.iter().any(|root| parent.starts_with(root)) {
            return Err("output parent crossed a reparse boundary".into());
        }
        Ok(parent.join(path.file_name().ok_or("output has no filename")?))
    }

    pub(super) fn write_atomic(&self, output: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let output = self.output(output)?;
        let parent = output.parent().ok_or("output has no parent")?;
        let temp = parent.join(format!(
            ".ai-revit-v05-{}-{}.tmp",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let mut created = false;
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|e| e.to_string())?;
            created = true;
            let actual = temp.canonicalize().map_err(|e| e.to_string())?;
            if !self.outputs.iter().any(|root| actual.starts_with(root)) {
                return Err("temporary output crossed a reparse boundary".into());
            }
            file.write_all(bytes).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            drop(file);
            replace_file(&temp, &output)?;
            Ok(output.clone())
        })();
        if result.is_err() && created {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}

#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    fs::rename(from, to).map_err(|e| e.to_string())
}
#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let a: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let b: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), 0x1 | 0x8) } == 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_roots_bound_reads_atomic_writes_and_import_only_roots() {
        let root = std::env::temp_dir().join(format!(
            "ai-revit-file-policy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let out = root.join("out");
        let import = root.join("import");
        fs::create_dir_all(&out).unwrap();
        fs::create_dir_all(&import).unwrap();
        fs::write(import.join("source.json"), b"source").unwrap();
        let policy = FilePolicy::new(vec![out.clone()], vec![out.clone(), import.clone()]).unwrap();
        assert!(policy.output(&import.join("overwrite.json")).is_err());
        assert!(policy.output(Path::new("../escape.json")).is_err());
        assert_eq!(
            policy.read(&import.join("source.json"), 6).unwrap().1,
            b"source"
        );
        assert!(policy.read(&import.join("source.json"), 5).is_err());
        policy
            .write_atomic(Path::new("nested/file.json"), b"first")
            .unwrap();
        policy
            .write_atomic(Path::new("nested/file.json"), b"second")
            .unwrap();
        assert_eq!(fs::read(out.join("nested/file.json")).unwrap(), b"second");
        assert!(!out.join("escape.json").exists());
        // Only this uniquely created fixture directory is removed.
        let resolved = root.canonicalize().unwrap();
        assert!(resolved.starts_with(std::env::temp_dir().canonicalize().unwrap()));
        assert!(
            resolved
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ai-revit-file-policy-")
        );
        fs::remove_dir_all(resolved).unwrap();
    }
}
