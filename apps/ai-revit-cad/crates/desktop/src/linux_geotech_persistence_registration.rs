//! Private trusted-startup registration shared by the App and native client.
//! No request-time root/path/identity adoption. No build/run has been performed.
use crate::{linux_lifecycle_wire as life,linux_geotech_persistence_contract as policy,
    linux_geotech_persistence_io as io};
use serde::{Deserialize,Serialize};
use std::{fs::{self,OpenOptions},io::Read,os::unix::fs::{MetadataExt,OpenOptionsExt},path::Path};
pub const REGISTRATION_FILE:&str="persistence-registration.json";
const REGISTRATION_CAP:usize=16 << 10;
const O_NONBLOCK:i32=0x800;const O_NOFOLLOW:i32=0x20000;const O_CLOEXEC:i32=0x80000;
pub fn sha(value:&str)->bool {value.len()==64 && value.bytes().all(|c|c.is_ascii_digit()||(b'a'..=b'f').contains(&c))}
pub fn digest(value:&str)->Result<[u8;32],String> {
    life::ensure(sha(value),"PERSISTENCE_DIGEST_INVALID")?;let mut out=[0;32];
    for (i,pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let nibble=|c:u8|if c<=b'9' {c-b'0'}else{c-b'a'+10};out[i]=(nibble(pair[0])<<4)|nibble(pair[1]);
    } Ok(out)
}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stamp {
    pub dev:u64,pub ino:u64,pub uid:u32,pub mode:u32,pub links:u64,pub len:u64,
    pub mtime:i64,pub mtime_ns:i64,pub ctime:i64,pub ctime_ns:i64,
}
impl Stamp {
    pub fn native(&self)->io::FileStamp {io::FileStamp {dev:self.dev,ino:self.ino,uid:self.uid,mode:self.mode,
        links:self.links,len:self.len,mtime:self.mtime,mtime_ns:self.mtime_ns,ctime:self.ctime,ctime_ns:self.ctime_ns}}
    pub fn from_native(s:io::FileStamp)->Self {Self {dev:s.dev,ino:s.ino,uid:s.uid,mode:s.mode,links:s.links,
        len:s.len,mtime:s.mtime,mtime_ns:s.mtime_ns,ctime:s.ctime,ctime_ns:s.ctime_ns}}
}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Persisted {
    pub uuid:String,pub revision:u64,pub semantic_sha256:String,
    pub archive_sha256:Option<String>,pub report_sha256:Option<String>,
}
impl Persisted {
    pub fn native(&self)->Result<policy::PersistedKey,String> {
        life::ensure(!self.uuid.is_empty()&&self.uuid.len()<=128,"PERSISTENCE_UUID_INVALID")?;
        Ok(policy::PersistedKey {uuid:self.uuid.clone(),revision:self.revision,semantic_digest:digest(&self.semantic_sha256)?,
            archive_digest:self.archive_sha256.as_deref().map(digest).transpose()?,
            report_digest:self.report_sha256.as_deref().map(digest).transpose()?})
    }
}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredFile {pub stamp:Stamp,pub sha256:String,pub document:Persisted}
impl RegisteredFile {
    fn validate(&self)->Result<(),String> {life::ensure(sha(&self.sha256)&&self.stamp.len>0&&self.stamp.len<=io::FILE_CAP
        &&self.stamp.links==1&&self.stamp.mode&0o170000==0o100000&&self.stamp.mode&0o7777==0o600,
        "PERSISTENCE_REGISTERED_FILE_INVALID")?;self.document.native().map(|_|())}
}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Root {pub dev:u64,pub ino:u64,pub uid:u32}
impl Root {pub fn native(&self)->io::RootIdentity {io::RootIdentity {dev:self.dev,ino:self.ino,uid:self.uid}}}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub schema:String,pub run_id:String,pub session_id:String,pub owner:String,pub elf_sha256:String,
    pub root_path:String,pub root:Root,pub input:Option<RegisteredFile>,pub owned_output:Option<RegisteredFile>,
    pub allow_first_save:bool,pub allow_owned_update:bool,pub update_sequence:u64,
}
impl Registration {
    pub fn paths(&self)->Result<policy::RegisteredPaths,String> {policy::RegisteredPaths::new(&self.root_path).map_err(str::to_owned)}
    pub fn validate(&self,b:&life::Bootstrap)->Result<(),String> {
        life::ensure(self.schema=="source13-persistence-registration-v2"&&self.run_id==b.run_id
            &&self.session_id==b.session_id&&self.owner==b.owner&&self.elf_sha256==b.elf_sha256
            &&sha(&self.elf_sha256)&&self.root.dev!=0&&self.root.ino!=0
            &&(!self.allow_first_save||self.owned_output.is_none())
            &&self.update_sequence<=io::owned_update::UPDATE_LIMIT
            &&(self.owned_output.is_some()||self.update_sequence==0),"PERSISTENCE_REGISTRATION_BINDING")?;
        let _=self.paths()?;life::ensure(self.input.is_some()||self.owned_output.is_some(),"PERSISTENCE_REGISTERED_INPUT_REQUIRED")?;
        if let Some(input)=&self.input {input.validate()?;}if let Some(output)=&self.owned_output {output.validate()?;}Ok(())
    }
    pub fn bind(&self,b:&life::Binding)->Result<(),String> {life::ensure(self.run_id==b.run_id&&self.session_id==b.session_id
        &&self.owner==b.owner&&self.elf_sha256==b.elf_sha256,"PERSISTENCE_INSTALLED_BINDING_CHANGED")}
}
fn same(a:&fs::Metadata,b:&fs::Metadata)->bool {(a.dev(),a.ino(),a.uid(),a.mode(),a.nlink(),a.len(),a.mtime(),a.mtime_nsec(),a.ctime(),a.ctime_nsec())
    ==(b.dev(),b.ino(),b.uid(),b.mode(),b.nlink(),b.len(),b.mtime(),b.mtime_nsec(),b.ctime(),b.ctime_nsec())}
