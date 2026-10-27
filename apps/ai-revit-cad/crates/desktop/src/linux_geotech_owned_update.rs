//! Owned expected-version Save, not an arbitrary file overwrite interface.
//! Linux x86_64/local filesystem only. Root must own this exclusive 0700 root
//! and its container; flock coordinates cooperating users, not hostile same-UID
//! writers. Every ambiguous publication is preserved, consumed, and never retried.
//! The caller first verifies expected_bytes SHA256 against its retained trusted
//! receipt/startup pin. This module repeats exact stamp AND byte verification at
//! actual I/O and retains the old version using atomic RENAME_EXCHANGE.
use super::*;
const LOCK_EX:i32=2;const LOCK_NB:i32=4;const LOCK_UN:i32=8;
const RENAME_EXCHANGE:u32=2;
pub const UPDATE_LIMIT:u64=16;
unsafe extern "C" {
    fn flock(fd:i32,operation:i32)->i32;
    fn renameat2(oldfd:i32,old:*const std::ffi::c_char,newfd:i32,new:*const std::ffi::c_char,flags:u32)->i32;
}
struct DirectoryLock(i32);
impl DirectoryLock {
    fn acquire(fd:i32)->Result<Self,OwnedUpdateFailure>{
        if unsafe {flock(fd,LOCK_EX|LOCK_NB)}!=0 {return Err(err("OWNED_UPDATE_ROOT_BUSY",false,false));}
        Ok(Self(fd))
    }
}
impl Drop for DirectoryLock {fn drop(&mut self){unsafe {flock(self.0,LOCK_UN);}}}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct OwnedUpdateFailure {
    pub code:&'static str,pub output_may_have_changed:bool,pub previous_name_may_exist:bool,
}
fn err(code:&'static str,output:bool,previous:bool)->OwnedUpdateFailure {
    OwnedUpdateFailure {code,output_may_have_changed:output,previous_name_may_exist:previous}
}
#[derive(Clone,Debug)]
pub struct OwnedUpdateReceipt {
    pub root:RootIdentity,pub file:FileStamp,pub previous:FileStamp,pub previous_name:String,
    pub sequence:u64,pub expected_bytes_verified:bool,pub atomic_exchange_verified:bool,
}
fn previous_name(sequence:u64)->Result<String,OwnedUpdateFailure>{
    if !(1..=UPDATE_LIMIT).contains(&sequence) {return Err(err("OWNED_UPDATE_SEQUENCE_INVALID",false,false));}
    Ok(format!(".saved.ai-revit.json.previous-{sequence:02}"))
}
fn stamp(file:&File)->Result<FileStamp,OwnedUpdateFailure>{
    metadata(file).map(|m|FileStamp::from_metadata(&m)).map_err(|_|err("OWNED_UPDATE_FSTAT",false,false))
}
fn bounded_bytes(file:&mut File)->Result<Vec<u8>,OwnedUpdateFailure>{
    let mut bytes=Vec::new();file.take(FILE_CAP+1).read_to_end(&mut bytes)
        .map_err(|_|err("OWNED_UPDATE_READ",false,false))?;
    if bytes.is_empty()||bytes.len() as u64>FILE_CAP {return Err(err("OWNED_UPDATE_READ_CAP",false,false));}
    Ok(bytes)
}
impl RootLease {
    fn previous_leaf(&self,sequence:u64,flags:i32,mode:u32)->Result<File,OwnedUpdateFailure>{
        let name=previous_name(sequence)?;
        // Closed derived basename only; NB precedes fstat for every leaf open.
        opened(self.root.as_raw_fd(),&name,flags|O_NONBLOCK,mode)
            .map_err(|_|err("OWNED_UPDATE_PREVIOUS_OPEN_NOFOLLOW",false,false))
    }
    fn update_root(&self)->Result<(),OwnedUpdateFailure>{
        self.current_root().map_err(|_|err("OWNED_UPDATE_ROOT_CHANGED",false,false))
    }
    fn update_current(&self,expected:FileStamp,expected_bytes:&[u8])->Result<File,OwnedUpdateFailure>{
        let mut file=self.leaf(OUTPUT_BASENAME,O_RDONLY,0)
            .map_err(|_|err("OWNED_UPDATE_EXPECTED_OPEN_NOFOLLOW",false,false))?;
        let before=stamp(&file)?;
        if !before.regular_private()||before!=expected {return Err(err("OWNED_UPDATE_EXPECTED_STAMP",false,false));}
        if bounded_bytes(&mut file)?.as_slice()!=expected_bytes || stamp(&file)?!=before {
            return Err(err("OWNED_UPDATE_EXPECTED_BYTES",false,false));
        }
        let named=self.leaf(OUTPUT_BASENAME,O_RDONLY,0).map_err(|_|err("OWNED_UPDATE_EXPECTED_NAME",false,false))?;
        if stamp(&named)?!=before {return Err(err("OWNED_UPDATE_EXPECTED_NAME",false,false));}
        Ok(file)
    }
    /// expected_bytes must already match the trusted old SHA256 at the caller.
    /// sequence comes only from trusted startup plus successful local receipts,
    /// never RPC. It consumes a fixed create_new name and is never reused after
    /// ambiguity. Requires mutable access: no same-instance overlapping update.
    pub fn write_owned_expected(&mut self,path:&str,expected:FileStamp,expected_bytes:&[u8],bytes:&[u8],sequence:u64)
        ->Result<OwnedUpdateReceipt,OwnedUpdateFailure>{
        self.write_owned_with_hooks(path,expected,expected_bytes,bytes,sequence,||{},||{},||{})
    }
    // Private counterexample seams; the real method supplies empty closures.
    fn write_owned_with_hooks(&mut self,path:&str,expected:FileStamp,expected_bytes:&[u8],bytes:&[u8],sequence:u64,
        before_verify:impl FnOnce(),at_exchange:impl FnOnce(),after_exchange:impl FnOnce())
        ->Result<OwnedUpdateReceipt,OwnedUpdateFailure>{
        if !self.paths.save_path(path)||bytes.is_empty()||bytes.len() as u64>FILE_CAP
            ||expected_bytes.is_empty()||expected_bytes.len() as u64>FILE_CAP||!expected.regular_private(){
            return Err(err("OWNED_UPDATE_PATH_OR_CAP",false,false));
        }
        let previous_name=previous_name(sequence)?;self.update_root()?;
        let _lock=DirectoryLock::acquire(self.root.as_raw_fd())?;
        self.update_root()?;
        // A wrong stamp or bytes pin is refused before creating a stage name.
        let expected_file=self.update_current(expected,expected_bytes)?;
        let mut staged=self.previous_leaf(sequence,O_RDWR|O_CREAT|O_EXCL,0o600)?;
        let mut exchange_attempted=false;
        let result=(||{
            staged.write_all(bytes).map_err(|_|err("OWNED_UPDATE_STAGE_WRITE",false,true))?;
            staged.sync_all().map_err(|_|err("OWNED_UPDATE_STAGE_SYNC",false,true))?;
            let new_stamp=stamp(&staged)?;
            if !new_stamp.regular_private()||new_stamp.len!=bytes.len() as u64 {return Err(err("OWNED_UPDATE_STAGE_METADATA",false,true));}
            before_verify();self.update_root()?;
            // Revalidate BOTH descriptor and root-relative names immediately
            // before atomic exchange. No lstat/preflight grants publication.
            if stamp(&expected_file)?!=expected {return Err(err("OWNED_UPDATE_EXPECTED_CHANGED",false,true));}
            let _current=self.update_current(expected,expected_bytes)?;
            let mut stage_name=self.previous_leaf(sequence,O_RDONLY,0)?;
            if stamp(&stage_name)?!=new_stamp||bounded_bytes(&mut stage_name)?.as_slice()!=bytes||stamp(&stage_name)?!=new_stamp {
                return Err(err("OWNED_UPDATE_STAGE_CHANGED",false,true));
            }
            self.update_root()?;
            let from=cstring(&previous_name).map_err(|_|err("OWNED_UPDATE_NAME",false,true))?;
            let to=cstring(OUTPUT_BASENAME).map_err(|_|err("OWNED_UPDATE_NAME",false,true))?;
            at_exchange();exchange_attempted=true;
            // Exchange, rather than replacing and deleting, preserves whatever
            // occupied BOTH names. No rename fallback, unlink, truncation or
            // overwrite-capable open exists. A raced foreign leaf is preserved
            // under previous_name and causes OUTCOME_UNKNOWN below.
            if unsafe {renameat2(self.root.as_raw_fd(),from.as_ptr(),self.root.as_raw_fd(),to.as_ptr(),RENAME_EXCHANGE)}!=0 {
                return Err(err("OWNED_UPDATE_EXCHANGE_REFUSED",true,true));
            }
            after_exchange();self.update_root()?;
            let mut previous=self.previous_leaf(sequence,O_RDONLY,0)?;
            let previous_stamp=stamp(&previous)?;
            if !previous_stamp.regular_private()||!expected.same_content_object(&previous_stamp)
                ||bounded_bytes(&mut previous)?.as_slice()!=expected_bytes||stamp(&previous)?!=previous_stamp
                ||stamp(&expected_file)?!=previous_stamp {
                return Err(err("OWNED_UPDATE_PREVIOUS_UNKNOWN",true,true));
            }
            let mut output=self.leaf(OUTPUT_BASENAME,O_RDONLY,0).map_err(|_|err("OWNED_UPDATE_OUTPUT_OPEN",true,true))?;
            let output_stamp=stamp(&output)?;
            if !output_stamp.regular_private()||!new_stamp.same_content_object(&output_stamp)
                ||bounded_bytes(&mut output)?.as_slice()!=bytes||stamp(&output)?!=output_stamp||stamp(&staged)?!=output_stamp {
                return Err(err("OWNED_UPDATE_OUTPUT_UNKNOWN",true,true));
            }
            self.root.sync_all().map_err(|_|err("OWNED_UPDATE_DIRECTORY_SYNC",true,true))?;
            self.update_root()?;
            // Last name checks cover renames during readback/sync. Subsequent
            // operations again verify these full pins; this is no durable lease
            // against a hostile writer outside the exclusive-root contract.
            let output_now=self.leaf(OUTPUT_BASENAME,O_RDONLY,0).map_err(|_|err("OWNED_UPDATE_FINAL_NAME",true,true))?;
            let previous_now=self.previous_leaf(sequence,O_RDONLY,0)?;
            if stamp(&output_now)?!=output_stamp||stamp(&previous_now)?!=previous_stamp
                ||stamp(&staged)?!=output_stamp||stamp(&expected_file)?!=previous_stamp {
                return Err(err("OWNED_UPDATE_FINAL_IDENTITY",true,true));
            }
            Ok(OwnedUpdateReceipt {root:self.identity,file:output_stamp,previous:previous_stamp,previous_name,
                sequence,expected_bytes_verified:true,atomic_exchange_verified:true})
        })();
        // Preserve every candidate, previous version and raced leaf. No rollback
        // or retry, even if a later check failed after a successful exchange.
        result.map_err(|mut e|{e.output_may_have_changed|=exchange_attempted;e.previous_name_may_exist=true;e})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs,os::unix::fs::{PermissionsExt,symlink},time::{SystemTime,UNIX_EPOCH}};
    fn fixture()->(std::path::PathBuf,RegisteredPaths,RootLease,FileStamp){
        let root=std::env::temp_dir().join(format!("ai-revit-owned-update-{}-{}",std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();fs::set_permissions(&root,fs::Permissions::from_mode(0o700)).unwrap();
        let paths=RegisteredPaths::new(root.to_str().unwrap()).unwrap();private_file(paths.output(),b"old-version");
        let pin=FileStamp::from_metadata(&fs::symlink_metadata(paths.output()).unwrap());
        let id=RootIdentity::from_metadata(&fs::symlink_metadata(&root).unwrap());
        let io=RootLease::acquire(paths.clone(),id).unwrap();(root,paths,io,pin)
    }
    fn private_file(path:impl AsRef<std::path::Path>,bytes:&[u8]){
        fs::write(&path,bytes).unwrap();fs::set_permissions(path,fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[test]fn two_owned_updates_retain_each_exact_previous_version(){
        let (root,p,mut io,old)=fixture();
        let a=io.write_owned_expected(&p.output(),old,b"old-version",b"analyzed-version",1).unwrap();
        assert!(a.atomic_exchange_verified&&a.expected_bytes_verified);assert_eq!(a.sequence,1);
        assert_eq!(fs::read(root.join(&a.previous_name)).unwrap(),b"old-version");assert_ne!(old.ino,a.file.ino);
        assert_eq!(io.read_registered(&p.output(),a.file).unwrap().bytes,b"analyzed-version");
        let b=io.write_owned_expected(&p.output(),a.file,b"analyzed-version",b"later-version",2).unwrap();
        assert_eq!(fs::read(root.join(&b.previous_name)).unwrap(),b"analyzed-version");
        assert_eq!(fs::read(root.join(&a.previous_name)).unwrap(),b"old-version");
        assert_eq!(io.read_registered(&p.output(),b.file).unwrap().bytes,b"later-version");
    }
    #[test]fn wrong_expected_bytes_and_replaced_same_bytes_inode_cannot_publish(){
        let (root,p,mut io,old)=fixture();
        let e=io.write_owned_expected(&p.output(),old,b"wrong-pin",b"new",1).unwrap_err();
        assert_eq!(e.code,"OWNED_UPDATE_EXPECTED_BYTES");assert!(!e.output_may_have_changed&&!e.previous_name_may_exist);
        assert!(!root.join(previous_name(1).unwrap()).exists());
        fs::rename(p.output(),root.join("preserved-original")).unwrap();private_file(p.output(),b"old-version");
        let e=io.write_owned_expected(&p.output(),old,b"old-version",b"new",1).unwrap_err();
        assert_eq!(e.code,"OWNED_UPDATE_EXPECTED_STAMP");assert!(!e.output_may_have_changed);
        assert_eq!(fs::read(p.output()).unwrap(),b"old-version");
    }
    #[test]fn existing_previous_name_is_not_adopted_or_replaced(){
        for symlink_case in [false,true]{
            let (root,p,mut io,old)=fixture();private_file(root.join("sentinel"),b"sentinel");
            let previous=root.join(previous_name(1).unwrap());
            if symlink_case{symlink(root.join("sentinel"),&previous).unwrap();}else{private_file(&previous,b"existing");}
            let e=io.write_owned_expected(&p.output(),old,b"old-version",b"new",1).unwrap_err();
            assert_eq!(e.code,"OWNED_UPDATE_PREVIOUS_OPEN_NOFOLLOW");assert!(!e.output_may_have_changed);
            assert_eq!(fs::read(p.output()).unwrap(),b"old-version");assert_eq!(fs::read(root.join("sentinel")).unwrap(),b"sentinel");
        }
    }
    #[test]fn parent_replaced_before_publication_is_refused_in_retained_root(){
        let (root,p,mut io,old)=fixture();let moved=root.with_extension("preserved-root");
        let e=io.write_owned_with_hooks(&p.output(),old,b"old-version",b"new",1,||{
            fs::rename(&root,&moved).unwrap();fs::create_dir(&root).unwrap();
            fs::set_permissions(&root,fs::Permissions::from_mode(0o700)).unwrap();private_file(root.join(OUTPUT_BASENAME),b"replacement-root");
        },||{},||{}).unwrap_err();
        assert_eq!(e.code,"OWNED_UPDATE_ROOT_CHANGED");assert!(!e.output_may_have_changed&&e.previous_name_may_exist);
        assert_eq!(fs::read(p.output()).unwrap(),b"replacement-root");assert_eq!(fs::read(moved.join(OUTPUT_BASENAME)).unwrap(),b"old-version");
    }
    #[test]fn final_window_foreign_output_is_preserved_and_never_accepted(){
        let (root,p,mut io,old)=fixture();
        let e=io.write_owned_with_hooks(&p.output(),old,b"old-version",b"new",1,||{},||{
            fs::rename(p.output(),root.join("preserved-original")).unwrap();private_file(p.output(),b"foreign-output");
        },||{}).unwrap_err();
        assert_eq!(e.code,"OWNED_UPDATE_PREVIOUS_UNKNOWN");assert!(e.output_may_have_changed&&e.previous_name_may_exist);
        assert_eq!(fs::read(root.join(previous_name(1).unwrap())).unwrap(),b"foreign-output");
        assert_eq!(fs::read(root.join("preserved-original")).unwrap(),b"old-version");assert_eq!(fs::read(p.output()).unwrap(),b"new");
        // An attempted retry with the stale trusted pin still cannot adopt new.
        assert!(io.write_owned_expected(&p.output(),old,b"old-version",b"retry",2).is_err());
    }
    #[test]fn raced_stage_symlink_and_post_exchange_output_are_preserved_unknown(){
        let (root,p,mut io,old)=fixture();private_file(root.join("sentinel"),b"sentinel");
        let e=io.write_owned_with_hooks(&p.output(),old,b"old-version",b"new",1,||{},||{
            fs::rename(root.join(previous_name(1).unwrap()),root.join("preserved-stage")).unwrap();
            symlink(root.join("sentinel"),root.join(previous_name(1).unwrap())).unwrap();
        },||{}).unwrap_err();
        assert!(e.output_may_have_changed&&e.previous_name_may_exist);assert_eq!(fs::read(root.join("sentinel")).unwrap(),b"sentinel");
        assert!(fs::symlink_metadata(p.output()).unwrap().file_type().is_symlink());
        let (root,p,mut io,old)=fixture();
        let e=io.write_owned_with_hooks(&p.output(),old,b"old-version",b"new",1,||{},||{},||{
            fs::rename(p.output(),root.join("preserved-new")).unwrap();private_file(p.output(),b"raced-after");
        }).unwrap_err();
        assert_eq!(e.code,"OWNED_UPDATE_OUTPUT_UNKNOWN");assert!(e.output_may_have_changed);
        assert_eq!(fs::read(root.join(previous_name(1).unwrap())).unwrap(),b"old-version");
        assert_eq!(fs::read(root.join("preserved-new")).unwrap(),b"new");assert_eq!(fs::read(p.output()).unwrap(),b"raced-after");
    }
    #[test]fn invalid_sequence_and_unregistered_path_create_no_internal_names(){
        let (root,p,mut io,old)=fixture();
        for sequence in [0,17,u64::MAX]{assert!(io.write_owned_expected(&p.output(),old,b"old-version",b"new",sequence).is_err());}
        assert!(io.write_owned_expected(&p.input(),old,b"old-version",b"new",1).is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(),1);assert_eq!(fs::read(p.output()).unwrap(),b"old-version");
    }
    unsafe extern "C"{fn mkfifo(path:*const std::ffi::c_char,mode:u32)->i32;}
    fn fifo(path:impl AsRef<std::path::Path>){
        use std::os::unix::ffi::OsStrExt;
        let name=CString::new(path.as_ref().as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe{mkfifo(name.as_ptr(),0o600)},0);
    }
    fn fifo_case(case:&str){
        use std::os::unix::fs::FileTypeExt;
        let (root,p,mut io,old)=fixture();private_file(root.join("sentinel"),b"sentinel");
        eprintln!("SOURCE13_UPDATE_FIFO_CASE:{case} ROOT:{}",root.display());
        let previous=root.join(previous_name(1).unwrap());
        match case{
            "output_before"=>{
                fs::rename(p.output(),root.join("preserved-old")).unwrap();fifo(p.output());
                let e=io.write_owned_expected(&p.output(),old,b"old-version",b"new",1).unwrap_err();
                assert_eq!(e.code,"OWNED_UPDATE_EXPECTED_STAMP");assert!(!e.output_may_have_changed);
                assert!(!previous.exists());assert!(fs::symlink_metadata(p.output()).unwrap().file_type().is_fifo());
            }
            "previous_before_verify"=>{
                let e=io.write_owned_with_hooks(&p.output(),old,b"old-version",b"new",1,||{
                    fs::rename(&previous,root.join("preserved-new")).unwrap();fifo(&previous);
                },||{},||{}).unwrap_err();
                assert_eq!(e.code,"OWNED_UPDATE_STAGE_CHANGED");assert!(!e.output_may_have_changed);
                assert_eq!(fs::read(p.output()).unwrap(),b"old-version");
            }
            "previous_after_exchange"=>{
                let e=io.write_owned_with_hooks(&p.output(),old,b"old-version",b"new",1,||{},||{},||{
                    fs::rename(&previous,root.join("preserved-old")).unwrap();fifo(&previous);
                }).unwrap_err();
                assert_eq!(e.code,"OWNED_UPDATE_PREVIOUS_UNKNOWN");assert!(e.output_may_have_changed);
                assert_eq!(fs::read(root.join("preserved-old")).unwrap(),b"old-version");
            }
            "output_after_exchange"=>{
                let e=io.write_owned_with_hooks(&p.output(),old,b"old-version",b"new",1,||{},||{},||{
                    fs::rename(p.output(),root.join("preserved-new")).unwrap();fifo(p.output());
                }).unwrap_err();
                assert_eq!(e.code,"OWNED_UPDATE_OUTPUT_UNKNOWN");assert!(e.output_may_have_changed);
                assert_eq!(fs::read(root.join("preserved-new")).unwrap(),b"new");
            }
            "previous_at_exchange"=>{
                let e=io.write_owned_with_hooks(&p.output(),old,b"old-version",b"new",1,||{},||{
                    fs::rename(&previous,root.join("preserved-new")).unwrap();fifo(&previous);
                },||{}).unwrap_err();
                assert_eq!(e.code,"OWNED_UPDATE_OUTPUT_UNKNOWN");assert!(e.output_may_have_changed);
                assert_eq!(fs::read(&previous).unwrap(),b"old-version");
                assert!(fs::symlink_metadata(p.output()).unwrap().file_type().is_fifo());
            }
            _=>panic!("closed owned-update FIFO case required"),
        }
        assert_eq!(fs::read(root.join("sentinel")).unwrap(),b"sentinel");
    }
    struct OwnedFifoChild(std::process::Child);
    impl Drop for OwnedFifoChild{fn drop(&mut self){
        if !matches!(self.0.try_wait(),Ok(Some(_))){let _=self.0.kill();let _=self.0.wait();}
    }}
    #[test]fn owned_update_fifo_child_probe(){
        if let Ok(case)=std::env::var("AI_REVIT_SOURCE13_UPDATE_FIFO_CHILD"){
            fifo_case(&case);println!("SOURCE13_UPDATE_FIFO_PASSED:{case}:END");
        }
    }
    #[test]fn owned_update_fifo_counterexamples_have_one_absolute_child_deadline(){
        use std::{process::{Command,Stdio},time::{Duration,Instant}};
        assert!(std::env::var_os("AI_REVIT_SOURCE13_UPDATE_FIFO_CHILD").is_none());
        for case in ["output_before","previous_before_verify","previous_after_exchange","output_after_exchange","previous_at_exchange"]{
            let child=Command::new(std::env::current_exe().unwrap())
                .args(["--exact","linux_geotech_persistence_io::owned_update::tests::owned_update_fifo_child_probe","--nocapture","--test-threads=1"])
                .env("AI_REVIT_SOURCE13_UPDATE_FIFO_CHILD",case).stdout(Stdio::piped()).spawn().unwrap();
            let mut owned=OwnedFifoChild(child);let deadline=Instant::now()+Duration::from_secs(2);
            loop{
                assert!(Instant::now()<deadline,"owned Update FIFO {case}: deadline before poll");
                if let Some(status)=owned.0.try_wait().unwrap(){
                    assert!(Instant::now()<deadline,"owned Update FIFO {case}: deadline before exit acceptance");
                    assert!(status.success(),"owned Update FIFO {case}: child failed");
                    let mut bytes=Vec::new();owned.0.stdout.take().unwrap().take(4097).read_to_end(&mut bytes).unwrap();
                    assert!(bytes.len()<=4096,"owned Update FIFO child stdout cap");
                    let output=std::str::from_utf8(&bytes).unwrap();let marker=format!("SOURCE13_UPDATE_FIFO_PASSED:{case}:END");
                    assert_eq!(output.matches(&marker).count(),1,"owned Update FIFO exact marker");
                    assert!(Instant::now()<deadline,"owned Update FIFO {case}: deadline before case acceptance");break;
                }
                assert!(Instant::now()<deadline,"owned Update FIFO {case}: deadline after poll");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}
