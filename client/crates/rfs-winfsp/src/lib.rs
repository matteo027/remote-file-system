#![cfg(windows)] // questo file è compilato solo su Windows

use glob::Pattern;
use winfsp::filesystem::{DirBuffer, DirInfo, DirMarker, FileInfo, FileSecurity, FileSystemContext, ModificationDescriptor, OpenFileInfo, VolumeInfo, WideNameInfo};
use winfsp_sys::{FILE_ACCESS_RIGHTS, FILE_FLAGS_AND_ATTRIBUTES};
use winfsp::{FspError, Result as FspResult, U16CStr};
use winapi::um::winnt::{FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_REPARSE_POINT, IO_REPARSE_TAG_SYMLINK};
use filetime::FileTime;
use rfs_models::{FileEntry, RemoteBackend, SetAttrRequest, BackendError, ByteStream, BLOCK_SIZE, EntryType};
use std::collections::{BTreeMap, HashMap};
use std::ffi::{c_void, OsStr};
use std::fs::File;
use std::io::ErrorKind;
use std::path::{Path};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
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
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => {
            let unix_seconds = duration.as_secs();
            let nanoseconds = duration.subsec_nanos();

            let windows_seconds = unix_seconds + 11644473600; // 1970 -> 1601
            
            // Windows FILETIME: 100-nanosecond intervals since January 1, 1601
            windows_seconds * 10_000_000 + (nanoseconds as u64 / 100)
        },
        Err(_) => {
            eprintln!("Warning: Invalid timestamp, using default");
            116444736000000000  // January 1, 1970 in Windows FILETIME
        }
    }
}

