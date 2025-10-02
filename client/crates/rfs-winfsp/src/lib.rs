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
use std::ptr;
use windows_permissions::{LocalBox, SecurityDescriptor};
use windows_sys::Win32::Security::GetSecurityDescriptorLength;


const SDDL_ALLOW_ALL: &str = "O:BA G:SY D:(A;;FA;;;WD)";

fn sd_from_sddl(sddl: &str,buf_opt: Option<&mut [std::ffi::c_void]>) -> Result<u64, winfsp::FspError> {
    // Parse SDDL in modo "safe" (gestione memoria automatica su LocalHeap)
    let sd: LocalBox<SecurityDescriptor> = sddl
        .parse()
        .map_err(|_| winfsp::FspError::IO(ErrorKind::InvalidData))?;

    // Puntatore raw al SECURITY_DESCRIPTOR sottostante
    let psd = (&*sd) as *const SecurityDescriptor as *const std::ffi::c_void;

    // Dimensione del SECURITY_DESCRIPTOR in byte
    let len = unsafe { GetSecurityDescriptorLength(psd as *mut std::ffi::c_void) as usize };
    if len == 0 {
        return Err(winfsp::FspError::IO(ErrorKind::InvalidData));
    }

    // Copia opzionale nel buffer del chiamante
    if let Some(dst) = buf_opt {
        let n = len.min(dst.len());
        unsafe {
            ptr::copy_nonoverlapping(psd as *const u8, dst.as_ptr() as *mut u8, n);
        }
    }

    Ok(len as u64)
}

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
    
    file_info.file_size = entry.size as u64;
    file_info.allocation_size = ((entry.size + 511) / 512) * 512;
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
    lookup_ino: Mutex<HashMap<String, u64>>, //tiene riferimento al numero di riferimenti di naming per uno specifico inode, per gestire il caso di lookup multipli

    // file handle management
    next_fh: AtomicU64, // file handle da allocare
    fh_to_entry: Mutex<HashMap<u64, FileEntry>>,
    read_file_handles: Mutex<HashMap<u64, ReadMode>>, // mappa file handle, per gestire read in streaming continuo su file già aperti
    write_buffers: Mutex<HashMap<u64, BTreeMap<u64, Vec<u8>>>>, // buffer di scrittura per ogni file aperto; il valore è la coppia (buffer, offset)

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
}

impl<B: RemoteBackend> FileSystemContext for RemoteFS<B> {
    type FileContext = u64; // file handle

    fn get_security_by_name(
        &self,
        file_name: &winfsp::U16CStr,
        security_descriptor: Option<&mut [std::ffi::c_void]>,
        _reparse_point_resolver: impl FnOnce(&winfsp::U16CStr) -> Option<winfsp::filesystem::FileSecurity>, // symlink managed server-side
    ) -> winfsp::Result<winfsp::filesystem::FileSecurity> {
        let path = file_name.to_string_lossy();
        let (parent_ino, f_name) = self.get_parent_ino_and_fname(&path)?;
        let entry = self.backend.lock().expect("Mutex poisoned")
            .lookup(parent_ino, &f_name).map_err(|e| map_error(&e))?;
        self.lookup_ino.lock().expect("Mutex poisoned").insert(path, entry.ino);

        // SDDL "tutti full control" per test/dev:
        // Owner=Administrators, Group=SYSTEM, DACL: Everyone (WD) Full Access (FA)
        let needed = sd_from_sddl(SDDL_ALLOW_ALL, security_descriptor)?;

        Ok(winfsp::filesystem::FileSecurity {
            reparse: matches!(entry.kind, EntryType::Symlink),
            sz_security_descriptor: needed, // **mai 0**
            attributes: match entry.kind {
                EntryType::Directory => FILE_ATTRIBUTE_DIRECTORY,
                EntryType::File      => FILE_ATTRIBUTE_ARCHIVE,
                EntryType::Symlink   => FILE_ATTRIBUTE_REPARSE_POINT,
            },
        })
    }

    fn open(
        &self,
        file_name: &winfsp::U16CStr,
        _create_options: u32,
        _granted_access: FILE_ACCESS_RIGHTS,
        file_info: &mut winfsp::filesystem::OpenFileInfo,
    ) -> winfsp::Result<Self::FileContext> {
        println!("open");
        let path = file_name.to_string_lossy();
        // lookup
        let ino = match self.lookup_ino.lock().expect("Mutex poisoned").get(&path) {
            Some(&ino) => ino,
            None => return Err(FspError::IO(ErrorKind::NotFound)),
        };
        // getattr
        let entry = match self.backend.lock().expect("Mutex poisoned").get_attr(ino) {
            Ok(e) => e,
            Err(err) => return Err(map_error(&err)),
        };

        
        // updating OpenFileInfo with file's metadata
        let file_info_data = file_info.as_mut();
        entry_to_file_info(file_info_data, &entry);

        let fh = self.next_fh.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        
        self.fh_to_entry.lock().expect("Mutex poisoned").insert(fh, entry.clone());
        
        if entry.kind != EntryType::Directory {
            if entry.size > LARGE_FILE_SIZE {
                self.read_file_handles.lock().expect("Mutex poisoned").insert(fh, ReadMode::LargeStream(StreamState::new(entry.ino)));
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
        println!("cleanup");

        let fh = context;
        
        if self.write_buffers.lock().expect("Mutex posioned").contains_key(fh) {
            if let Err(e) = self.flush_file(*fh) {
                map_error(&e);
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
                        // flushes other files
                    }
                }
                
            }
        }
        
        Ok(())
    }

    fn get_file_info(&self, context: &Self::FileContext, file_info: &mut FileInfo) -> winfsp::Result<()> {
        println!("get_file_info");
        
        let fh = *context;
        
        let entry = {
            let fh_entries = self.fh_to_entry.lock().map_err(|_| FspError::IO(ErrorKind::Other))?;
            match fh_entries.get(&fh) {
                Some(entry) => entry.clone(),
                None => return Err(FspError::IO(ErrorKind::NotFound)),
            }
        };
        
        entry_to_file_info(file_info, &entry);
        
        Ok(())
    }

    /// Get file or directory security descriptor.
    fn get_security(
        &self,
        context: &Self::FileContext,
        security_descriptor: Option<&mut [c_void]>,
    ) -> winfsp::Result<u64> {
        sd_from_sddl(SDDL_ALLOW_ALL, security_descriptor)
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

        println!("read_directory");

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
        file_name: &U16CStr,
        delete_file: bool,
    ) -> winfsp::Result<()> {
        todo!()
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
        todo!()
    }

    /// Write to a file. Return the number of bytes written.
    fn write(
        &self,
        context: &Self::FileContext,
        buffer: &[u8],
        offset: u64,
        write_to_eof: bool,
        constrained_io: bool,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<u32> {
        println!("write");
        todo!()
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
