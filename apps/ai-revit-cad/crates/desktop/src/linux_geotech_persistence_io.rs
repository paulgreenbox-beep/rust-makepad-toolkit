//! SOURCE13 PRIVATE CANDIDATE. Linux x86_64 descriptor-anchored persistence.
//! No invocation occurs merely by compiling or loading this source module.
//! Trusted launcher creates a unique 0700 root and pins dev/ino/uid. Requests
//! choose only two registered absolute names, never a root/basename/expected ID.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
// Separate descendant; the frozen helper03 create-only module is unchanged.
#[path="linux_geotech_owned_update.rs"]
pub mod owned_update;
use super::linux_geotech_persistence_contract::{RegisteredPaths,INPUT_BASENAME,OUTPUT_BASENAME,STAGE_BASENAME};
use std::{ffi::CString,fs::{File,Metadata},io::{Read,Write},os::{fd::{AsRawFd,FromRawFd},unix::fs::MetadataExt}};

const AT_FDCWD:i32=-100;
const O_RDONLY:i32=0; const O_RDWR:i32=2; const O_CREAT:i32=0x40; const O_EXCL:i32=0x80;
const O_NONBLOCK:i32=0x800; // Linux x86_64; prevents FIFO peer-open waits.
const O_DIRECTORY:i32=0x10000; const O_NOFOLLOW:i32=0x20000; const O_CLOEXEC:i32=0x80000;
pub const FILE_CAP:u64=64 << 20;
unsafe extern "C" {
    fn openat(dirfd:i32,path:*const std::ffi::c_char,flags:i32,mode:u32)->i32;
    fn linkat(oldfd:i32,old:*const std::ffi::c_char,newfd:i32,new:*const std::ffi::c_char,flags:i32)->i32;
    fn unlinkat(dirfd:i32,path:*const std::ffi::c_char,flags:i32)->i32;
    fn geteuid()->u32;
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct RootIdentity {pub dev:u64,pub ino:u64,pub uid:u32}
impl RootIdentity {
    pub fn from_metadata(m:&Metadata)->Self {Self {dev:m.dev(),ino:m.ino(),uid:m.uid()}}
    fn matches(&self,m:&Metadata)->bool {
        m.is_dir() && *self==Self::from_metadata(m) && m.mode() & 0o7777 == 0o700 && m.uid()==unsafe {geteuid()}
    }
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct FileStamp {
    pub dev:u64,pub ino:u64,pub uid:u32,pub mode:u32,pub links:u64,pub len:u64,
    pub mtime:i64,pub mtime_ns:i64,pub ctime:i64,pub ctime_ns:i64,
}
impl FileStamp {
    pub fn from_metadata(m:&Metadata)->Self {Self {dev:m.dev(),ino:m.ino(),uid:m.uid(),mode:m.mode(),links:m.nlink(),len:m.len(),
        mtime:m.mtime(),mtime_ns:m.mtime_nsec(),ctime:m.ctime(),ctime_ns:m.ctime_nsec()}}
    fn regular_private(&self)->bool {self.mode & 0o170000 == 0o100000 && self.mode & 0o7777 == 0o600
        && self.links==1 && self.len>0 && self.len<=FILE_CAP && self.uid==unsafe {geteuid()}}
    fn same_content_object(&self,other:&Self)->bool {
        self.dev==other.dev && self.ino==other.ino && self.uid==other.uid && self.mode==other.mode
            && self.len==other.len && self.mtime==other.mtime && self.mtime_ns==other.mtime_ns
    }
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct IoFailure {
    pub code:&'static str,
    // False certifies only that this call did not successfully create its
    // output link. It says nothing about another actor's colliding file.
    pub output_link_may_have_been_created:bool, pub stage_may_exist:bool,
}
fn failure(code:&'static str,output:bool,stage:bool)->IoFailure {
    IoFailure {code,output_link_may_have_been_created:output,stage_may_exist:stage}
}
#[derive(Clone,Debug)]
pub struct ReadReceipt {pub bytes:Vec<u8>,pub file:FileStamp,pub root:RootIdentity}
#[derive(Clone,Copy,Debug)]
pub struct WriteReceipt {pub file:FileStamp,pub root:RootIdentity,pub no_replace:bool}
pub struct RootLease {paths:RegisteredPaths,root:File,identity:RootIdentity}
fn cstring(value:&str)->Result<CString,IoFailure> {CString::new(value).map_err(|_|failure("PERSISTENCE_CSTRING",false,false))}
fn opened(dirfd:i32,name:&str,flags:i32,mode:u32)->Result<File,IoFailure> {
    let name=cstring(name)?;
    let fd=unsafe {openat(dirfd,name.as_ptr(),flags|O_CLOEXEC|O_NOFOLLOW,mode)};
    if fd<0 {Err(failure("PERSISTENCE_OPEN_NOFOLLOW",false,false))}
    else {Ok(unsafe {File::from_raw_fd(fd)})}
}
fn open_absolute_root(paths:&RegisteredPaths)->Result<File,IoFailure> {
    let mut directory=opened(AT_FDCWD,"/",O_RDONLY|O_DIRECTORY,0)?;
    for component in paths.root()[1..].split('/') {
        directory=opened(directory.as_raw_fd(),component,O_RDONLY|O_DIRECTORY,0)?;
    }
    Ok(directory)
}
fn metadata(file:&File)->Result<Metadata,IoFailure> {
    file.metadata().map_err(|_|failure("PERSISTENCE_FSTAT",false,false))
}
impl RootLease {
    pub fn acquire(paths:RegisteredPaths,expected:RootIdentity)->Result<Self,IoFailure> {
        let root=open_absolute_root(&paths)?;
        if !expected.matches(&metadata(&root)?) {return Err(failure("PERSISTENCE_ROOT_IDENTITY",false,false));}
        Ok(Self {paths,root,identity:expected})
    }
    fn current_root(&self)->Result<(),IoFailure> {
        let current=open_absolute_root(&self.paths)?;
        if !self.identity.matches(&metadata(&self.root)?) || !self.identity.matches(&metadata(&current)?) {
            return Err(failure("PERSISTENCE_ROOT_CHANGED",false,false));
        }
        Ok(())
    }
    fn leaf(&self,name:&str,flags:i32,mode:u32)->Result<File,IoFailure> {
        if ![INPUT_BASENAME,OUTPUT_BASENAME,STAGE_BASENAME].contains(&name) {
            return Err(failure("PERSISTENCE_BASENAME_NOT_REGISTERED",false,false));
        }
        // All leaf opens, including identity reopens, must be nonblocking BEFORE
        // fstat. O_NOFOLLOW alone does not prevent a substituted FIFO wait.
        // Regular-file latency still relies on a trusted local filesystem and
        // the separate Root-owned finite execution deadline.
        opened(self.root.as_raw_fd(),name,flags|O_NONBLOCK,mode)
    }
    /// The exact expected file stamp and content SHA-256 are trusted startup or
    /// prior successful write-receipt pins. Caller MUST verify its SHA-256 over
    /// returned bounded bytes before Document::from_json_str. No fresh adoption.
    pub fn read_registered(&self,path:&str,expected:FileStamp)->Result<ReadReceipt,IoFailure> {
        if !self.paths.registered(path) {return Err(failure("PERSISTENCE_READ_PATH_UNREGISTERED",false,false));}
        self.current_root()?;
        let name=if path==self.paths.input() {INPUT_BASENAME}else{OUTPUT_BASENAME};
        let mut file=self.leaf(name,O_RDONLY,0)?;
        let before=FileStamp::from_metadata(&metadata(&file)?);
        if before!=expected || !before.regular_private() {return Err(failure("PERSISTENCE_INPUT_PIN_CHANGED",false,false));}
        let mut bytes=Vec::new();
        (&mut file).take(FILE_CAP+1).read_to_end(&mut bytes).map_err(|_|failure("PERSISTENCE_READ",false,false))?;
        let after=FileStamp::from_metadata(&metadata(&file)?);
        self.current_root()?;
        // Reopen by exact root FD too: replacement of the leaf cannot borrow
        // the retained descriptor's proof, even if its bytes happen to match.
        let current=FileStamp::from_metadata(&metadata(&self.leaf(name,O_RDONLY,0)?)?);
        if before!=after || before!=current || bytes.len() as u64!=before.len {
            return Err(failure("PERSISTENCE_INPUT_CHANGED_DURING_READ",false,false));
        }
        Ok(ReadReceipt {bytes,file:after,root:self.identity})
    }
    pub fn write_new(&self,path:&str,bytes:&[u8])->Result<WriteReceipt,IoFailure> {
        self.write_impl(path,bytes,||{})
    }
    fn write_impl(&self,path:&str,bytes:&[u8],before_publish:impl FnOnce())->Result<WriteReceipt,IoFailure> {
        self.write_with_hooks(path,bytes,before_publish,||{})
    }
    // Internal test seams only; production write_new supplies two empty hooks.
    // Neither hook is exposed through RPC or trusted-startup path configuration.
    fn write_with_hooks(&self,path:&str,bytes:&[u8],before_publish:impl FnOnce(),after_publish:impl FnOnce())
        ->Result<WriteReceipt,IoFailure> {
        if !self.paths.save_path(path) || bytes.is_empty() || bytes.len() as u64>FILE_CAP {
            return Err(failure("PERSISTENCE_WRITE_PATH_OR_CAP",false,false));
        }
        self.current_root()?;
        // No overwrite, including stale stages from ambiguous prior attempts.
        let mut staged=self.leaf(STAGE_BASENAME,O_RDWR|O_CREAT|O_EXCL,0o600)?;
        let mut published=false;
        let result=(|| {
            staged.write_all(bytes).map_err(|_|failure("PERSISTENCE_STAGE_WRITE",false,true))?;
            staged.sync_all().map_err(|_|failure("PERSISTENCE_STAGE_SYNC",false,true))?;
            let initial=FileStamp::from_metadata(&metadata(&staged)?);
            if !initial.regular_private() || initial.len!=bytes.len() as u64 {
                return Err(failure("PERSISTENCE_STAGE_METADATA",false,true));
            }
            before_publish();
            self.current_root().map_err(|_|failure("PERSISTENCE_PARENT_CHANGED_BEFORE_LINK",false,true))?;
            let stage_name=self.leaf(STAGE_BASENAME,O_RDONLY,0).map_err(|_|failure("PERSISTENCE_STAGE_NAME_CHANGED",false,true))?;
            if FileStamp::from_metadata(&metadata(&stage_name)?)!=initial {
                return Err(failure("PERSISTENCE_STAGE_NAME_CHANGED",false,true));
            }
            let from=cstring(STAGE_BASENAME)?;let to=cstring(OUTPUT_BASENAME)?;
            // linkat without AT_SYMLINK_FOLLOW never traverses a leaf symlink.
            // Destination EEXIST is authoritative even if preflight was absent.
            if unsafe {linkat(self.root.as_raw_fd(),from.as_ptr(),self.root.as_raw_fd(),to.as_ptr(),0)}!=0 {
                return Err(failure("PERSISTENCE_LINK_NO_REPLACE_REFUSED",false,true));
            }
            published=true;
            after_publish();
            let mut output=self.leaf(OUTPUT_BASENAME,O_RDONLY,0).map_err(|_|failure("PERSISTENCE_PUBLISHED_OPEN_UNKNOWN",true,true))?;
            let linked=FileStamp::from_metadata(&metadata(&output)?);
            if !initial.same_content_object(&linked) || linked.links!=2 {
                return Err(failure("PERSISTENCE_PUBLISHED_IDENTITY_UNKNOWN",true,true));
            }
            let mut readback=Vec::new();
            (&mut output).take(FILE_CAP+1).read_to_end(&mut readback).map_err(|_|failure("PERSISTENCE_PUBLISHED_READ_UNKNOWN",true,true))?;
            if readback.as_slice()!=bytes || FileStamp::from_metadata(&metadata(&output)?)!=linked {
                return Err(failure("PERSISTENCE_PUBLISHED_CONTENT_UNKNOWN",true,true));
            }
            // Only remove the exact operation-owned stage after its descriptor
            // and name still identify this inode. Never remove the output.
            let source_now=self.leaf(STAGE_BASENAME,O_RDONLY,0).map_err(|_|failure("PERSISTENCE_STAGE_RELEASE_UNKNOWN",true,true))?;
            if FileStamp::from_metadata(&metadata(&source_now)?)!=linked {
                return Err(failure("PERSISTENCE_STAGE_RELEASE_UNKNOWN",true,true));
            }
            if unsafe {unlinkat(self.root.as_raw_fd(),from.as_ptr(),0)}!=0 {
                return Err(failure("PERSISTENCE_STAGE_RELEASE_UNKNOWN",true,true));
            }
            self.root.sync_all().map_err(|_|failure("PERSISTENCE_DIRECTORY_SYNC_UNKNOWN",true,false))?;
            self.current_root().map_err(|_|failure("PERSISTENCE_PARENT_CHANGED_AFTER_LINK",true,false))?;
            let final_stamp=FileStamp::from_metadata(&metadata(&output)?);
            let name_stamp=FileStamp::from_metadata(&metadata(&self.leaf(OUTPUT_BASENAME,O_RDONLY,0)
                .map_err(|_|failure("PERSISTENCE_FINAL_NAME_UNKNOWN",true,false))?)?);
            if !final_stamp.regular_private() || !initial.same_content_object(&final_stamp) || final_stamp!=name_stamp {
                return Err(failure("PERSISTENCE_FINAL_IDENTITY_UNKNOWN",true,false));
            }
            Ok(WriteReceipt {file:final_stamp,root:self.identity,no_replace:true})
        })();
        // Deliberately preserve failed stage/output artifacts for Root review.
        // A future retry must use a NEW admitted root, never reuse this result.
        result.map_err(|mut e| {e.stage_may_exist=true;e.output_link_may_have_been_created |= published;e})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs,os::unix::fs::{PermissionsExt,symlink},time::{SystemTime,UNIX_EPOCH}};
    fn fixture()->(std::path::PathBuf,RegisteredPaths,RootIdentity) {
        let root=std::env::temp_dir().join(format!("ai-revit-source13-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();fs::set_permissions(&root,fs::Permissions::from_mode(0o700)).unwrap();
        let paths=RegisteredPaths::new(root.to_str().unwrap()).unwrap();
        let identity=RootIdentity::from_metadata(&fs::symlink_metadata(&root).unwrap());
        (root,paths,identity)
    }
    fn private_file(path:impl AsRef<std::path::Path>,bytes:&[u8]) {
        fs::write(&path,bytes).unwrap();fs::set_permissions(path,fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[test] fn collision_created_after_staging_never_clobbers_existing_output() {
        let (_root,p,id)=fixture();let io=RootLease::acquire(p.clone(),id).unwrap();
        let e=io.write_impl(&p.output(),b"saved",||private_file(p.output(),b"do-not-overwrite")).unwrap_err();
        assert_eq!(e.code,"PERSISTENCE_LINK_NO_REPLACE_REFUSED");assert!(!e.output_link_may_have_been_created);
        assert_eq!(fs::read(p.output()).unwrap(),b"do-not-overwrite");
    }
    #[test] fn output_symlink_never_writes_its_target_and_input_symlink_is_refused() {
        let (root,p,id)=fixture();private_file(root.join("sentinel"),b"sentinel");
        symlink(root.join("sentinel"),p.output()).unwrap();symlink(root.join("sentinel"),p.input()).unwrap();
        let io=RootLease::acquire(p.clone(),id).unwrap();
        assert!(io.write_new(&p.output(),b"hostile").is_err());
        let pin=FileStamp::from_metadata(&fs::metadata(root.join("sentinel")).unwrap());
        assert!(io.read_registered(&p.input(),pin).is_err());assert_eq!(fs::read(root.join("sentinel")).unwrap(),b"sentinel");
    }
    #[test] fn renamed_parent_is_detected_at_publication_without_following_replacement() {
        let (root,p,id)=fixture();let moved=root.with_extension("moved");
        let io=RootLease::acquire(p.clone(),id).unwrap();
        let e=io.write_impl(&p.output(),b"saved",|| {fs::rename(&root,&moved).unwrap();fs::create_dir(&root).unwrap();
            fs::set_permissions(&root,fs::Permissions::from_mode(0o700)).unwrap();}).unwrap_err();
        assert_eq!(e.code,"PERSISTENCE_PARENT_CHANGED_BEFORE_LINK");
        assert!(!root.join(OUTPUT_BASENAME).exists());assert!(!moved.join(OUTPUT_BASENAME).exists());
    }
    #[test] fn ancestor_symlink_and_wrong_root_inode_do_not_acquire() {
        let (root,p,mut id)=fixture();id.ino=id.ino.wrapping_add(1);
        assert!(RootLease::acquire(p,id).is_err());
        let alias=root.with_extension("alias");symlink(&root,&alias).unwrap();
        let q=RegisteredPaths::new(alias.to_str().unwrap()).unwrap();
        let real_id=RootIdentity::from_metadata(&fs::metadata(&root).unwrap());
        assert!(RootLease::acquire(q,real_id).is_err());
    }
    #[test] fn exact_read_pin_detects_same_bytes_replaced_inode_and_no_replace_is_one_use() {
        let (root,p,id)=fixture();private_file(p.input(),b"same-bytes");
        let pin=FileStamp::from_metadata(&fs::metadata(p.input()).unwrap());
        let io=RootLease::acquire(p.clone(),id).unwrap();assert_eq!(io.read_registered(&p.input(),pin).unwrap().bytes,b"same-bytes");
        fs::rename(p.input(),root.join("old-input")).unwrap();private_file(p.input(),b"same-bytes");
        assert!(io.read_registered(&p.input(),pin).is_err());
        let saved=io.write_new(&p.output(),b"new-saved").unwrap();assert!(saved.no_replace);
        assert_eq!(io.read_registered(&p.output(),saved.file).unwrap().bytes,b"new-saved");
        assert!(io.write_new(&p.output(),b"must-not-clobber").is_err());assert_eq!(fs::read(p.output()).unwrap(),b"new-saved");
    }
    // Fixtures intentionally remain under unique temp roots for Root evidence.
    // No recursive cleanup, user source mutation, launch, solver or GUI is here.
    #[test] fn replaced_stage_cannot_publish_another_inode_or_a_symlink() {
        let (root,p,id)=fixture();let io=RootLease::acquire(p.clone(),id).unwrap();
        let e=io.write_impl(&p.output(),b"intended",|| {
            fs::rename(root.join(STAGE_BASENAME),root.join("preserved-owned-stage")).unwrap();
            symlink(root.join("outside-target"),root.join(STAGE_BASENAME)).unwrap();
        }).unwrap_err();
        assert_eq!(e.code,"PERSISTENCE_STAGE_NAME_CHANGED");assert!(!p.save_path(root.join("outside-target").to_str().unwrap()));
        assert!(!root.join(OUTPUT_BASENAME).exists());assert!(!root.join("outside-target").exists());
    }    unsafe extern "C" {fn mkfifo(path:*const std::ffi::c_char,mode:u32)->i32;}
    fn fifo_without_writer(path:impl AsRef<std::path::Path>) {
        use std::os::unix::ffi::OsStrExt;
        let path=CString::new(path.as_ref().as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe {mkfifo(path.as_ptr(),0o600)},0);
    }
    fn run_fifo_case(case:&str) {
        use std::os::unix::fs::FileTypeExt;
        let (root,p,id)=fixture();private_file(root.join("external-sentinel"),b"untouched");
        eprintln!("SOURCE13_FIFO_CASE:{case} ROOT:{}",root.display());
        let io=RootLease::acquire(p.clone(),id).unwrap();
        match case {
            "input"=>{
                private_file(p.input(),b"registered-input");
                let pin=FileStamp::from_metadata(&fs::metadata(p.input()).unwrap());
                fs::rename(p.input(),root.join("preserved-input")).unwrap();fifo_without_writer(p.input());
                let refused=io.read_registered(&p.input(),pin).unwrap_err();
                assert_eq!(refused.code,"PERSISTENCE_INPUT_PIN_CHANGED");
                assert!(!refused.output_link_may_have_been_created);
                assert_eq!(fs::read(root.join("preserved-input")).unwrap(),b"registered-input");
                assert!(fs::symlink_metadata(p.input()).unwrap().file_type().is_fifo());
            }
            "stage"=>{
                let refused=io.write_impl(&p.output(),b"intended",|| {
                    fs::rename(root.join(STAGE_BASENAME),root.join("preserved-stage")).unwrap();
                    fifo_without_writer(root.join(STAGE_BASENAME));
                }).unwrap_err();
                assert_eq!(refused.code,"PERSISTENCE_STAGE_NAME_CHANGED");
                assert!(!refused.output_link_may_have_been_created);assert!(refused.stage_may_exist);
                assert!(!root.join(OUTPUT_BASENAME).exists());
                assert_eq!(fs::read(root.join("preserved-stage")).unwrap(),b"intended");
                assert!(fs::symlink_metadata(root.join(STAGE_BASENAME)).unwrap().file_type().is_fifo());
            }
            "output"=>{
                let refused=io.write_with_hooks(&p.output(),b"intended",||{},|| {
                    fs::rename(p.output(),root.join("preserved-output")).unwrap();fifo_without_writer(p.output());
                }).unwrap_err();
                assert_eq!(refused.code,"PERSISTENCE_PUBLISHED_IDENTITY_UNKNOWN");
                assert!(refused.output_link_may_have_been_created);assert!(refused.stage_may_exist);
                assert_eq!(fs::read(root.join("preserved-output")).unwrap(),b"intended");
                assert!(fs::symlink_metadata(p.output()).unwrap().file_type().is_fifo());
            }
            _=>panic!("closed FIFO case enum required"),
        }
        assert_eq!(fs::read(root.join("external-sentinel")).unwrap(),b"untouched");
    }
    // A regression that omits O_NONBLOCK must not hang the rest of the suite.
    // This test-only child is the SAME executable, one sequential owned child
    // at a time, with exact case enum and Child-handle kill+wait on deadline.
    // Root must separately admit this descendant and keep its outer deadline.
    struct OwnedFifoProbe(std::process::Child);
    impl Drop for OwnedFifoProbe {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(),Ok(Some(_))) {let _=self.0.kill();let _=self.0.wait();}
        }
    }
    #[test] fn fifo_child_probe() {
        if let Ok(case)=std::env::var("AI_REVIT_SOURCE13_FIFO_CHILD") {
            run_fifo_case(&case);println!("SOURCE13_FIFO_CASE_PASSED:{case}:END");
        }
    }
    #[test] fn fifo_input_stage_output_refuse_under_finite_owned_child_deadline() {
        use std::{process::{Command,Stdio},time::{Duration,Instant}};
        assert!(std::env::var_os("AI_REVIT_SOURCE13_FIFO_CHILD").is_none());
        for case in ["input","stage","output"] {
            let child=Command::new(std::env::current_exe().unwrap())
                .args(["--exact","linux_geotech_persistence_io::tests::fifo_child_probe","--nocapture","--test-threads=1"])
                .env("AI_REVIT_SOURCE13_FIFO_CHILD",case).stdout(Stdio::piped()).spawn().unwrap();
            let mut owned=OwnedFifoProbe(child);let deadline=Instant::now()+Duration::from_secs(2);
            loop {
                // The original absolute deadline is never renewed by a poll.
                // Parent descheduling cannot make an already-late exit pass.
                assert!(Instant::now()<deadline,"FIFO case {case} exceeded 2s before poll; owned child will be killed and waited");
                if let Some(status)=owned.0.try_wait().unwrap() {
                    assert!(Instant::now()<deadline,"FIFO case {case} exceeded 2s before exit acceptance; owned child will be killed and waited");
                    assert!(status.success(),"FIFO case {case} failed");
                    let mut output=Vec::new();
                    owned.0.stdout.take().unwrap().take(4097).read_to_end(&mut output).unwrap();
                    assert!(output.len()<=4096,"FIFO probe output cap");
                    let output=std::str::from_utf8(&output).unwrap();
                    let marker=format!("SOURCE13_FIFO_CASE_PASSED:{case}:END");
                    assert_eq!(output.matches(&marker).count(),1,"exact child case acknowledgement missing");
                    assert!(Instant::now()<deadline,"FIFO case {case} exceeded 2s before case acceptance; owned child will be killed and waited");
                    break;
                }
                assert!(Instant::now()<deadline,"FIFO case {case} exceeded 2s; owned child will be killed and waited");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}
