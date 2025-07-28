use thiserror::Error;
use std::time::{Duration, SystemTime};

// Modello di dominio per una voce di file system remoto, da utilizzare internamente e per caching
#[derive(Debug, Clone)]
pub struct FsEntry {
    /// percorso completo di file o directory
    pub path: String,
    /// nome (ultima componente di path)
    pub name: String,
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
    /// permessi in formato octale (es. 0o755)
    pub perms: u16, 
    /// numero di link
    pub nlinks: u32,
    /// user ID
    pub uid: u32,
    /// group ID
    pub gid: u32,
}

pub struct FileChunk {
    pub data: Vec<u8>,
    pub offset: u64,
}

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Unauthorized")]  
    Unauthorized,
    #[error("Conflict")]
    Conflict(String),
    #[error("Internal server error")]
    InternalServerError,
    #[error("Bad answer format")]
    BadAnswerFormat,
    #[error("Server unreachable")]
    ServerUnreachable,
    #[error("Other: {0}")]
    Other(String),
}

pub trait RemoteBackend: Send + Sync {
    /// Lista il contenuto di una directory
    fn list_dir(&mut self, path: &str) -> Result<Vec<FsEntry>, BackendError>;
    /// Ottiene metadati completi di un file o directory
    fn get_attr(&mut self, path: &str) -> Result<FsEntry, BackendError>;
    /// Crea un file vuoto e restituisce i metadati
    fn create_file(&mut self, path: &str) -> Result<FsEntry, BackendError>;
    /// Crea una directory e restituisce i metadati
    fn create_dir(&mut self, path: &str) -> Result<FsEntry, BackendError>;
    /// Elimina un file
    fn delete_file(&mut self, path: &str) -> Result<(), BackendError>;
    /// Elimina una directory
    fn delete_dir(&mut self, path: &str) -> Result<(), BackendError>;
    /// Legge un chunk di file (offset, lunghezza)
    fn read_chunk(&mut self, path: &str, offset: u64, size: u64) -> Result<FileChunk, BackendError>;
    /// Scrive un chunk di file (offset incluso) e restituisce il numero di byte scritti
    fn write_chunk(&mut self, path: &str, offset: u64, data: Vec<u8>) -> Result<u64, BackendError>;
}