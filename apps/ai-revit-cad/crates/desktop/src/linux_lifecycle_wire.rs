//! Private Linux smoke contract. No public server or arbitrary path/command.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs::{self, File, OpenOptions}, io::{Read, Write}, path::{Path, PathBuf},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    sync::{OnceLock, atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering}},
    time::{Duration, Instant}};

pub const ROOT: &str = "/run/aiui-gui";
pub const ENDPOINT: &str = "127.0.0.1:48728";
pub const PROFILE: &str = "ai-revit-source03-linux-gui-v1";
pub const REQUEST_CAP: usize = 4096;
pub const REPLY_CAP: usize = 1 << 20;
const ELF_CAP: u64 = 512 << 20;
const O_NOFOLLOW: i32 = 0x20000; // Linux x86_64, the fixed target only.
const O_CLOEXEC: i32 = 0x80000; // Linux x86_64; retained image FD must not survive exec.

pub type Result<T> = std::result::Result<T, String>;
pub fn ensure(ok: bool, code: &str) -> Result<()> { if ok { Ok(()) } else { Err(code.into()) } }
pub fn remaining(deadline: Instant) -> Result<Duration> {
    deadline.checked_duration_since(Instant::now()).filter(|v| !v.is_zero())
        .ok_or_else(|| "LIFECYCLE_DEADLINE_EXPIRED".into())
}
pub fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
fn hex64(value: &str, lower: bool) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit() && (!lower || !b.is_ascii_uppercase()))
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WindowKey { pub index: usize, pub generation: u64 }
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub profile_id: String, pub run_id: String, pub session_id: String, pub owner: String,
    pub pid: u32, pub start_ticks: u64, pub pid_namespace: String, pub elf_sha256: String,
    pub window: WindowKey,
}
impl Binding {
    pub fn validate(&self) -> Result<()> {
        let ns = self.pid_namespace.strip_prefix("pid:[").and_then(|v| v.strip_suffix(']'));
        ensure(self.profile_id == PROFILE && [&self.run_id, &self.session_id, &self.owner].into_iter()
            .all(|v| identifier(v)) && self.pid > 0 && self.start_ticks > 0 && self.window.index == 0
            && self.pid_namespace.len() <= 64 && ns.is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
            && hex64(&self.elf_sha256, true), "LIFECYCLE_BINDING_INVALID")
    }
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bootstrap {
    pub profile_id: String, pub run_id: String, pub session_id: String, pub owner: String,
    pub elf_sha256: String, pub budget_ms: u64,
}
impl Bootstrap {
    pub fn bind(&self, observed: &ProcessIdentity, window: WindowKey) -> Result<Binding> {
        let value = Binding { profile_id: self.profile_id.clone(), run_id: self.run_id.clone(),
            session_id: self.session_id.clone(), owner: self.owner.clone(), pid: observed.pid,
            start_ticks: observed.start_ticks, pid_namespace: observed.pid_namespace.clone(),
            elf_sha256: observed.elf_sha256.clone(), window };
        value.validate()?;
        ensure(self.elf_sha256 == observed.elf_sha256 && self.budget_ms > 0 && self.budget_ms <= 90_000,
            "LIFECYCLE_BOOTSTRAP_MISMATCH")?;
        Ok(value)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusParams { pub expected: Binding }
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase { Prepare, Commit }
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseParams { pub expected: Binding, pub phase: Phase, pub command_id: String }
#[derive(Serialize, Deserialize)]
#[serde(tag = "method", deny_unknown_fields)]
pub enum LifecycleRequest {
    #[serde(rename = "diagnostic.lifecycle.status")]
    Status { version: u8, id: String, token: String, params: StatusParams },
    #[serde(rename = "diagnostic.lifecycle.close")]
    Close { version: u8, id: String, token: String, params: CloseParams },
}
impl LifecycleRequest {
    pub fn validate(&self) -> Result<()> {
        let (version, id, token, binding) = match self {
            Self::Status { version, id, token, params } => (version, id, token, &params.expected),
            Self::Close { version, id, token, params } => {
                ensure(identifier(&params.command_id), "LIFECYCLE_COMMAND_ID_INVALID")?;
                (version, id, token, &params.expected)
            }
        };
        ensure(*version == 1 && identifier(id) && hex64(token, false), "LIFECYCLE_REQUEST_INVALID")?;
        binding.validate()
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Initial {
    pub schema: String, pub binding: Binding, pub state: String, pub api_ready: bool,
    pub ui_tick: u64, pub worker_quiescent: bool, pub native_xid: Option<u64>,
    pub native_xid_verified: bool, pub binding_source: String,
}
#[derive(Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32, pub start_ticks: u64, pub pid_namespace: String, pub elf_sha256: String,
}
pub struct Trusted {
    pub bootstrap: Bootstrap, token: String, pub started: Instant, pub deadline: Instant,
    pub process: ProcessLease,
}
static TRUSTED: OnceLock<Trusted> = OnceLock::new();
static FATAL: OnceLock<String> = OnceLock::new();
static CLOSED_EVENT: AtomicBool = AtomicBool::new(false);

fn uid() -> Result<u32> { fs::metadata("/proc/self").map(|v| v.uid()).map_err(|_| "LIFECYCLE_PROC_UID".into()) }
fn directory() -> Result<u32> {
    let current = uid()?;
    for name in ["/", "/run", ROOT] {
        let meta = fs::symlink_metadata(name).map_err(|_| "LIFECYCLE_TRUSTED_DIRECTORY_MISSING")?;
        ensure(meta.is_dir() && !meta.file_type().is_symlink() && meta.mode() & 0o022 == 0
            && (meta.uid() == 0 || meta.uid() == current || current == 0), "LIFECYCLE_TRUSTED_DIRECTORY_INVALID")?;
    }
    fs::metadata(ROOT).map(|v| v.uid()).map_err(|_| "LIFECYCLE_TRUSTED_DIRECTORY_INVALID".into())
}
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    (a.dev(), a.ino(), a.len(), a.mtime(), a.mtime_nsec()) ==
    (b.dev(), b.ino(), b.len(), b.mtime(), b.mtime_nsec())
}
pub fn private_read(name: &str, cap: usize) -> Result<Vec<u8>> {
    ensure(matches!(name, "bootstrap.json" | "api-token" | "initial-lifecycle.json"), "LIFECYCLE_PRIVATE_PATH_REFUSED")?;
    let owner = directory()?;
    let path = Path::new(ROOT).join(name);
    let before = fs::symlink_metadata(&path).map_err(|_| "LIFECYCLE_PRIVATE_FILE_MISSING")?;
    ensure(before.is_file() && !before.file_type().is_symlink() && before.mode() & 0o7777 == 0o600
        && before.nlink() == 1 && (before.uid() == owner || before.uid() == 0) && before.len() <= cap as u64,
        "LIFECYCLE_PRIVATE_FILE_INVALID")?;
    let mut file = OpenOptions::new().read(true).custom_flags(O_NOFOLLOW).open(&path)
        .map_err(|_| "LIFECYCLE_PRIVATE_FILE_OPEN")?;
    ensure(same_file(&before, &file.metadata().map_err(|_| "LIFECYCLE_PRIVATE_FILE_STAT")?), "LIFECYCLE_PRIVATE_FILE_CHANGED")?;
    let mut data = Vec::new();
    (&mut file).take(cap as u64 + 1).read_to_end(&mut data).map_err(|_| "LIFECYCLE_PRIVATE_FILE_READ")?;
    ensure(data.len() <= cap && same_file(&before, &file.metadata().map_err(|_| "LIFECYCLE_PRIVATE_FILE_STAT")?)
        && same_file(&before, &fs::symlink_metadata(path).map_err(|_| "LIFECYCLE_PRIVATE_FILE_STAT")?),
        "LIFECYCLE_PRIVATE_FILE_CHANGED")?;
    Ok(data)
}
pub fn read_bootstrap() -> Result<Bootstrap> {
    serde_json::from_slice(&private_read("bootstrap.json", REQUEST_CAP)?)
        .map_err(|_| "LIFECYCLE_BOOTSTRAP_INVALID".into())
}
pub fn read_token() -> Result<String> {
    let data = private_read("api-token", 65)?;
    let text = std::str::from_utf8(&data).map_err(|_| "LIFECYCLE_TOKEN_INVALID")?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    ensure(hex64(text, false), "LIFECYCLE_TOKEN_INVALID")?;
    Ok(text.into())
}
pub fn atomic_json(name: &str, value: &impl Serialize) -> Result<()> {
    ensure(matches!(name, "initial-lifecycle.json" | "lifecycle-error.json" | "lifecycle-close-dispatched.json"
        | "lifecycle-window-closed.json" | "client-started.json" | "client-commit-send-started.json" | "client-result.json"
        | "client-attestation-started.json"),
        "LIFECYCLE_OUTPUT_PATH_REFUSED")?;
    directory()?;
    let path = Path::new(ROOT).join(name);
    let tmp = Path::new(ROOT).join(format!(".{name}.tmp"));
    ensure(!path.try_exists().map_err(|_| "LIFECYCLE_OUTPUT_STAT")?
        && fs::symlink_metadata(&path).is_err(), "LIFECYCLE_OUTPUT_ALREADY_EXISTS")?;
    let data = serde_json::to_vec(value).map_err(|_| "LIFECYCLE_OUTPUT_ENCODE")?;
    ensure(data.len() < REQUEST_CAP, "LIFECYCLE_OUTPUT_BUDGET")?;
    let mut out = OpenOptions::new().create_new(true).write(true).mode(0o600).custom_flags(O_NOFOLLOW)
        .open(&tmp).map_err(|_| "LIFECYCLE_OUTPUT_CREATE")?;
    let created = out.metadata().map_err(|_| "LIFECYCLE_OUTPUT_STAT")?;
    ensure(created.is_file() && created.nlink() == 1 && created.mode() & 0o7777 == 0o600
        && created.uid() == uid()?, "LIFECYCLE_OUTPUT_PERMISSIONS")?;
    out.write_all(&data).and_then(|_| out.write_all(b"\n")).and_then(|_| out.sync_all())
        .map_err(|_| "LIFECYCLE_OUTPUT_WRITE")?;
    drop(out);
    fs::rename(&tmp, &path).map_err(|_| "LIFECYCLE_OUTPUT_RENAME")?;
    File::open(ROOT).and_then(|v| v.sync_all()).map_err(|_| "LIFECYCLE_OUTPUT_DURABILITY")?;
    Ok(())
}
fn small_proc(path: &Path, cap: u64) -> Result<String> {
    let mut file = File::open(path).map_err(|_| "LIFECYCLE_PROC_OPEN")?;
    let mut data = String::new();
    (&mut file).take(cap + 1).read_to_string(&mut data).map_err(|_| "LIFECYCLE_PROC_READ")?;
    ensure(data.len() as u64 <= cap, "LIFECYCLE_PROC_BUDGET")?;
    Ok(data)
}
pub fn start_ticks(pid: u32) -> Result<u64> {
    let value = small_proc(&PathBuf::from(format!("/proc/{pid}/stat")), 16384)?;
    let end = value.rfind(')').ok_or("LIFECYCLE_PROC_STAT_INVALID")?;
    value[end + 1..].split_whitespace().nth(19).ok_or("LIFECYCLE_PROC_STAT_INVALID")?
        .parse::<u64>().map_err(|_| "LIFECYCLE_PROC_STAT_INVALID".into())
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileState {
    regular: bool, dev: u64, ino: u64, size: u64, mtime: i64, mtime_ns: i64,
    ctime: i64, ctime_ns: i64, mode: u32, uid: u32, gid: u32, nlink: u64,
}
impl FileState {
    fn from_metadata(m: &fs::Metadata) -> Self {
        Self { regular: m.is_file(), dev: m.dev(), ino: m.ino(), size: m.len(),
            mtime: m.mtime(), mtime_ns: m.mtime_nsec(), ctime: m.ctime(), ctime_ns: m.ctime_nsec(),
            mode: m.mode(), uid: m.uid(), gid: m.gid(), nlink: m.nlink() }
    }
    fn validate(&self) -> Result<()> {
        ensure(self.regular && self.size >= 4 && self.size <= ELF_CAP && self.nlink > 0,
            "LIFECYCLE_EXECUTABLE_STATE_INVALID")
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct ProcessState {
    pid: u32, birth: u64, pid_ns: String, net_ns: String, image: FileState,
}
// Private seam for bounded in-memory tests of the SAME attestation/check logic.
// It is not a public runner or a source of trusted observations in production.
trait ImageObservation {
    type Image: Read;
    fn observe(&self, pid: u32, deadline: Instant) -> Result<(ProcessState, Self::Image)>;
    fn state(&self, image: &Self::Image) -> Result<FileState>;
}
struct Procfs;
fn namespace(pid: u32, kind: &str) -> Result<String> {
    let value = fs::read_link(format!("/proc/{pid}/ns/{kind}"))
        .map_err(|_| "LIFECYCLE_PROC_NAMESPACE")?;
    let value = value.to_str().ok_or("LIFECYCLE_PROC_NAMESPACE")?;
    let prefix = match kind { "pid" => "pid:[", "net" => "net:[", _ => return Err("LIFECYCLE_NAMESPACE_KIND".into()) };
    let number = value.strip_prefix(prefix).and_then(|v| v.strip_suffix(']'));
    ensure(value.len() <= 64 && number.is_some_and(|v| !v.is_empty()
        && v.bytes().all(|b| b.is_ascii_digit())), "LIFECYCLE_PROC_NAMESPACE_INVALID")?;
    Ok(value.to_owned())
}
impl ImageObservation for Procfs {
    type Image = File;
    fn observe(&self, pid: u32, deadline: Instant) -> Result<(ProcessState, File)> {
        remaining(deadline)?;
        ensure(pid > 0, "LIFECYCLE_PROC_PID_INVALID")?;
        let birth = start_ticks(pid)?;
        let pid_ns = namespace(pid, "pid")?;
        let net_ns = namespace(pid, "net")?;
        // Intentional kernel procfs reference, not a trusted-input pathname.
        let file = OpenOptions::new().read(true).custom_flags(O_CLOEXEC)
            .open(format!("/proc/{pid}/exe")).map_err(|_| "LIFECYCLE_PROC_EXE")?;
        let image = self.state(&file)?;
        image.validate()?;
        ensure(birth > 0 && start_ticks(pid)? == birth && namespace(pid, "pid")? == pid_ns
            && namespace(pid, "net")? == net_ns && self.state(&file)? == image,
            "LIFECYCLE_PROC_CHANGED")?;
        remaining(deadline)?;
        Ok((ProcessState { pid, birth, pid_ns, net_ns, image }, file))
    }
    fn state(&self, image: &File) -> Result<FileState> {
        image.metadata().map(|v| FileState::from_metadata(&v)).map_err(|_| "LIFECYCLE_PROC_EXE_STAT".into())
    }
}
#[derive(Serialize)]
pub struct IdentityDiagnostics {
    schema: &'static str, full_hash_count: u64, full_hash_bytes: u64, full_hash_elapsed_us: u64,
    cheap_check_count: u64, last_phase: &'static str,
}
struct Attestation<H> {
    held: H, baseline: ProcessState, identity: ProcessIdentity,
    hash_bytes: u64, hash_us: u64, checks: AtomicU64, phase: AtomicU8, refused: AtomicBool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttestationProgressStage { NotStarted, InitialProcObserve, FullElfHash, PostHashIdentity, Complete }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttestationHashStep { NotStarted, Read, Digest, Finalize, Complete }
/// Optional per-call diagnostics only. Counters and labels never attest identity.
/// Existing attest wrappers pass None and do not sample diagnostic timestamps.
pub struct AttestationProgress {
    stage: AttestationProgressStage, hash_step: AttestationHashStep,
    stage_started: Option<Instant>, hash_bytes_read: u64, hash_complete: bool,
}
impl AttestationProgress {
    pub fn new() -> Self {
        Self { stage: AttestationProgressStage::NotStarted, hash_step: AttestationHashStep::NotStarted,
            stage_started: None, hash_bytes_read: 0, hash_complete: false }
    }
    fn mark(&mut self, stage: AttestationProgressStage, now: Instant) {
        self.stage = stage; self.stage_started = Some(now);
    }
    fn begin(&mut self) {
        *self = Self::new();
        self.mark(AttestationProgressStage::InitialProcObserve, Instant::now());
    }
    pub fn stage_name(&self) -> &'static str {
        match self.stage {
            AttestationProgressStage::NotStarted => "NOT_STARTED",
            AttestationProgressStage::InitialProcObserve => "INITIAL_PROC_OBSERVE",
            AttestationProgressStage::FullElfHash => "FULL_ELF_HASH",
            AttestationProgressStage::PostHashIdentity => "POST_HASH_IDENTITY",
            AttestationProgressStage::Complete => "COMPLETE",
        }
    }
    pub fn hash_step_name(&self) -> &'static str {
        match self.hash_step {
            AttestationHashStep::NotStarted => "NOT_STARTED",
            AttestationHashStep::Read => "READ",
            AttestationHashStep::Digest => "DIGEST",
            AttestationHashStep::Finalize => "FINALIZE",
            AttestationHashStep::Complete => "COMPLETE",
        }
    }
    pub fn stage_elapsed_ms(&self, now: Instant) -> u64 {
        self.stage_started.and_then(|start| now.checked_duration_since(start))
            .unwrap_or_default().as_millis().min(u64::MAX as u128) as u64
    }
    /// Sum of Ok(n) bytes from the retained-image Read, including bytes rejected
    /// by later validation. An Err read contributes zero; partial bytes are unknown.
    pub fn hash_bytes_read(&self) -> u64 { self.hash_bytes_read }
    /// Digest finalization happened; length and post-hash identity may still refuse.
    pub fn hash_complete(&self) -> bool { self.hash_complete }
}
fn attest_with<O: ImageObservation>(source: &O, pid: u32, deadline: Instant) -> Result<Attestation<O::Image>> {
    attest_with_progress(source, pid, deadline, None)
}
fn attest_with_progress<O: ImageObservation>(source: &O, pid: u32, deadline: Instant,
    mut progress: Option<&mut AttestationProgress>) -> Result<Attestation<O::Image>> {
    if let Some(value) = progress.as_deref_mut() { value.begin(); }
    let (baseline, mut held) = source.observe(pid, deadline)?;
    baseline.image.validate()?;
    let started = Instant::now();
    if let Some(value) = progress.as_deref_mut() {
        value.mark(AttestationProgressStage::FullElfHash, started);
    }
    let mut digest = Sha256::new();
    let mut count = 0u64;
    let mut block = [0u8; 65536];
    loop {
        if let Some(value) = progress.as_deref_mut() { value.hash_step = AttestationHashStep::Read; }
        remaining(deadline)?;
        let n = held.read(&mut block).map_err(|_| "LIFECYCLE_PROC_EXE_READ")?;
        if let Some(value) = progress.as_deref_mut() {
            value.hash_bytes_read = value.hash_bytes_read.saturating_add(n as u64);
        }
        if n == 0 { break; }
        if count == 0 { ensure(n >= 4 && &block[..4] == b"\x7fELF", "LIFECYCLE_NOT_ELF")?; }
        count += n as u64;
        ensure(count <= ELF_CAP, "LIFECYCLE_ELF_BUDGET")?;
        if let Some(value) = progress.as_deref_mut() { value.hash_step = AttestationHashStep::Digest; }
        digest.update(&block[..n]);
    }
    let hash_us = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
    if let Some(value) = progress.as_deref_mut() { value.hash_step = AttestationHashStep::Finalize; }
    let identity = ProcessIdentity { pid: baseline.pid, start_ticks: baseline.birth,
        pid_namespace: baseline.pid_ns.clone(), elf_sha256: format!("{:x}", digest.finalize()) };
    if let Some(value) = progress.as_deref_mut() {
        value.hash_complete = true; value.hash_step = AttestationHashStep::Complete;
    }
    ensure(count == baseline.image.size, "LIFECYCLE_EXECUTABLE_LENGTH_CHANGED")?;
    let value = Attestation { held, baseline, identity, hash_bytes: count, hash_us,
        checks: AtomicU64::new(0), phase: AtomicU8::new(0), refused: AtomicBool::new(false) };
    if let Some(value) = progress.as_deref_mut() {
        value.mark(AttestationProgressStage::PostHashIdentity, Instant::now());
    }
    value.check(source, deadline)?;
    if let Some(value) = progress.as_deref_mut() {
        value.mark(AttestationProgressStage::Complete, Instant::now());
    }
    Ok(value)
}
impl<H> Attestation<H> {
    fn check<O: ImageObservation<Image = H>>(&self, source: &O, deadline: Instant) -> Result<ProcessIdentity> {
        ensure(!self.refused.load(Ordering::Acquire), "LIFECYCLE_ATTESTATION_REFUSED")?;
        self.checks.fetch_add(1, Ordering::Relaxed);
        self.phase.store(1, Ordering::Relaxed);
        let result = (|| {
            remaining(deadline)?;
            let retained_before = source.state(&self.held)?;
            let (current, fresh_file) = source.observe(self.baseline.pid, deadline)?;
            ensure(current == self.baseline && retained_before == self.baseline.image
                && source.state(&self.held)? == self.baseline.image
                && source.state(&fresh_file)? == self.baseline.image, "LIFECYCLE_ATTESTED_IMAGE_CHANGED")?;
            remaining(deadline)?;
            ensure(!self.refused.load(Ordering::Acquire), "LIFECYCLE_ATTESTATION_REFUSED")?;
            Ok(self.identity.clone())
        })();
        if result.is_err() { self.refused.store(true, Ordering::Release); }
        self.phase.store(if result.is_ok() { 2 } else { 3 }, Ordering::Relaxed);
        result
    }
    fn verify_expected<O: ImageObservation<Image = H>>(&self, source: &O,
        expected: &Binding, deadline: Instant) -> Result<()> {
        let result = self.check(source, deadline).and_then(|v| identity_matches(&v, expected));
        if result.is_err() {
            self.refused.store(true, Ordering::Release);
            self.phase.store(3, Ordering::Relaxed);
        }
        result
    }
    fn diagnostics(&self) -> IdentityDiagnostics {
        IdentityDiagnostics { schema: "source03-identity-diagnostics-v1", full_hash_count: 1,
            full_hash_bytes: self.hash_bytes, full_hash_elapsed_us: self.hash_us,
            cheap_check_count: self.checks.load(Ordering::Relaxed),
            last_phase: match self.phase.load(Ordering::Relaxed) {
                0 => "HASH_COMPLETE", 1 => "CHECK_STARTED", 2 => "CHECK_COMPLETE", _ => "CHECK_REFUSED" } }
    }
}
/// Per-run retained executing-image attestation. Read-only FD alone is NOT byte
/// immutability. Requires the reviewed normal Linux regular-image trust model;
/// does not attest same-inode exec generation, live memory or privileged writers.
pub struct ProcessLease { value: Attestation<File> }
impl ProcessLease {
    pub fn attest(pid: u32, deadline: Instant) -> Result<Self> {
        Ok(Self { value: attest_with(&Procfs, pid, deadline)? })
    }
    /// Same attestation and guard order, with optional diagnostic progress retained on Err.
    pub fn attest_with_progress(pid: u32, deadline: Instant, progress: &mut AttestationProgress) -> Result<Self> {
        Ok(Self { value: attest_with_progress(&Procfs, pid, deadline, Some(progress))? })
    }
    pub fn checked_identity(&self, deadline: Instant) -> Result<ProcessIdentity> { self.value.check(&Procfs, deadline) }
    pub fn diagnostics(&self) -> IdentityDiagnostics { self.value.diagnostics() }
    fn refuse(&self) {
        self.value.refused.store(true, Ordering::Release);
        self.value.phase.store(3, Ordering::Relaxed);
    }
}
fn identity_matches(observed: &ProcessIdentity, expected: &Binding) -> Result<()> {
    expected.validate()?;
    ensure((observed.pid, observed.start_ticks, observed.pid_namespace.as_str(), observed.elf_sha256.as_str()) ==
        (expected.pid, expected.start_ticks, expected.pid_namespace.as_str(), expected.elf_sha256.as_str()),
        "LIFECYCLE_PROCESS_IDENTITY_MISMATCH")
}
pub fn verify_process(expected: &Binding, lease: &ProcessLease, deadline: Instant) -> Result<()> {
    lease.value.verify_expected(&Procfs, expected, deadline)
}
pub fn verify_listener(expected: &Binding, lease: &ProcessLease, deadline: Instant) -> Result<()> {
    let result = (|| {
        verify_process(expected, lease, deadline)?;
        ensure(fs::read_link("/proc/self/ns/net").map_err(|_| "LIFECYCLE_NET_NAMESPACE")? ==
            fs::read_link(format!("/proc/{}/ns/net", expected.pid)).map_err(|_| "LIFECYCLE_NET_NAMESPACE")?,
            "LIFECYCLE_NET_NAMESPACE_MISMATCH")?;
        let text = small_proc(&PathBuf::from(format!("/proc/{}/net/tcp", expected.pid)), 1 << 20)?;
        let rows: Vec<_> = text.lines().skip(1).filter_map(|line| {
            let parts: Vec<_> = line.split_whitespace().collect();
            (parts.len() > 9 && parts[1] == "0100007F:BE58" && parts[3] == "0A").then(|| parts[9].to_owned())
        }).collect();
        ensure(rows.len() == 1 && rows[0] != "0", "LIFECYCLE_LISTENER_AMBIGUOUS")?;
        let wanted = format!("socket:[{}]", rows[0]);
        let mut owned = false;
        let mut count = 0;
        for entry in fs::read_dir(format!("/proc/{}/fd", expected.pid)).map_err(|_| "LIFECYCLE_FD_DIRECTORY")? {
            remaining(deadline)?;
            count += 1;
            ensure(count <= 4096, "LIFECYCLE_FD_BUDGET")?;
            let entry = entry.map_err(|_| "LIFECYCLE_FD_ENTRY")?;
            if fs::read_link(entry.path()).ok().and_then(|v| v.to_str().map(str::to_owned)).as_deref() == Some(wanted.as_str()) { owned = true; }
        }
        ensure(owned && start_ticks(expected.pid)? == expected.start_ticks, "LIFECYCLE_LISTENER_NOT_OWNED")?;
        // Keep the socket-FD proof and bracket its whole scan with fresh exe identity.
        verify_process(expected, lease, deadline)?;
        Ok(())
    })();
    if result.is_err() { lease.refuse(); }
    result
}
pub fn initialize_trusted() -> Result<()> {
    let started = Instant::now();
    let bootstrap = read_bootstrap()?;
    ensure(bootstrap.budget_ms > 0 && bootstrap.budget_ms <= 90_000, "LIFECYCLE_BOOTSTRAP_BUDGET")?;
    let deadline = started + Duration::from_millis(bootstrap.budget_ms);
    let token = read_token()?;
    let process = ProcessLease::attest(std::process::id(), deadline)?;
    let observed = process.checked_identity(deadline)?;
    bootstrap.bind(&observed, WindowKey { index: 0, generation: 0 })?;
    if let Ok(port) = std::env::var("AI_REVIT_API_PORT") { ensure(port == "48728", "LIFECYCLE_PORT_OVERRIDE_REFUSED")?; }
    for name in ["initial-lifecycle.json", "lifecycle-error.json", "lifecycle-close-dispatched.json", "lifecycle-window-closed.json",
                 "client-started.json", "client-commit-send-started.json", "client-result.json", "client-attestation-started.json"] {
        ensure(fs::symlink_metadata(Path::new(ROOT).join(name)).is_err(), "LIFECYCLE_STALE_OUTPUT")?;
    }
    TRUSTED.set(Trusted { bootstrap, token, started, deadline, process }).map_err(|_| "LIFECYCLE_ALREADY_INITIALIZED".to_owned())
}
pub fn trusted() -> Result<&'static Trusted> { TRUSTED.get().ok_or_else(|| "LIFECYCLE_NOT_INITIALIZED".into()) }
pub fn expected_token() -> Option<&'static str> { TRUSTED.get().map(|v| v.token.as_str()) }
pub fn fail(code: &str) {
    if FATAL.set(code.chars().take(256).collect()).is_ok() {
        let _ = atomic_json("lifecycle-error.json", &serde_json::json!({"schema":"source03-lifecycle-error-v1","error":code,"ready":false,
            "identity_diagnostics": TRUSTED.get().map(|v| v.process.diagnostics())}));
        eprintln!("SOURCE03_LIFECYCLE_ERROR {}", code);
    }
}
pub fn healthy() -> bool { FATAL.get().is_none() }
pub fn closed_event() { CLOSED_EVENT.store(true, Ordering::Release); }
pub fn finish_process() -> Result<()> {
    if let Some(value) = TRUSTED.get() {
        let summary = serde_json::to_string(&value.process.diagnostics()).map_err(|_| "LIFECYCLE_DIAGNOSTICS_ENCODE")?;
        // One fixed-schema, nonsecret summary; not per-chunk UI logging.
        eprintln!("SOURCE03_IDENTITY_DIAGNOSTICS {}", summary);
    }
    if let Some(code) = FATAL.get() { return Err(code.clone()); }
    ensure(CLOSED_EVENT.load(Ordering::Acquire), "LIFECYCLE_NORMAL_CLOSE_NOT_OBSERVED")
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    use std::{cell::{Cell, RefCell}, io::{self, Cursor}, rc::Rc};

    // These tests never observe /proc or the kernel. File semantics, CLOEXEC,
    // socket ownership, actual deadlines and same-inode exec are NOT proved.
    const IMAGE: &[u8] = &[127, 69, 76, 70, 1, 2, 3, 4];
    // SHA256 of those exact eight bytes, independently computed with native
    // .NET SHA256 during source preparation, not learned from a target run.
    const IMAGE_SHA: &str = "1ce1805693fc7e6df020e40f78385981789a078c681dbbcbd1551be1c3939a3e";
    struct FakeImage {
        data: Cursor<Vec<u8>>, metadata: Rc<RefCell<FileState>>,
        read_calls: Rc<Cell<u64>>, read_error: Rc<Cell<bool>>,
    }
    impl Read for FakeImage {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            self.read_calls.set(self.read_calls.get() + 1);
            if self.read_error.get() { return Err(io::Error::other("fixed fake read refusal")); }
            self.data.read(out)
        }
    }
    struct Fake {
        current: RefCell<ProcessState>, bytes: RefCell<Vec<u8>>, handles: RefCell<Vec<Rc<RefCell<FileState>>>>,
        read_calls: Rc<Cell<u64>>, read_error: Rc<Cell<bool>>, observe_error: Cell<bool>,
        observations: Cell<u64>, change_on_second_observation: Cell<bool>,
    }
    impl Fake {
        fn new() -> Self {
            Self { current: RefCell::new(ProcessState { pid: 41, birth: 900,
                pid_ns: "pid:[12]".into(), net_ns: "net:[13]".into(), image: FileState {
                    regular: true, dev: 8, ino: 700, size: IMAGE.len() as u64,
                    mtime: 20, mtime_ns: 1, ctime: 30, ctime_ns: 2, mode: 0o100500,
                    uid: 0, gid: 0, nlink: 1 } }), bytes: RefCell::new(IMAGE.to_vec()),
                handles: RefCell::new(Vec::new()), read_calls: Rc::new(Cell::new(0)),
                read_error: Rc::new(Cell::new(false)), observe_error: Cell::new(false),
                observations: Cell::new(0), change_on_second_observation: Cell::new(false) }
        }
    }
    impl ImageObservation for Fake {
        type Image = FakeImage;
        fn observe(&self, _pid: u32, deadline: Instant) -> Result<(ProcessState, FakeImage)> {
            remaining(deadline)?;
            ensure(!self.observe_error.get(), "FAKE_ACCESS_REFUSED")?;
            let count = self.observations.get() + 1;
            self.observations.set(count);
            if count == 2 && self.change_on_second_observation.get() { self.current.borrow_mut().birth += 1; }
            let state = self.current.borrow().clone();
            let metadata = Rc::new(RefCell::new(state.image.clone()));
            self.handles.borrow_mut().push(metadata.clone());
            Ok((state, FakeImage { data: Cursor::new(self.bytes.borrow().clone()), metadata,
                read_calls: self.read_calls.clone(), read_error: self.read_error.clone() }))
        }
        fn state(&self, image: &FakeImage) -> Result<FileState> {
            ensure(!self.observe_error.get(), "FAKE_ACCESS_REFUSED")?;
            Ok(image.metadata.borrow().clone())
        }
    }
    fn deadline() -> Instant { Instant::now() + Duration::from_secs(1) }
    fn binding(sha: &str) -> Binding {
        Binding { profile_id: PROFILE.into(), run_id: "fake-run".into(), session_id: "fake-session".into(),
            owner: "fake-owner".into(), pid: 41, start_ticks: 900, pid_namespace: "pid:[12]".into(),
            elf_sha256: sha.into(), window: WindowKey { index: 0, generation: 0 } }
    }

    #[test]
    fn one_complete_hash_then_many_fresh_checks_without_reads() {
        let fake = Fake::new();
        let lease = attest_with(&fake, 41, deadline()).unwrap();
        assert_eq!(lease.identity.elf_sha256, IMAGE_SHA);
        let reads = fake.read_calls.get();
        for _ in 0..128 { identity_matches(&lease.check(&fake, deadline()).unwrap(), &binding(IMAGE_SHA)).unwrap(); }
        assert_eq!(fake.read_calls.get(), reads);
        assert_eq!(fake.observations.get(), 130); // initial + post-hash + 128 current observations.
        let d = lease.diagnostics();
        assert_eq!((d.full_hash_count, d.full_hash_bytes, d.cheap_check_count), (1, 8, 129));
    }

    #[test]
    fn process_pid_birth_and_each_namespace_drift_refuse() {
        for change in 0..4 {
            let fake = Fake::new();
            let lease = attest_with(&fake, 41, deadline()).unwrap();
            let mut state = fake.current.borrow_mut();
            match change { 0 => state.pid += 1, 1 => state.birth += 1,
                2 => state.pid_ns = "pid:[99]".into(), _ => state.net_ns = "net:[99]".into() }
            drop(state);
            assert!(lease.check(&fake, deadline()).is_err());
        }
    }

    #[test]
    fn current_inode_and_every_tracked_file_field_drift_refuse() {
        for change in 0..12 {
            let fake = Fake::new();
            let lease = attest_with(&fake, 41, deadline()).unwrap();
            let mut state = fake.current.borrow_mut();
            let m = &mut state.image;
            match change { 0 => m.dev += 1, 1 => m.ino += 1, 2 => m.size += 1,
                3 => m.mtime += 1, 4 => m.mtime_ns += 1, 5 => m.ctime += 1,
                6 => m.ctime_ns += 1, 7 => m.mode ^= 0o200, 8 => m.uid += 1,
                9 => m.gid += 1, 10 => m.nlink = 0, _ => m.regular = false }
            drop(state);
            assert!(lease.check(&fake, deadline()).is_err());
        }
    }

    #[test]
    fn retained_descriptor_metadata_drift_refuses_even_if_current_looks_unchanged() {
        let fake = Fake::new();
        let lease = attest_with(&fake, 41, deadline()).unwrap();
        fake.handles.borrow()[0].borrow_mut().ctime_ns += 1;
        assert!(lease.check(&fake, deadline()).is_err());
    }

    #[test]
    fn failed_observation_is_sticky_after_original_state_restored() {
        let fake = Fake::new();
        let lease = attest_with(&fake, 41, deadline()).unwrap();
        let original = fake.current.borrow().clone();
        fake.current.borrow_mut().image.ino += 1;
        assert!(lease.check(&fake, deadline()).is_err());
        *fake.current.borrow_mut() = original;
        assert_eq!(lease.check(&fake, deadline()).err().as_deref(), Some("LIFECYCLE_ATTESTATION_REFUSED"));
        assert_eq!(lease.diagnostics().last_phase, "CHECK_REFUSED");
    }

    #[test]
    fn access_loss_does_not_reuse_cached_identity() {
        let fake = Fake::new();
        let lease = attest_with(&fake, 41, deadline()).unwrap();
        fake.observe_error.set(true);
        assert!(lease.check(&fake, deadline()).is_err());
        fake.observe_error.set(false);
        assert!(lease.check(&fake, deadline()).is_err());
    }

    #[test]
    fn attestation_post_hash_process_change_refuses() {
        let fake = Fake::new();
        fake.change_on_second_observation.set(true);
        assert!(attest_with(&fake, 41, deadline()).is_err());
    }

    #[test]
    fn incorrect_expected_digest_is_not_adopted() {
        let fake = Fake::new();
        let lease = attest_with(&fake, 41, deadline()).unwrap();
        let wrong = "0".repeat(64);
        assert!(lease.verify_expected(&fake, &binding(&wrong), deadline()).is_err());
        assert_eq!(lease.identity.elf_sha256, IMAGE_SHA);
        assert!(lease.verify_expected(&fake, &binding(IMAGE_SHA), deadline()).is_err());
    }

    #[test]
    fn invalid_elf_length_and_initial_read_failure_refuse() {
        for change in 0..4 {
            let fake = Fake::new();
            match change { 0 => fake.bytes.borrow_mut()[0] = 0,
                1 => { fake.bytes.borrow_mut().pop(); },
                2 => fake.read_error.set(true), _ => fake.current.borrow_mut().image.size = ELF_CAP + 1 }
            assert!(attest_with(&fake, 41, deadline()).is_err());
        }
    }

    #[cfg(test)]
    mod close_diagnostic_tests {
        use super::*;

        struct FailAfterFirstImage { image: FakeImage, returned_chunk: bool }
        impl Read for FailAfterFirstImage {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                if self.returned_chunk { return Err(io::Error::other("fixed diagnostic fixture error")); }
                let n = self.image.read(out)?;
                self.returned_chunk = n > 0;
                Ok(n)
            }
        }
        struct FailAfterFirst { fake: Fake }
        impl ImageObservation for FailAfterFirst {
            type Image = FailAfterFirstImage;
            fn observe(&self, pid: u32, limit: Instant) -> Result<(ProcessState, Self::Image)> {
                let (state, image) = self.fake.observe(pid, limit)?;
                Ok((state, FailAfterFirstImage { image, returned_chunk: false }))
            }
            fn state(&self, image: &Self::Image) -> Result<FileState> { self.fake.state(&image.image) }
        }
        #[test]
        fn partial_read_counter_survives_original_read_error() {
            let fake = Fake::new();
            let mut bytes = vec![0u8; 65537];
            bytes[..4].copy_from_slice(b"\x7fELF");
            *fake.bytes.borrow_mut() = bytes;
            fake.current.borrow_mut().image.size = 65537;
            let source = FailAfterFirst { fake };
            let mut progress = AttestationProgress::new();
            let result = attest_with_progress(&source, 41, deadline(), Some(&mut progress));
            assert_eq!(result.err().as_deref(), Some("LIFECYCLE_PROC_EXE_READ"));
            assert_eq!(progress.hash_bytes_read(), 65536);
            assert!(!progress.hash_complete());
            assert_eq!((progress.stage_name(), progress.hash_step_name()), ("FULL_ELF_HASH", "READ"));
            assert_eq!(source.fake.read_calls.get(), 1);
            assert_eq!(source.fake.observations.get(), 1);
        }
        #[test]
        fn completed_hash_counter_does_not_attest_changed_process() {
            let fake = Fake::new();
            fake.change_on_second_observation.set(true);
            let mut progress = AttestationProgress::new();
            let result = attest_with_progress(&fake, 41, deadline(), Some(&mut progress));
            assert_eq!(result.err().as_deref(), Some("LIFECYCLE_ATTESTED_IMAGE_CHANGED"));
            assert_eq!(progress.hash_bytes_read(), IMAGE.len() as u64);
            assert!(progress.hash_complete());
            assert_eq!(progress.stage_name(), "POST_HASH_IDENTITY");
            assert_eq!((fake.observations.get(), fake.read_calls.get()), (2, 2));
        }
        #[test]
        fn legacy_noop_wrapper_keeps_observations_reads_identity_and_refusal() {
            let legacy = Fake::new();
            let optional = Fake::new();
            let a = attest_with(&legacy, 41, deadline()).unwrap();
            let b = attest_with_progress(&optional, 41, deadline(), None).unwrap();
            assert!(a.identity == b.identity);
            assert_eq!((legacy.observations.get(), legacy.read_calls.get()),
                (optional.observations.get(), optional.read_calls.get()));
            assert_eq!((a.diagnostics().full_hash_count, a.diagnostics().full_hash_bytes,
                a.diagnostics().cheap_check_count),
                (b.diagnostics().full_hash_count, b.diagnostics().full_hash_bytes,
                b.diagnostics().cheap_check_count));
            let legacy_bad = Fake::new(); let optional_bad = Fake::new();
            legacy_bad.change_on_second_observation.set(true);
            optional_bad.change_on_second_observation.set(true);
            assert_eq!(attest_with(&legacy_bad, 41, deadline()).err(),
                attest_with_progress(&optional_bad, 41, deadline(), None).err());
            assert_eq!((legacy_bad.observations.get(), legacy_bad.read_calls.get()),
                (optional_bad.observations.get(), optional_bad.read_calls.get()));
        }
        #[test]
        fn expired_original_deadline_reads_no_image_bytes() {
            let fake = Fake::new();
            let expired = Instant::now() - Duration::from_secs(1);
            let mut progress = AttestationProgress::new();
            let result = attest_with_progress(&fake, 41, expired, Some(&mut progress));
            assert_eq!(result.err().as_deref(), Some("LIFECYCLE_DEADLINE_EXPIRED"));
            assert_eq!(progress.hash_bytes_read(), 0);
            assert!(!progress.hash_complete());
            assert_eq!(progress.stage_name(), "INITIAL_PROC_OBSERVE");
            assert_eq!((fake.observations.get(), fake.read_calls.get()), (0, 0));
        }
    }
    #[test]
    fn expired_existing_deadline_refuses_without_new_hash_or_refresh() {
        let fake = Fake::new();
        let lease = attest_with(&fake, 41, deadline()).unwrap();
        let reads = fake.read_calls.get();
        let expired = Instant::now() - Duration::from_secs(1);
        assert!(lease.check(&fake, expired).is_err());
        assert_eq!(fake.read_calls.get(), reads);
        assert!(lease.check(&fake, deadline()).is_err());
    }
}