#[inline]
fn entry_to_file_security(entry: &FileEntry, security_descriptor: Option<&mut [std::ffi::c_void]>) -> FileSecurity {
    FileSecurity {
        reparse: entry.kind == EntryType::Symlink,
        sz_security_descriptor: 0,
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
    
    file_info.file_size = entry.size;
    file_info.allocation_size = if entry.kind == EntryType::Directory {
        4096
    } else {
        entry.size
    };
    file_info.creation_time = system_time_to_filetime(entry.btime);
    file_info.last_access_time = system_time_to_filetime(entry.atime);
    file_info.last_write_time = system_time_to_filetime(entry.mtime);
    file_info.change_time = system_time_to_filetime(entry.ctime);
    
    file_info.index_number = entry.ino;
    file_info.hard_links = entry.nlinks;
    file_info.ea_size = 0; // extended attributes size

    file_info.reparse_tag = match entry.kind { // symlink
        EntryType::Symlink => IO_REPARSE_TAG_SYMLINK,
        _ => 0,
    };

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
    backend: Mutex<B>,
    rt: Arc<Runtime>, // runtime per eseguire le operazioni asincrone

    // inode/path management
    lookup_ino: Mutex<HashMap<String, u64>>,

    // file handle management
    next_fh: AtomicU64, // file handle da allocare
    fh_to_entry: Mutex<HashMap<u64, FileEntry>>,
    read_file_handles: Mutex<HashMap<u64, ReadMode>>, // mappa file handle, per gestire read in streaming continuo su file già aperti
    write_buffers: Mutex<HashMap<u64, BTreeMap<u64, Vec<u8>>>>, // buffer di scrittura per ogni file aperto; il valore è la coppia (buffer, offset)
    files_to_delete: Mutex<Vec<u64>>, // fh (set by set_delete, used by cleanup)

    // opzioni di testing
    speed_testing: bool,
    speed_file: Mutex<Option<File>>,
}

impl<B: RemoteBackend> RemoteFS<B> {
    pub fn new(backend: B,runtime: Arc<Runtime>,speed_testing: bool,speed_file: Option<File>) -> Self {
        Self {
            backend: Mutex::new(backend),
            rt: runtime,
            lookup_ino: Mutex::new(HashMap::new()),
            next_fh: AtomicU64::new(3), //0,1,2 di solito sono assegnati, da controllare
            fh_to_entry: Mutex::new(HashMap::new()),
            read_file_handles: Mutex::new(HashMap::new()),
            write_buffers: Mutex::new(HashMap::new()),
            files_to_delete: Mutex::new(Vec::<u64>::new()),
            speed_testing,
            speed_file: Mutex::new(speed_file),
        }
    }

    fn get_parent_ino_and_fname<'a>(&self, path: &String) -> Result<(u64, String), FspError> {

        let path_obj = Path::new(&path);
        let parent_path = match path_obj.parent() {
            Some(p) => p.to_string_lossy().to_string(),
            None => "\\".to_string(), // root directory
        };
        let f_name = match path_obj.file_name() {
            Some(name) => name.to_string_lossy().to_string(),
            None => match parent_path.as_str() {
                "\\" => {
                    self.lookup_ino.lock().expect("Mutex poisoned").insert(String::from("\\"), 1u64);
                    return Ok((1u64, String::from("")));
                },
                _ => return Err(FspError::IO(ErrorKind::InvalidInput))
            },
        };

        let parent_ino = match self.lookup_ino.lock().expect("Mutex poisoned").get(&parent_path) {
            Some(&ino) => ino,
            None => return Err(FspError::IO(ErrorKind::NotFound))
        };
        Ok((parent_ino, f_name))
    }

    fn flush_file(&self, fh: u64) -> Result<(), BackendError> {

        let mut start_offset = 0u64;
        let mut last_offset = 0u64;
        let mut prev_block_size = 0u64;

        let ino = match self.fh_to_entry.lock().expect("Mutex poisoned").get(&fh) {
            Some(e) => e.ino,
            None => return Err(BackendError::NotFound(String::from("File handle associated to no ino"))),
        };

        // Collect the map's contents into a vector to avoid double mutable borrow
        let map_entries: Vec<(u64, Vec<u8>)> = {
            let write_buffers_lock = self.write_buffers.lock().expect("mutex poisoned");
            let mut write_buffers = write_buffers_lock;
            let map: &mut BTreeMap<u64, Vec<u8>> = write_buffers.get_mut(&fh).unwrap();
            let entries = map.iter().map(|(k, v)| (*k, v.clone())).collect();
            map.clear();
            entries
        };
        
        let mut buffer = Vec::<u8>::new();
        for (off, data) in map_entries.iter() {

            if buffer.is_empty() || last_offset + prev_block_size as u64 == *off {
                if buffer.is_empty() {
                    start_offset = *off;
                }
                buffer.extend_from_slice(&data);
            } else {
                // Flush the current buffer
                self.flush_buffer(&mut buffer, ino, start_offset)?;
                start_offset = *off;
                buffer.clear();
                buffer.extend_from_slice(&data);
            }
            last_offset = *off;
            prev_block_size = data.len() as u64;
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
                self.backend.lock().expect("Mutex poisoned").write_stream(ino, offset, buffer.clone())?
            } else {
                self.backend.lock().expect("Mutex poisoned").write_chunk(ino, offset, buffer.clone())?;
            }
        }
        buffer.clear();
        Ok(())
    }

    fn find_parent_and_name(&self, ino: u64) -> Result<(u64, String), FspError> {
 
        let lookup_cache = self.lookup_ino.lock().expect("Mutex poisoned");
        
        let path = lookup_cache.iter()
            .find(|(_, cached_ino)| **cached_ino == ino)
            .map(|(path, _)| path.clone());
        
        drop(lookup_cache);
        
        let path = match path {
            Some(p) => p,
            None => return Err(FspError::IO(ErrorKind::NotFound)),
        };
        
        self.get_parent_ino_and_fname(&path)
    }
}

impl<B: RemoteBackend> FileSystemContext for RemoteFS<B> {
    type FileContext = u64; // file handle