pub fn read()->Result<Registration,String> {
    let bootstrap=life::read_bootstrap()?; // existing trusted /run ownership gate
    let root=fs::symlink_metadata(life::ROOT).map_err(|_|"PERSISTENCE_REGISTRATION_ROOT")?;
    let path=Path::new(life::ROOT).join(REGISTRATION_FILE);
    let before=fs::symlink_metadata(&path).map_err(|_|"PERSISTENCE_REGISTRATION_MISSING")?;
    life::ensure(before.is_file()&&!before.file_type().is_symlink()&&before.mode()&0o7777==0o600
        &&before.nlink()==1&&(before.uid()==root.uid()||before.uid()==0)&&before.len()<=REGISTRATION_CAP as u64,
        "PERSISTENCE_REGISTRATION_FILE_INVALID")?;
    let mut file=OpenOptions::new().read(true).custom_flags(O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC).open(&path)
        .map_err(|_|"PERSISTENCE_REGISTRATION_OPEN")?;
    life::ensure(same(&before,&file.metadata().map_err(|_|"PERSISTENCE_REGISTRATION_STAT")?),"PERSISTENCE_REGISTRATION_CHANGED")?;
    let mut bytes=Vec::new();(&mut file).take(REGISTRATION_CAP as u64+1).read_to_end(&mut bytes).map_err(|_|"PERSISTENCE_REGISTRATION_READ")?;
    life::ensure(bytes.len()<=REGISTRATION_CAP&&same(&before,&file.metadata().map_err(|_|"PERSISTENCE_REGISTRATION_STAT")?)
        &&same(&before,&fs::symlink_metadata(path).map_err(|_|"PERSISTENCE_REGISTRATION_STAT")?)
        &&same(&root,&fs::symlink_metadata(life::ROOT).map_err(|_|"PERSISTENCE_REGISTRATION_ROOT")?),"PERSISTENCE_REGISTRATION_CHANGED")?;
    let value:Registration=serde_json::from_slice(&bytes).map_err(|_|"PERSISTENCE_REGISTRATION_SCHEMA")?;
    value.validate(&bootstrap)?;Ok(value)
}
#[cfg(test)]mod tests {
    use super::*;
    #[test]fn full_digest_and_stamp_types_are_strict_without_numeric_rounding(){
        assert!(digest(&"a".repeat(64)).is_ok());assert!(digest(&"A".repeat(64)).is_err());
        let raw=r#"{"dev":9007199254740993,"ino":1,"uid":0,"mode":33152,"links":1,"len":1,"mtime":0,"mtime_ns":0,"ctime":0,"ctime_ns":0}"#;
        assert_eq!(serde_json::from_str::<Stamp>(raw).unwrap().native().dev,9_007_199_254_740_993);
        assert!(serde_json::from_str::<Stamp>(&raw.replace("\"ino\":1","\"ino\":1,\"ino\":2")).is_err());
    }
}
