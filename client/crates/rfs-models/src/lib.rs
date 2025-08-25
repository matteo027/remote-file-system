use std::{pin::Pin, time::SystemTime};
use thiserror::Error;
use serde::{Deserialize, Serialize};
use tokio_stream::Stream;
use bytes::Bytes;

pub const BLOCK_SIZE: usize = 16 * 1024; // 16KB

// Modello di dominio per una voce di file system remoto, da utilizzare internamente e per caching
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// nome della voce (file o directory)
    pub name: String,
    /// percorso completo di file o directory
    pub path: String,
    /// indica se è directory
    pub is_dir: bool,
    /// inode assegnato dal server
    pub ino: u64,
    /// dimensione in byte
    pub size: u64,
    /// atime in secondi dall'epoch
    pub atime: SystemTime,
    /// mtime in secondi dall'epoch
    pub mtime: SystemTime,
    /// ctime in secondi dall'epoch
    pub ctime: SystemTime,
    /// btime in secondi dall'epoch (creazione)
    pub btime: SystemTime,
    /// permessi in formato octale (es. 0o755)
    pub perms: u16,
    /// numero di link
    pub nlinks: u32,
    /// user ID
    pub uid: u32,
    /// group ID
    pub gid: u32,
}

pub enum FileType {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetAttrRequest {
    pub perm: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub size: Option<u64>,
    pub flags: Option<u32>,
}

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Unauthorized")]
    Unauthorized,
    #[error("Conflict")]
    Conflict(String),
    #[error("Forbidden")]
    Forbidden,
    #[error("Internal server error")]
    InternalServerError,
    #[error("Bad answer format")]
    BadAnswerFormat,
    #[error("Server unreachable")]
    ServerUnreachable,
    #[error("Other: {0}")]
    Other(String),
}

pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, BackendError>> + Send>>;

pub trait RemoteBackend: Send + Sync {
    /// Lista il contenuto di una directory
    fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>, BackendError>;
    /// Ottiene metadati completi di un file o directory
    fn get_attr(&mut self, path: &str) -> Result<FileEntry, BackendError>;
    /// Crea un file vuoto e restituisce i metadati
    fn create_file(&mut self, path: &str) -> Result<FileEntry, BackendError>;
    /// Crea una directory e restituisce i metadati
    fn create_dir(&mut self, path: &str) -> Result<FileEntry, BackendError>;
    /// Elimina un file
    fn delete_file(&mut self, path: &str) -> Result<(), BackendError>;
    /// Elimina una directory
    fn delete_dir(&mut self, path: &str) -> Result<(), BackendError>;
    /// Legge un chunk di file (offset, lunghezza)
    fn read_chunk(&mut self, path: &str, offset: u64, size: u64)-> Result<Vec<u8>, BackendError>;
    /// Scrive un chunk di file (offset incluso) e restituisce il numero di byte scritti
    fn write_chunk(&mut self, path: &str, offset: u64, data: Vec<u8>) -> Result<u64, BackendError>;
    /// Rinomina un file o directory
    fn rename(&mut self, old_path: &str, new_path: &str) -> Result<FileEntry, BackendError>;
    /// Imposta gli attributi di un file o directory
    fn set_attr(&mut self, path: &str, attrs: SetAttrRequest) -> Result<FileEntry, BackendError>;

    /// legge un file intero come stream di byte (per file molto grandi)
    fn read_stream(&mut self, path: &str, offset: u64) -> Result<ByteStream, BackendError>;
    /// scrive un file intero come stream di byte (per file molto grandi)
    fn write_stream(&mut self, path: &str, offset: u64, data: Vec<u8>) -> Result<(), BackendError>;

    fn get_attr_if_modified_since(&mut self, path: &str, _since: SystemTime) -> Result<Option<FileEntry>, BackendError> {
        Ok(Some(self.get_attr(path)?))
    }
}