    fn get_security_by_name(
        &self,
        file_name: &winfsp::U16CStr,
        security_descriptor: Option<&mut [std::ffi::c_void]>,
        _reparse_point_resolver: impl FnOnce(&winfsp::U16CStr) -> Option<winfsp::filesystem::FileSecurity>,
    ) -> winfsp::Result<winfsp::filesystem::FileSecurity> {
        let path = file_name.to_string_lossy();
        println!("get_security_by_name: path='{}'", path);
        
        let (parent_ino, f_name) = match self.get_parent_ino_and_fname(&path) {
            Ok(result) => {
                println!("  → parent_ino={}, f_name='{}'", result.0, result.1);
                result
            },
            Err(e) => {
                println!("  → get_parent_ino_and_fname FAILED: {:?}", e);
                return Err(e);
            }
        };
        
        let entry: FileEntry = match self.backend.lock().expect("Mutex poisoned").lookup(parent_ino, &f_name) {
            Ok(e) => {
                println!("  → lookup SUCCESS: ino={}, name='{}', kind={:?}", e.ino, e.name, e.kind);
                e
            },
            Err(err) => {
                println!("  → lookup FAILED: {}", err);
                return Err(map_error(&err));
            }
        };

        self.lookup_ino.lock().expect("Mutex poisoned").insert(path.clone(), entry.ino);
        println!("  → cached: '{}' → ino={}", path, entry.ino);
        
        Ok(entry_to_file_security(&entry, security_descriptor))
    }

    fn open(
        &self,
        file_name: &winfsp::U16CStr,
        _create_options: u32,
        _granted_access: FILE_ACCESS_RIGHTS,
        file_info: &mut winfsp::filesystem::OpenFileInfo,
    ) -> winfsp::Result<Self::FileContext> {
        let path = file_name.to_string_lossy();
        println!("open: path='{}'", path);
    
        // lookup
        let ino = match self.lookup_ino.lock().expect("Mutex poisoned").get(&path) {
            Some(&ino) => {
                println!("  → Found ino={} for path '{}'", ino, path);
                ino
            },
            None => {
                println!("  → Path '{}' not found in lookup cache", path);
                return Err(FspError::IO(ErrorKind::NotFound));
            }
        };
        
        // getattr
        let entry = match self.backend.lock().expect("Mutex poisoned").get_attr(ino) {
            Ok(e) => {
                println!("  → get_attr SUCCESS: ino={}, name='{}', size={}, kind={:?}", 
                    e.ino, e.name, e.size, e.kind);
                e
            },
            Err(err) => {
                println!("  → get_attr FAILED for ino {}: {}", ino, err);
                return Err(map_error(&err));
            }
        };

    // updating OpenFileInfo with file's metadata
    let file_info_data = file_info.as_mut();
    entry_to_file_info(file_info_data, &entry);

    let fh = self.next_fh.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    println!("  → Assigned file handle: {}", fh);
    
    self.fh_to_entry.lock().expect("Mutex poisoned").insert(fh, entry.clone());
    
    if entry.kind != EntryType::Directory {
        if entry.size > LARGE_FILE_SIZE {
            self.read_file_handles.lock().expect("Mutex poisoned").insert(fh, ReadMode::LargeStream(StreamState::new(entry.ino)));
        } else {
            self.read_file_handles.lock().expect("Mutex poisoned").insert(fh, ReadMode::SmallPages);
        }
        self.write_buffers.lock().expect("Mutex poisoned").insert(fh, BTreeMap::new());
    }
    
    println!("  → open SUCCESS: fh={}", fh);
    Ok(fh)
    }

    fn close(&self, context: Self::FileContext) {
        println!("close: '{}'", self.fh_to_entry.lock().expect("Mutex poisoned").get(&context).unwrap().name);

        let fh = context;
        
        self.fh_to_entry.lock().expect("Mutex poisoned").remove(&fh);
        self.read_file_handles.lock().expect("Mutex poisoned").remove(&fh);
        self.write_buffers.lock().expect("Mutex poisoned").remove(&fh);

    }

