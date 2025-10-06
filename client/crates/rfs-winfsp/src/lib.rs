#![cfg(windows)] // questo file è compilato solo su Windows

use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::io::ErrorKind;
use std::path::{Path};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use glob::Pattern;
use rfs_models::{BackendError, ByteStream, EntryType, FileEntry, RemoteBackend, SetAttrRequest};
use tokio::runtime::Runtime;
use tokio_stream::StreamExt;
use winapi::um::winnt::{FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, IO_REPARSE_TAG_SYMLINK};
use winfsp::filesystem::{DirBuffer, DirInfo, DirMarker, FileInfo, FileSecurity, FileSystemContext, OpenFileInfo, VolumeInfo, WideNameInfo};
use winfsp::{FspError, Result as FspResult, U16CStr};
use winfsp_sys::{FILE_ACCESS_RIGHTS, FILE_FLAGS_AND_ATTRIBUTES};
use winfsp::constants::FspCleanupFlags;

const SDDL_ALLOW_ALL: &str = "O:BA G:SY D:(A;;FA;;;WD)";
const LARGE_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100 MB
const WINDOWS_TICKS_PER_SEC: u64 = 10_000_000;
const UNIX_EPOCH_TO_WINDOWS_SECS: u64 = 11_644_473_600;

fn sd_from_sddl(sddl: &str, dest: Option<&mut [c_void]>) -> Result<u64, FspError> {
    use windows_permissions::{LocalBox, SecurityDescriptor};
    use windows_sys::Win32::Security::GetSecurityDescriptorLength;
    use std::ptr;

    let sd: LocalBox<SecurityDescriptor> = sddl.parse().map_err(|_| FspError::IO(ErrorKind::InvalidData))?;
    let (len,scr_bytes): (usize, &[u8]) = unsafe{
        let psd=(&*sd) as *const SecurityDescriptor as *const c_void;
        let len = GetSecurityDescriptorLength(psd as *mut c_void) as usize;
        if len == 0 {
            return Err(FspError::IO(ErrorKind::InvalidData));
        }
        let bytes=std::slice::from_raw_parts(psd as *const u8, len);
        (len,bytes)
    };

    // Copia opzionale nel buffer del chiamante
    if let Some(out) = dest {
        let n = out.len().min(len);
        unsafe {
            ptr::copy_nonoverlapping(scr_bytes.as_ptr(), out.as_ptr() as *mut u8, n);
        }
    }

    Ok(len as u64)
}

