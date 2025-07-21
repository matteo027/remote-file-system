use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct DirectoryEntry {
    pub ino: u64, // Inode number
    pub name: String,
    pub is_dir: bool,
    pub size: u64, // Size in bytes
    pub perms: u16, // File permissions
    pub nlinks: u32, // Number of hard links
    pub atime: std::time::SystemTime, // Last access time
    pub mtime: std::time::SystemTime, // Last modified time
    pub ctime: std::time::SystemTime, // Creation time
    pub uid: u32, // User ID of the owner
    pub gid: u32, // Group ID of the owner

}

impl DirectoryEntry {
    pub fn new(ino: u64, name: String, is_dir: bool, size: u64, perms: u16, nlinks: u32, uid: u32, gid: u32, mtime: std::time::SystemTime, ctime: std::time::SystemTime, atime: std::time::SystemTime) -> Self {
        DirectoryEntry {
            ino,
            name,
            is_dir,
            size,
            perms,
            nlinks,
            atime,
            mtime,
            ctime,
            uid,
            gid,
        }
    }
}

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Unauthorized")]  
    Unauthorized,
    #[error("Other: {0}")]
    Other(String),
}

pub trait RemoteBackend:Send + Sync {
    fn new() -> Self where Self: Sized;
    fn list_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>, BackendError>;
    // fn read_file(&self, path: &str) -> Result<Vec<u8>, BackendError>;
    // fn write_file(&self, path: &str, data: &[u8]) -> Result<(), BackendError>;
    // fn delete_file(&self, path: &str) -> Result<(), BackendError>;
    fn create_dir(&mut self, entry: DirectoryEntry) -> Result<(), BackendError>;
    fn delete_dir(&mut self, path: &str) -> Result<(), BackendError>;
}