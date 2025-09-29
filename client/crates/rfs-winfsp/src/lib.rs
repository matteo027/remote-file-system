#![cfg(windows)]

use winfsp::filesystem::{FileInfo, FileSecurity, FileSystemContext};
use winfsp_sys::FILE_ACCESS_RIGHTS;
use winfsp::{FspError, Result as FspResult};
use winapi::um::winnt::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_REPARSE_POINT};
use filetime::FileTime;
use rfs_models::{FileEntry, RemoteBackend, SetAttrRequest, BackendError, ByteStream, BLOCK_SIZE, EntryType};
use std::collections::{BTreeMap, HashMap};
use std::cell::{RefCell, Cell};
use std::ffi::OsStr;
use std::fs::File;
use std::io::ErrorKind;
use std::path::{Path};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::runtime::Runtime;
use tokio_stream::StreamExt;

const TTL_FILE: Duration = Duration::from_secs(7);
const TTL_DIR: Duration = Duration::from_secs(3);
const FOPEN_NONSEEKABLE: u32 = 1 << 2; //bit per settare nonseekable flag (controllare meglio abi, non viene codificato in fuser)
const LARGE_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100 MB

fn map_error(error: &BackendError) -> FspError {
    match error {
        BackendError::NotFound(_) => {
            eprintln!("File not found.");
            FspError::IO(ErrorKind::NotFound)
        },
        BackendError::Unauthorized => {
            eprintln!("Unauthorized error.");
            FspError::IO(ErrorKind::PermissionDenied)
        },
        BackendError::Forbidden => {
            eprintln!("Forbidden error.");
            FspError::IO(ErrorKind::PermissionDenied)
        },
        BackendError::Conflict(err) => {
            eprintln!("Conflict error: {}", err);
            FspError::IO(ErrorKind::AlreadyExists)
        },
        BackendError::InternalServerError => {
            eprintln!("Internal server error.");
            FspError::IO(ErrorKind::Other)
        },
        BackendError::BadAnswerFormat => {
            eprintln!("Bad answer format.");
            FspError::IO(ErrorKind::InvalidData)
        },
        BackendError::ServerUnreachable => {
            eprintln!("Server unreachable.");
            FspError::IO(ErrorKind::TimedOut)
        },
        BackendError::Other(err) => {
            eprintln!("Backend error: {}", err);
            FspError::IO(ErrorKind::Other)
        },
    }
}

// SystemTime → Windows FILETIME
fn system_time_to_filetime(time: SystemTime) -> u64 {
    let ft = FileTime::from(time);
    // FileTime ha metodi per ottenere il valore raw Windows
    ft.unix_seconds() as u64 * 10_000_000 + (ft.nanoseconds() / 100) as u64 + 116444736000000000
}

#[inline]
fn entry_to_file_security(entry: &FileEntry, security_descriptor: Option<&mut [std::ffi::c_void]>) -> FileSecurity {
    FileSecurity {
        reparse: entry.kind == EntryType::Symlink,
        sz_security_descriptor: security_descriptor.map(|sd| sd.len() as u64).unwrap_or(0), // defaul Windows security descriptor
        attributes: match entry.kind {
            EntryType::Directory => FILE_ATTRIBUTE_DIRECTORY,
            EntryType::File => FILE_ATTRIBUTE_ARCHIVE,
            EntryType::Symlink => FILE_ATTRIBUTE_REPARSE_POINT,
        },
    }
}

#[inline]
fn entry_to_file_info(file_info: &mut FileInfo, entry: &FileEntry) -> () {
    
    file_info.file_attributes = match entry.kind {
        EntryType::Directory => FILE_ATTRIBUTE_DIRECTORY,
        EntryType::File => FILE_ATTRIBUTE_ARCHIVE,
        EntryType::Symlink => FILE_ATTRIBUTE_REPARSE_POINT,
    };
    
    file_info.file_size = entry.size as u64;
    file_info.allocation_size = ((entry.size + 511) / 512) * 512;
    file_info.creation_time = system_time_to_filetime(entry.btime);
    file_info.last_access_time = system_time_to_filetime(entry.atime);
    file_info.last_write_time = system_time_to_filetime(entry.mtime);
    file_info.change_time = system_time_to_filetime(entry.ctime);

}

struct StreamState{
    ino: u64,
    pos: u64,
    buffer: Vec<u8>,
    stream: Option<ByteStream>,
    eof: bool,
}

impl StreamState{
    fn new(ino: u64)->Self{
        Self{
            ino,
            pos: 0,
            buffer: Vec::new(),
            stream: None,
            eof: false,
        }
    }
}

enum ReadMode{
    SmallPages,
    LargeStream(StreamState),
}

pub struct RemoteFS<B: RemoteBackend> {
    backend: RefCell<B>,
    rt: Arc<Runtime>, // runtime per eseguire le operazioni asincrone

    // inode/path management
    lookup_ino: RefCell<HashMap<String, u64>>, //tiene riferimento al numero di riferimenti di naming per uno specifico inode, per gestire il caso di lookup multipli

    // file handle management
    next_fh: Cell<u64>, // file handle da allocare
    read_file_handles: RefCell<HashMap<u64, ReadMode>>, // mappa file handle, per gestire read in streaming continuo su file già aperti
    write_buffers: RefCell<HashMap<u64, BTreeMap<u64, Vec<u8>>>>, // buffer di scrittura per ogni file aperto; il valore è la coppia (buffer, offset)