    fn create(
        &self,
        file_name: &U16CStr,
        create_options: u32,
        granted_access: FILE_ACCESS_RIGHTS,
        file_attributes: FILE_FLAGS_AND_ATTRIBUTES,
        _security_descriptor: Option<&[c_void]>,
        _allocation_size: u64,
        _extra_buffer: Option<&[u8]>,
        _extra_buffer_is_reparse_point: bool,
        file_info: &mut OpenFileInfo,
    ) -> winfsp::Result<Self::FileContext> {

        println!("create");
        
        let path = file_name.to_string_lossy();
        let (parent_ino, f_name) = self.get_parent_ino_and_fname(&path)?;

        let entry = if (file_attributes & FILE_ATTRIBUTE_DIRECTORY) != 0 {
                match self.backend.lock().expect("Mutex poisoned").create_dir(parent_ino, &f_name) {
                    Ok(e) => e,
                    Err(err) => return Err(map_error(&err))
                }
            } else {
                match self.backend.lock().expect("Mutex poisoned").create_file(parent_ino, &f_name) {
                    Ok(e) => e,
                    Err(err) => return Err(map_error(&err))
                }
        };

        self.lookup_ino.lock().expect("Mutex poisoned").insert(path.to_string(), entry.ino);

        self.open(file_name, create_options, granted_access, file_info)

    }

    /// Clean up a file.
    fn cleanup(&self, context: &Self::FileContext, file_name: Option<&U16CStr>, flags: u32) {
        println!("cleanup: '{}'", self.fh_to_entry.lock().expect("Mutex poisoned").get(context).unwrap().name);

        let fh = *context;
        
        // cleaning the file buffer(s)
        self.write_buffers.lock().expect("Mutex poisoned").remove(&fh);

        // removing the file
        let mut files_to_delete = self.files_to_delete.lock().expect("Mutex poisoned");
        if let Some(pos) = files_to_delete.iter().position(|&x| x == fh) {
                files_to_delete.remove(pos);
                drop(files_to_delete);
                
                let mut files_to_delete = self.files_to_delete.lock().expect("Mutex poisoned");
        if let Some(pos) = files_to_delete.iter().position(|&x| x == fh) {
            files_to_delete.remove(pos);
            drop(files_to_delete); // Release the lock
            
            if let Some(entry) = self.fh_to_entry.lock().expect("Mutex poisoned").get(&fh) {
                // ✅ Get parent_ino and filename from the file path
                let (parent_ino, filename) = match self.find_parent_and_name(entry.ino) {
                    Ok(result) => result,
                    Err(e) => {
                        println!("Failed to find parent for deletion: {:?}", e);
                        return;
                    }
                };
                
                match entry.kind {
                    EntryType::Directory => {
                        if let Err(e) = self.backend.lock().expect("Mutex poisoned").delete_dir(parent_ino, &filename) {
                            println!("Failed to delete directory: {}", e);
                        } else {
                            println!("Directory deleted successfully");
                        }
                    },
                    _ => { // File or Symlink
                        if let Err(e) = self.backend.lock().expect("Mutex poisoned").delete_file(parent_ino, &filename) {
                            println!("Failed to delete file: {}", e);
                        } else {
                            println!("File deleted successfully");
                        }
                    }
                }
                
                // ✅ Remove from path cache
                let mut lookup_cache = self.lookup_ino.lock().expect("Mutex poisoned");
                lookup_cache.retain(|_, &mut ino| ino != entry.ino);
            }
        }
        }
        
    }

    /// Flush a file or volume.
    ///
    /// If `context` is `None`, the request is to flush the entire volume.
    fn flush(&self, context: Option<&Self::FileContext>, file_info: &mut FileInfo) -> winfsp::Result<()> {
        println!("flush");

        match context {
            Some(file_context) => {
                let fh = *file_context;
                
                if !self.fh_to_entry.lock().expect("Mutex poisoned").contains_key(&fh) {
                    return Err(FspError::IO(ErrorKind::NotFound));
                }
                
                if self.write_buffers.lock().expect("Mutex poisoned").contains_key(&fh) {
                    match self.flush_file(fh) {
                        Ok(()) => {
                            self.get_file_info(file_context, file_info)?;
                        },
                        Err(e) => {
                            println!("Flush error: {}", e);
                            return Err(map_error(&e));
                        }
                    }
                } else {
                    println!("No write buffers to flush for fh {}", fh);
                    self.get_file_info(file_context, file_info)?;
                }
            },
            None => {
                let all_handles: Vec<u64> = {
                    let write_buffers = self.write_buffers.lock().expect("Mutex poisoned");
                    write_buffers.keys().cloned().collect()
                };
                
                for fh in all_handles {
                    if let Err(e) = self.flush_file(fh) {
                        println!("Warning: Failed to flush file handle {}: {}", fh, e);
                    }
                }
                
            }
        }
        
        Ok(())
    }