fn map_error(error: &BackendError) -> FspError {
    match error {
        BackendError::NotFound(_) => {
            //eprintln!("File not found.");
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
        Ok(duration) => 
            (duration.as_secs()+ UNIX_EPOCH_TO_WINDOWS_SECS) * WINDOWS_TICKS_PER_SEC + (duration.subsec_nanos() as u64 / 100),
        Err(_) => {
            eprintln!("Warning: Invalid timestamp, using default");
            UNIX_EPOCH_TO_WINDOWS_SECS * WINDOWS_TICKS_PER_SEC  // January 1, 1970 in Windows FILETIME
        }
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
    pos: u64,
    buffer: Vec<u8>,
    stream: Option<ByteStream>,
    eof: bool,
}

impl StreamState{
    fn new()->Self{
        Self{
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
    files_to_delete: Mutex<HashMap<u64, String>>, // fh -> path (set by set_delete, used by cleanup)
}

impl<B: RemoteBackend> RemoteFS<B> {
    pub fn new(backend: B,runtime: Arc<Runtime>) -> Self {
        let mut ino_map=HashMap::new();
        ino_map.insert(String::from("\\"), 1u64); // root directory
        Self {
            backend: Mutex::new(backend),
            rt: runtime,
            lookup_ino: Mutex::new(ino_map),
            next_fh: AtomicU64::new(3), //0,1,2 di solito sono assegnati, da controllare
            fh_to_entry: Mutex::new(HashMap::new()),
            read_file_handles: Mutex::new(HashMap::new()),
            write_buffers: Mutex::new(HashMap::new()),
            files_to_delete: Mutex::new(HashMap::new()),
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
                "\\" => "".to_string(),
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

}

impl<B: RemoteBackend> FileSystemContext for RemoteFS<B> {
    type FileContext = u64; // file handle

    fn get_security_by_name(&self,file_name: &U16CStr,security_descriptor: Option<&mut [c_void]>,_reparse_point_resolver: impl FnOnce(&U16CStr) -> Option<FileSecurity>) -> FspResult<FileSecurity> {
        let path = file_name.to_string_lossy();
        println!("get_security_by_name: path='{}'", path);

        if path == "\\" {
            // root directory
            let secdesc_len = sd_from_sddl(SDDL_ALLOW_ALL, security_descriptor)?;
            return Ok(FileSecurity {
                reparse: false,
                sz_security_descriptor: secdesc_len,
                attributes: FILE_ATTRIBUTE_DIRECTORY,
            });
        }

        if path.ends_with("\\desktop.ini") {
            return Err(FspError::IO(ErrorKind::NotFound));
        }
        
        let (parent_ino, f_name) = self.get_parent_ino_and_fname(&path)?;
        let entry: FileEntry = self.backend.lock().expect("Mutex poisoned").lookup(parent_ino, &f_name).map_err(|err| map_error(&err))?;
        self.lookup_ino.lock().expect("Mutex poisoned").insert(path.clone(), entry.ino);

        let secdesc_len = sd_from_sddl(SDDL_ALLOW_ALL, security_descriptor)?;
        Ok(FileSecurity {
            reparse: matches!(entry.kind, EntryType::Symlink),
            sz_security_descriptor: secdesc_len,
            attributes: match entry.kind {
                EntryType::Directory => FILE_ATTRIBUTE_DIRECTORY,
                EntryType::File      => FILE_ATTRIBUTE_ARCHIVE,
                EntryType::Symlink   => FILE_ATTRIBUTE_REPARSE_POINT,
            },
        })
    }

    fn open(&self,file_name: &U16CStr,_create_options: u32,_granted_access: FILE_ACCESS_RIGHTS,file_info: &mut OpenFileInfo) -> FspResult<Self::FileContext> {
        let path = file_name.to_string_lossy();
        println!("open: path='{}'", path);
    
        // lookup
        let ino = *self.lookup_ino.lock().expect("Mutex poisoned").get(&path).ok_or(FspError::IO(ErrorKind::NotFound))?;
        // getattr
        let entry = self.backend.lock().expect("Mutex poisoned").get_attr(ino).map_err(|err| map_error(&err))?;

        // updating OpenFileInfo with file's metadata
        let file_info_data = file_info.as_mut();
        entry_to_file_info(file_info_data, &entry);

        let fh = self.next_fh.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        println!("  → Assigned file handle: {}", fh);
        
        self.fh_to_entry.lock().expect("Mutex poisoned").insert(fh, entry.clone());
        
        if entry.kind != EntryType::Directory {
            if entry.size > LARGE_FILE_SIZE {
                self.read_file_handles.lock().expect("Mutex poisoned").insert(fh, ReadMode::LargeStream(StreamState::new()));
            } else {
                self.read_file_handles.lock().expect("Mutex poisoned").insert(fh, ReadMode::SmallPages);
            }
            self.write_buffers.lock().expect("Mutex poisoned").insert(fh, BTreeMap::new());
        }
        
        Ok(fh)
    }

    fn close(&self, context: Self::FileContext) {
        println!("close");
        let fh = context;

        let need_flush = { self.write_buffers.lock().expect("Mutex").contains_key(&fh) };
        if need_flush {
            if let Err(e) = self.flush_file(fh) {
                eprintln!("Warning: flush on close failed: {}", e);
            }
        }

        self.fh_to_entry.lock().expect("Mutex poisoned").remove(&fh);
        self.read_file_handles.lock().expect("Mutex poisoned").remove(&fh);
        self.write_buffers.lock().expect("Mutex poisoned").remove(&fh);
    }

    fn create(&self,file_name: &U16CStr,create_options: u32,granted_access: FILE_ACCESS_RIGHTS,file_attributes: FILE_FLAGS_AND_ATTRIBUTES,_security_descriptor: Option<&[c_void]>,_allocation_size: u64,
        _extra_buffer: Option<&[u8]>,_extra_buffer_is_reparse_point: bool,file_info: &mut OpenFileInfo) -> FspResult<Self::FileContext> {
        println!("create");
        
        let path = file_name.to_string_lossy();
        let (parent_ino, f_name) = self.get_parent_ino_and_fname(&path)?;
        let entry = if (file_attributes & FILE_ATTRIBUTE_DIRECTORY) != 0 {
            self.backend.lock().expect("Mutex poisoned").create_dir(parent_ino, &f_name).map_err(|err| map_error(&err))?
        } else {
            self.backend.lock().expect("Mutex poisoned").create_file(parent_ino, &f_name).map_err(|err| map_error(&err))?
        };
        self.lookup_ino.lock().expect("Mutex poisoned").insert(path.to_string(), entry.ino);
        self.open(file_name, create_options, granted_access, file_info)
    }

    /// Clean up a file.
    fn cleanup(&self, context: &Self::FileContext, _file_name: Option<&U16CStr>, flags: u32) {
        println!("cleanup: '{}'", self.fh_to_entry.lock().expect("Mutex poisoned").get(context).unwrap().name);
        let fh = *context;

        // 1) Flush eventuali scritture buffered per questo handle
        if self.write_buffers.lock().expect("Mutex").contains_key(&fh)
        {
            if let Err(e) = self.flush_file(fh) {
                eprintln!("Warning: flush on close failed: {}", e);
            }
        }
        // pulisci comunque il buffer
        self.write_buffers.lock().expect("Mutex poisoned").remove(&fh);

        // 2) Serve cancellare?
        let delete_requested = FspCleanupFlags::FspCleanupDelete.is_flagged(flags) || self.files_to_delete.lock().expect("Mutex poisoned").contains_key(&fh);

        if !delete_requested {
            return;
        }

        // 3) Determina il path da cancellare
        let mut path_opt = self.files_to_delete.lock().expect("Mutex poisoned").remove(&fh);

        if path_opt.is_none() {
            if let Some(entry) = self.fh_to_entry.lock().expect("Mutex poisoned").get(&fh)
            {
                path_opt = Some(entry.path.clone().replace("/", "\\"));
            }
        }

        // 4) Esegui la rimozione lato backend
        if let (Some(path), Some(entry)) = (path_opt,self.fh_to_entry.lock().expect("Mutex poisoned").get(&fh).cloned()) {
            // (parent ino, file name) per le tue API delete_dir/delete_file
            let (parent_ino, filename) = match self.get_parent_ino_and_fname(&path) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("cleanup: parent lookup failed for '{}': {:?}", path, e);
                    return;
                }
            };

            match entry.kind {
                EntryType::Directory => {
                    if let Err(e) = self.backend.lock().expect("Mutex poisoned").delete_dir(parent_ino, &filename)
                    {
                        eprintln!("cleanup: delete_dir('{}') failed: {}", path, e);
                    }
                }
                _ => {
                    if let Err(e) = self.backend.lock().expect("Mutex poisoned").delete_file(parent_ino, &filename)
                    {
                        eprintln!("cleanup: delete_file('{}') failed: {}", path, e);
                    }
                }
            }

            // 5) Ripulisci la cache path->ino (togliamo l'ino dell'entry eliminata)
            let ino = entry.ino;
            let mut lookup_cache = self.lookup_ino.lock().expect("Mutex poisoned");
            lookup_cache.retain(|_, v| *v != ino);
        }
    }

    /// Flush a file or volume.
    ///
    /// If `context` is `None`, the request is to flush the entire volume.
    fn flush(&self, context: Option<&Self::FileContext>, file_info: &mut FileInfo) -> FspResult<()> {
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
                            eprintln!("Flush error: {}", e);
                            return Err(map_error(&e));
                        }
                    }
                } else {
                    eprintln!("No write buffers to flush for fh {}", fh);
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
                        eprintln!("Warning: Failed to flush file handle {}: {}", fh, e);
                    }
                }
                
            }
        }
        
        Ok(())
    }

    fn get_file_info(&self, context: &Self::FileContext, file_info: &mut FileInfo) -> FspResult<()> {
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
    fn get_security(&self,_context: &Self::FileContext,security_descriptor: Option<&mut [c_void]>) -> FspResult<u64> {
        sd_from_sddl(SDDL_ALLOW_ALL, security_descriptor)
    }

    /// Overwrite a file.
    fn overwrite(&self,context: &Self::FileContext,_file_attributes: FILE_FLAGS_AND_ATTRIBUTES,_replace_file_attributes: bool,_allocation_size: u64,_extra_buffer: Option<&[u8]>,file_info: &mut FileInfo) -> FspResult<()> {
        let fh = *context;

        // prendi l’entry legata a questo handle
        let mut entry = {
            let map = self.fh_to_entry.lock().map_err(|_| FspError::IO(std::io::ErrorKind::Other))?;
            map.get(&fh).cloned().ok_or(FspError::IO(std::io::ErrorKind::NotFound))?
        };

        if entry.kind == EntryType::Directory {
            return Err(FspError::IO(std::io::ErrorKind::IsADirectory));
        }

        // tronca al size richiesto (di solito 0)
        let attribute=SetAttrRequest{
            size: Some(0),
            perm: None,
            uid: None,
            gid: None,
            flags: None,
        };
        entry=self.backend.lock().expect("Mutex poisoned").set_attr(entry.ino, attribute).map_err(|e| map_error(&e))?;

        self.fh_to_entry.lock().expect("Mutex poisoned").insert(fh, entry.clone());
        entry_to_file_info(file_info, &entry);

        Ok(())
    }

    /// Read directory entries from a directory handle.
    fn read_directory(&self,context: &Self::FileContext,pattern: Option<&U16CStr>,marker: DirMarker,buffer: &mut [u8]) -> FspResult<u32> {

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
    fn rename(&self,context: &Self::FileContext,file_name: &U16CStr,new_file_name: &U16CStr,_replace_if_exists: bool) -> FspResult<()> {
        println!("rename");
        
        let fh = *context;
        let old_path = file_name.to_string_lossy();
        let new_path = new_file_name.to_string_lossy();

        // Get current entry from file handle
        let entry = match self.fh_to_entry.lock().expect("Mutex poisoned").get(&fh) {
            Some(entry) => entry.clone(),
            None => return Err(FspError::IO(ErrorKind::NotFound)),
        };

        // old file path (source)
        let (old_parent_ino, old_filename) = self.get_parent_ino_and_fname(&old_path)?;
        // new file path (destination)
        let (new_parent_ino, new_filename) = self.get_parent_ino_and_fname(&new_path)?;

        let new_entry = self.backend.lock().expect("Mutex poisoned").rename(old_parent_ino, &old_filename, new_parent_ino, &new_filename).map_err(|e|{map_error(&e)})?;

        //println!("Rename successful: new ino={}, new name='{}'", new_entry.ino, new_entry.name);
        self.fh_to_entry.lock().expect("Mutex poisoned").insert(fh, new_entry.clone());
        let mut lookup_cache = self.lookup_ino.lock().expect("Mutex poisoned");
        lookup_cache.retain(|_, &mut ino| ino != entry.ino);
        lookup_cache.insert(new_path.to_string(), new_entry.ino);
        

        Ok(())
    }

    /// Set the file delete flag.
    ///
    /// ## Safety
    /// The file should **never** be deleted in this function. Instead,
    /// set a flag to indicate that the file is to be deleted later by
    /// [`FileSystemContext::cleanup`](crate::filesystem::FileSystemContext::cleanup).
    fn set_delete(&self,context: &Self::FileContext,file_name: &U16CStr,delete_file: bool) -> FspResult<()> {
        println!("set_delete: '{}'", file_name.to_string_lossy());
        let fh = *context;

        // prendi l'entry dall'handle per sapere se è una dir
        let entry = {
            let map = self.fh_to_entry.lock().map_err(|_| FspError::IO(ErrorKind::Other))?;
            map.get(&fh).cloned().ok_or(FspError::IO(ErrorKind::NotFound))?
        };

        if delete_file {
            // se è directory, verifica che sia vuota ORA (fallisci qui, non in cleanup)
            if entry.kind == EntryType::Directory {
                let items = self.backend.lock().expect("Mutex poisoned").list_dir(entry.ino).map_err(|e| map_error(&e))?;
                if !items.is_empty() {
                    return Err(FspError::IO(ErrorKind::DirectoryNotEmpty));
                }
            }

            // segna il path per il delete-on-close
            self.files_to_delete.lock().expect("Mutex poisoned").insert(fh, file_name.to_string_lossy());
        } else {
            // rimuovi il flag
            self.files_to_delete.lock().expect("Mutex poisoned").remove(&fh);
        }
        Ok(())
    }

    /// Set the file or allocation size.
    fn set_file_size(&self,context: &Self::FileContext,new_size: u64,set_allocation_size: bool,file_info: &mut FileInfo) -> FspResult<()> {
        let fh = *context;

        let mut entry = {
            let map = self.fh_to_entry.lock().map_err(|_| FspError::IO(std::io::ErrorKind::Other))?;
            map.get(&fh).cloned().ok_or(FspError::IO(std::io::ErrorKind::NotFound))?
        };

        if entry.kind == EntryType::Directory {
            return Err(FspError::IO(std::io::ErrorKind::IsADirectory));
        }

        // Se è una richiesta di *allocation size*, NON cambiare la dimensione logica del file.
        if set_allocation_size {
            // opzionale: potresti passare un hint di preallocazione al backend qui.
            entry_to_file_info(file_info, &entry);
            return Ok(());
        }

        let attribute=SetAttrRequest{
            size: Some(new_size),
            perm: None,
            uid: None,
            gid: None,
            flags: None,
        };

        entry=self.backend.lock().expect("Mutex").set_attr(entry.ino, attribute).map_err(|e| map_error(&e))?;

        self.fh_to_entry.lock().expect("Mutex").insert(fh, entry.clone());
        entry_to_file_info(file_info, &entry);
        Ok(())
    }

    /// Read from a file. Return the number of bytes read,
    fn read(&self, context: &Self::FileContext, buffer: &mut [u8], offset: u64) -> FspResult<u32> {
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
    fn write(&self,context: &Self::FileContext,buffer: &[u8],offset: u64,write_to_eof: bool,_constrained_io: bool,file_info: &mut FileInfo) -> FspResult<u32> {
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

        // 2) Calcolo l'offset reale (supporto write_to_eof)
        let off = if write_to_eof { entry.size } else { offset };

        // 3) Se non c’è nulla da scrivere, esco subito
        if buffer.is_empty() {
            entry_to_file_info(file_info, &entry);
            return Ok(0);
        }

        // 4) Scrittura immediata al backend (nessun passaggio in write_buffers)
        let ino = entry.ino;
        // NB: LARGE_FILE_SIZE è già definita nel tuo file
        let write_res = if buffer.len() > LARGE_FILE_SIZE as usize {
            self.backend
                .lock()
                .expect("Mutex poisoned")
                .write_stream(ino, off, buffer.to_vec())
        } else {
            self.backend
                .lock()
                .expect("Mutex poisoned")
                .write_chunk(ino, off, buffer.to_vec())
                .map(|_| ()) // uniformo a Result<(), BackendError>
        };

        match write_res {
            Ok(()) => {
                // 5) Aggiorno metadata locali (size/mtime) e rifletto su file_info
                let new_end = off + buffer.len() as u64;
                if new_end > entry.size {
                    entry.size = new_end;
                }
                entry.mtime = SystemTime::now();

                // salvo l'entry aggiornata nella mappa del FH
                self.fh_to_entry
                    .lock()
                    .expect("Mutex poisoned")
                    .insert(fh, entry.clone());

                entry_to_file_info(file_info, &entry);
                Ok(buffer.len() as u32)
            }
            Err(e) => Err(map_error(&e)),
        }

    }

    fn get_volume_info(&self, out_volume_info: &mut VolumeInfo) -> winfsp::Result<()> {        
        println!("get volume info");
        let (total, available)= self.backend.lock().expect("Mutex poisoned").get_size().map_err(|e| {map_error(&e)})?;

        println!("Total size {}, size {}", total, available);
        out_volume_info.total_size = total;
        out_volume_info.free_size =  available;
        
        // Set volume label
        let volume_label = "Remote-FS\0";
        out_volume_info.set_volume_label(volume_label);
        
        Ok(())
    }

}