    // opzioni di testing
    speed_testing: bool,
    speed_file: RefCell<Option<File>>,
}

impl<B: RemoteBackend> RemoteFS<B> {
    pub fn new(backend: B,runtime: Arc<Runtime>,speed_testing: bool,speed_file: Option<File>) -> Self {
        Self {
            backend: RefCell::new(backend),
            rt: runtime,
            lookup_ino: RefCell::new(HashMap::new()),
            next_fh: Cell::new(3), //0,1,2 di solito sono assegnati, da controllare
            read_file_handles: RefCell::new(HashMap::new()),
            write_buffers: RefCell::new(HashMap::new()),
            speed_testing,
            speed_file: RefCell::new(speed_file),
        }
    }

    fn flush_file(&self, ino: u64) -> Result<(), BackendError> {

        let mut start_offset = 0 as u64;
        let mut last_offset = 0 as u64;
        let mut prev_block_size = 0 as u64;
        // Collect the map's contents into a vector to avoid double mutable borrow
        let map_entries: Vec<(u64, Vec<u8>)> = {
            let mut write_buffers = self.write_buffers.borrow_mut();
            let map: &mut BTreeMap<u64, Vec<u8>> = write_buffers.get_mut(&ino).unwrap();
            let entries = map.iter().map(|(k, v)| (*k, v.clone())).collect();
            map.clear();
            entries
        };
        
        let mut buffer = Vec::<u8>::new();
        for (off, data) in map_entries.iter() {

            if buffer.is_empty() || last_offset + prev_block_size as u64 == *off {
                last_offset = *off;
                prev_block_size = data.len() as u64;
                buffer.extend_from_slice(&data);
            } else {
                // Flush the current buffer
                self.flush_buffer(&mut buffer, ino, start_offset)?;

                start_offset = *off;
                last_offset = *off;
                buffer.extend_from_slice(&data);
            }
        }

        // flushing last bytes
        if !buffer.is_empty() {
            self.flush_buffer(&mut buffer, ino, start_offset)?;
        }

        Ok(())
    }

    fn flush_buffer(&self, buffer: &mut Vec<u8>, ino: u64, offset: u64) -> Result<(), BackendError> {
        if !buffer.is_empty() {
            if buffer.len() > LARGE_FILE_SIZE as usize {
                self.backend.borrow_mut().write_stream(ino, offset, buffer.clone())?
            } else {
                self.backend.borrow_mut().write_chunk(ino, offset, buffer.clone())?;
            }
        }
        buffer.clear();
        Ok(())
    }
}

impl<B: RemoteBackend> FileSystemContext for RemoteFS<B> {
    type FileContext = Option<u64>;

    fn get_security_by_name(
        &self,
        file_name: &winfsp::U16CStr,
        security_descriptor: Option<&mut [std::ffi::c_void]>,
        _reparse_point_resolver: impl FnOnce(&winfsp::U16CStr) -> Option<winfsp::filesystem::FileSecurity>, // symlink managed server-side
    ) -> winfsp::Result<winfsp::filesystem::FileSecurity> {
        let path = file_name.to_string_lossy();
        
        let entry: FileEntry = match self.backend.borrow_mut().lookup_by_path(&path) {
            Ok(e) => e,
            Err(err) => return Err(map_error(&err)),
        };

        self.lookup_ino.borrow_mut().insert(path, entry.ino);
        
        Ok(entry_to_file_security(&entry, security_descriptor))
    }

    fn open(
        &self,
        file_name: &winfsp::U16CStr,
        create_options: u32,
        granted_access: FILE_ACCESS_RIGHTS,
        file_info: &mut winfsp::filesystem::OpenFileInfo,
    ) -> winfsp::Result<Self::FileContext> {
        let path = file_name.to_string_lossy();
        // lookup
        let ino = match self.lookup_ino.borrow().get(&path) {
            Some(&ino) => ino,
            None => return Err(FspError::IO(ErrorKind::NotFound)),
        };
        // getattr
        let entry = match self.backend.borrow_mut().get_attr(ino) {
            Ok(e) => e,
            Err(_) => return Err(FspError::IO(ErrorKind::NotFound)),
        };

        
        // updating OpenFileInfo with file's metadata
        let file_info_data = file_info.as_mut();
        entry_to_file_info(file_info_data, &entry);
        
        if entry.kind != EntryType::Directory {
            
            if entry.size > LARGE_FILE_SIZE {
                self.read_file_handles.borrow_mut().insert(ino, ReadMode::LargeStream(StreamState::new(entry.ino)));
            } else {
                self.read_file_handles.borrow_mut().insert(ino, ReadMode::SmallPages);
            }

            self.write_buffers.borrow_mut().insert(ino, BTreeMap::new());
        }
        
        Ok(Some(ino))
    }

    fn close(&self, context: Self::FileContext) {
        
        if let Some(ino) = context {
            
            if let Err(e) = self.flush_file(ino) {
                map_error(&e);
            }
            
            // Rimuovi dalle strutture
            self.read_file_handles.borrow_mut().remove(&ino);
            self.write_buffers.borrow_mut().remove(&ino);
        }

    }
}