    fn get_file_info(&self, context: &Self::FileContext, file_info: &mut FileInfo) -> winfsp::Result<()> {
        println!("get_file_info: {}", self.fh_to_entry.lock().expect("Mutex poisoned").get(context).unwrap().name);
        
        let fh = *context;

        let cached_entry = {
            let fh_entries = self.fh_to_entry.lock().map_err(|_| FspError::IO(ErrorKind::Other))?;
            match fh_entries.get(&fh) {
                Some(entry) => entry.clone(),
                None => return Err(FspError::IO(ErrorKind::NotFound)),
            }
        };
        
        let fresh_entry = match self.backend.lock().expect("Mutex poisoned").get_attr(cached_entry.ino) {
            Ok(entry) => {
                self.fh_to_entry.lock().expect("Mutex poisoned").insert(fh, entry.clone());
                entry
            },
            Err(e) => return Err(map_error(&e)),
        };
        
        entry_to_file_info(file_info, &fresh_entry);
        
        Ok(())
    }

    /// Get file or directory security descriptor.
    fn get_security(
        &self,
        context: &Self::FileContext,
        security_descriptor: Option<&mut [c_void]>,
    ) -> winfsp::Result<u64> {
        println!("get_security");
        todo!()
    }

    /// Set file or directory security descriptor.
    fn set_security(
        &self,
        context: &Self::FileContext,
        security_information: u32,
        modification_descriptor: ModificationDescriptor,
    ) -> winfsp::Result<()> {
        println!("set_security");
        todo!()
    }

    /// Overwrite a file.
    fn overwrite(
        &self,
        context: &Self::FileContext,
        file_attributes: FILE_FLAGS_AND_ATTRIBUTES,
        replace_file_attributes: bool,
        allocation_size: u64,
        extra_buffer: Option<&[u8]>,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        println!("overwrite");
        todo!()
    }

    /// Read directory entries from a directory handle.
    fn read_directory(
            &self,
            context: &Self::FileContext,
            pattern: Option<&U16CStr>,
            marker: DirMarker,
            buffer: &mut [u8],
        ) -> winfsp::Result<u32> {

        println!("read_directory: {}", self.fh_to_entry.lock().expect("Mutex poisoned").get(context).unwrap().name);

        if !marker.is_none() {
            return Ok(0);
        }
        
        let fh = *context;

        let dir_entry = match self.fh_to_entry.lock().expect("Mutex poisoned").get(&fh) {
            Some(entry) => entry.clone(),
            None => return Err(FspError::IO(ErrorKind::NotFound)),
        };
        if dir_entry.kind != EntryType::Directory {
            return Err(FspError::IO(ErrorKind::NotADirectory));
        }

        let entries = self.backend.lock().expect("Mutex poisoned").list_dir(dir_entry.ino).map_err(|e|{map_error(&e)})?;

        let pattern_str = pattern.map(|p| p.to_string_lossy().to_string());

        let dir_buffer = DirBuffer::new();
        let buffer_lock = dir_buffer.acquire(true, Some(entries.len() as u32))?;

        for entry in entries.iter() {

            // filter
            if let Some(ref pat) = pattern_str {
                match Pattern::new(pat) {
                    Ok(p) => if !p.matches(&entry.name){
                        continue;
                    },
                    Err(_) => return Err(FspError::IO(ErrorKind::InvalidInput)), // invalid pattern
                }
            }


            let mut dir_info = DirInfo::<255>::new();
            dir_info.set_name(&entry.name)?;

            let file_info = dir_info.file_info_mut();
            entry_to_file_info(file_info, entry);

            buffer_lock.write(&mut dir_info)?;
        }

        drop(buffer_lock);

        Ok(dir_buffer.read(marker, buffer))
    }

    /// Renames a file or directory.
    fn rename(
        &self,
        context: &Self::FileContext,
        file_name: &U16CStr,
        new_file_name: &U16CStr,
        replace_if_exists: bool,
    ) -> winfsp::Result<()> {
        todo!()
    }

    /// Set file or directory basic information.
    #[allow(clippy::too_many_arguments)]
    fn set_basic_info(
        &self,
        context: &Self::FileContext,
        file_attributes: u32,
        creation_time: u64,
        last_access_time: u64,
        last_write_time: u64,
        last_change_time: u64,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        todo!()
    }

    /// Set the file delete flag.
    ///
    /// ## Safety
    /// The file should **never** be deleted in this function. Instead,
    /// set a flag to indicate that the file is to be deleted later by
    /// [`FileSystemContext::cleanup`](crate::filesystem::FileSystemContext::cleanup).
    fn set_delete(
        &self,
        context: &Self::FileContext,
        _file_name: &U16CStr,
        delete_file: bool,
    ) -> winfsp::Result<()> {

        let fh = *context;

        if delete_file {
            self.files_to_delete.lock().expect("Mutex poisoned").push(fh);
        } else {
            self.files_to_delete.lock().expect("Mutex poisoned").retain(|&x| x != fh);
        }

        Ok(())
    }

    /// Set the file or allocation size.
    fn set_file_size(
        &self,
        context: &Self::FileContext,
        new_size: u64,
        set_allocation_size: bool,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        todo!()
    }

    /// Read from a file. Return the number of bytes read,
    fn read(&self, context: &Self::FileContext, buffer: &mut [u8], offset: u64) -> winfsp::Result<u32> {
        println!("read");

        let fh = *context;
        let entry = {
            let fh_entries = self.fh_to_entry.lock().map_err(|_| FspError::IO(ErrorKind::Other))?;
            match fh_entries.get(&fh) {
                Some(entry) => entry.clone(),
                None => return Err(FspError::IO(ErrorKind::NotFound)),
            }
        };

        if entry.kind == EntryType::Directory {
            return Err(FspError::IO(ErrorKind::IsADirectory));
        }

        // Check bounds
        if offset >= entry.size {
            return Ok(0); // EOF
        }

        let read_size = std::cmp::min(buffer.len() as u64, entry.size - offset) as usize;
        if read_size == 0 {
            return Ok(0);
        }

        // Get the read mode for this file handle
        let mut read_handles = self.read_file_handles.lock().map_err(|_| FspError::IO(ErrorKind::Other))?;
        let read_mode = match read_handles.get_mut(&fh) {
            Some(rm) => rm,
            None => return Err(FspError::IO(ErrorKind::NotFound)),
        };
        
        match read_mode {
            ReadMode::LargeStream(state) => {
                let need= buffer.len() as usize;
                if offset as u64 != state.pos { 
                    return Err(FspError::IO(ErrorKind::InvalidInput)); // Non-seekable
                }

                if state.stream.is_none() && !state.eof {
                    match self.backend.lock().expect("Mutex poisoned").read_stream(entry.ino, state.pos) {
                        Ok(stream) => {
                            state.stream = Some(stream);
                            state.buffer.clear(); // clean the buffer for the new stram
                        }
                        Err(e) => return Err(map_error(&e)),
                    }
                }

                while state.buffer.len() < need && !state.eof {
                    let Some(stream)=state.stream.as_mut() else {break};
                    let next = self.rt.block_on(async { stream.next().await });
                    match next {
                        Some(Ok(bytes))=> {
                            if !bytes.is_empty() {
                                state.buffer.extend_from_slice(&bytes);
                            }
                        },
                        Some(Err(e)) => return Err(map_error(&e)),
                        None => { // EOF server side
                            state.eof = true;
                            break;
                        }
                    }
                }

                if state.buffer.is_empty() {
                    return Ok(0); // EOF
                }

                let take = need.min(state.buffer.len());
                let out:Vec<u8>  = state.buffer.drain(..take).collect();
                state.pos = state.pos.saturating_add(take as u64);
                
                buffer[..take].copy_from_slice(&out);
                Ok(take as u32)
            }
            ReadMode::SmallPages => {
                // chunk reading
                match self.backend.lock().expect("Mutex poisoned").read_chunk(entry.ino, offset as u64, read_size as u64) {
                    Ok(data) => {
                        let bytes_read = if data.len() < buffer.len() { data.len() } else { buffer.len() };
                        buffer[..bytes_read].copy_from_slice(&data[..bytes_read]);
                        Ok(bytes_read as u32)
                    }
                    Err(e) => Err(map_error(&e)),
                }
            },
        }

    }

    /// Write to a file. Return the number of bytes written.
    fn write(
        &self,
        context: &Self::FileContext,
        buffer: &[u8],
        offset: u64,
        write_to_eof: bool,
        _constrained_io: bool,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<u32> {
        println!("write");
        
        let fh = *context;
        let mut entry = {
            let fh_entries = self.fh_to_entry.lock().map_err(|_| FspError::IO(ErrorKind::Other))?;
            match fh_entries.get(&fh) {
                Some(entry) => entry.clone(),
                None => return Err(FspError::IO(ErrorKind::NotFound)),
            }
        };

        if entry.kind == EntryType::Directory {
            return Err(FspError::IO(ErrorKind::IsADirectory));
        }

        if !self.write_buffers.lock().expect("Mutex poisoned").contains_key(&fh) {
            return Err(FspError::IO(ErrorKind::NotFound)); // File handle not found in write buffers
        }

        let off = match write_to_eof {
            true => entry.size, // write to end of file
            false => offset
        };
        
        // Scope to limit the mutable borrow of write_buffers
        {
            let mut write_buffers = self.write_buffers.lock().expect("Mutex poisoned");
            let file_buffer = write_buffers.get_mut(&fh).ok_or(FspError::IO(ErrorKind::NotFound))?;
            file_buffer.insert(off, buffer.to_vec());
        }

        let new_end_offset = off + buffer.len() as u64;
        if new_end_offset > entry.size {
            entry.size = new_end_offset;
            entry.mtime = SystemTime::now();
            
            self.fh_to_entry.lock().expect("Mutex poisoned").insert(fh, entry.clone());
        }

        entry_to_file_info(file_info, &entry);

        Ok(buffer.len() as u32)

    }

    /// Get directory information for a single file or directory within a parent directory.
    ///
    /// This method is only called when [VolumeParams::pass_query_directory_filename](crate::host::VolumeParams::pass_query_directory_filename)
    /// is set to true, and the file system was created with [FileSystemParams::use_dir_info_by_name](crate::host::FileSystemParams).
    /// set to true.
    fn get_dir_info_by_name(
        &self,
        context: &Self::FileContext,
        file_name: &U16CStr,
        out_dir_info: &mut DirInfo,
    ) -> winfsp::Result<()> {
        println!("get_dir_info_by_name");
        todo!()
    }

    // SI POTREBBE ELIMINARE, chiamata solo in certe situazioni; DA RICONTROLLARE
    fn get_volume_info(&self, out_volume_info: &mut VolumeInfo) -> winfsp::Result<()> {
        println!("get_volume_info");
        
        // Imposta informazioni di base del volume
        out_volume_info.total_size = 1024 * 1024 * 1024 * 100; // 100GB fittizi
        out_volume_info.free_size = 1024 * 1024 * 1024 * 50;   // 50GB liberi fittizi
        
        // Set volume label
        let volume_label = "Remote-FS\0";
        out_volume_info.set_volume_label(volume_label);
        
        Ok(())
    }

}
