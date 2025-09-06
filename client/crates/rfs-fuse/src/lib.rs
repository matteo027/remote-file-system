use fuser::{FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow, consts};
use rfs_models::{FileEntry, RemoteBackend, SetAttrRequest, BackendError, ByteStream, BLOCK_SIZE, EntryType};
use libc::{EAGAIN, EBADF, EILSEQ, EINVAL, ENOENT, ENOSYS, ESTALE, O_ACCMODE, O_RDONLY, O_RDWR, O_WRONLY};
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs::File;
use std::path::{Path};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::runtime::Runtime;
use tokio_stream::StreamExt;

const TTL_FILE: Duration = Duration::from_secs(7);
const TTL_DIR: Duration = Duration::from_secs(3);
const FOPEN_NONSEEKABLE: u32 = 1 << 2; //bit per settare nonseekable flag (controllare meglio abi, non viene codificato in fuser)
const LARGE_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100 MB

fn map_error(error: &BackendError) -> libc::c_int {
    use libc::{EIO, EACCES, EEXIST, EHOSTUNREACH, EPERM, EPROTO};
    match error {
        BackendError::NotFound(_) => {
            ENOENT
        },
        BackendError::Unauthorized => {
            eprintln!("Unauthorized error.");
            EPERM
        },
        BackendError::Forbidden => {
            eprintln!("Forbidden error.");
            EACCES
        },
        BackendError::Conflict(err) => {
            eprintln!("Conflict error: {}", err);
            EEXIST
        },
        BackendError::InternalServerError => {
            eprintln!("Internal server error.");
            EIO
        },
        BackendError::BadAnswerFormat => {
            eprintln!("Bad answer format.");
            EPROTO
        },
        BackendError::ServerUnreachable => {
            eprintln!("Server unreachable.");
            EHOSTUNREACH
        },
        BackendError::Other(err) => {
            eprintln!("Backend error: {}", err);
            EIO
        },
    }
}


#[inline]
fn entry_to_attr(entry: &FileEntry, req: &Request<'_>) -> FileAttr {
    FileAttr {
        ino: entry.ino,
        size: entry.size,
        blocks: (entry.size + 511) / 512, // i blocchi sono di 512 byte come da specifica posix
        atime: entry.atime,
        mtime: entry.mtime,
        ctime: entry.ctime,
        crtime: entry.btime,
        kind: match entry.kind {
            EntryType::File => FileType::RegularFile,
            EntryType::Directory => FileType::Directory,
            EntryType::Symlink => FileType::Symlink,
        },
        perm: entry.perms,
        nlink: entry.nlinks,
        flags:0, // usato per device files, non ci interessa
        rdev:0, // non lo usiamo per ora, serve per mac os?
        blksize:BLOCK_SIZE as u32, // è la dimensione di blocco preferita per le operazioni di I/O, matcha con il layer di cache
        // su macOS usa l’UID/GID della request, altrove quelli dal backend
        uid: if cfg!(target_os = "macos") { req.uid() } else { entry.uid },
        gid: if cfg!(target_os = "macos") { req.gid() } else { entry.gid },
    }
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
    backend: B,
    rt: Arc<Runtime>, // runtime per eseguire le operazioni asincrone

    // inode/path management
    //nlookup: HashMap<u64, u64>, //tiene riferimento al numero di riferimenti di naming per uno specifico inode, per gestire il caso di lookup multipli

    // file handle management
    next_fh: u64, // file handle da allocare
    file_handles: HashMap<u64, ReadMode>, // mappa file handle, per gestire read in streaming continuo su file già aperti
    write_buffers: HashMap<u64, BTreeMap<u64, Vec<u8>>>, // buffer di scrittura per ogni file aperto; il valore è la coppia (buffer, offset)

    // opzioni di testing
    speed_testing: bool,
    speed_file: Option<File>,
}

impl<B: RemoteBackend> RemoteFS<B> {
    pub fn new(backend: B,runtime: Arc<Runtime>,speed_testing: bool,speed_file: Option<File>) -> Self {
        Self {
            backend,
            rt: runtime,
            //nlookup: HashMap::new(),
            next_fh: 3, //0,1,2 di solito sono assegnati, da controllare
            file_handles: HashMap::new(),
            write_buffers: HashMap::new(),
            speed_testing,
            speed_file,
        }
    }


    // #[inline]
    // fn bump_lookup(&mut self, ino: u64) {
    //     let count = self.nlookup.entry(ino).or_insert(0);
    //     *count += 1;
    // }

    fn flush_file(&mut self, fh: u64, ino: u64) -> Result<(), BackendError> {

        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;

        let mut start_offset = None;
        let mut last_offset = None;
        // Collect the map's contents into a vector to avoid double mutable borrow
        let map_entries: Vec<(u64, Vec<u8>)> = {
            let map: &mut BTreeMap<u64, Vec<u8>> = self.write_buffers.get_mut(&fh).unwrap();
            let entries = map.iter().map(|(k, v)| (*k, v.clone())).collect();
            map.clear();
            entries
        };
        
        let mut buffer = Vec::<u8>::new();
        for (off, data) in map_entries.iter() {

            if None == start_offset {
                start_offset = Some(*off);
                last_offset = Some(*off);
            }

            if last_offset.unwrap() + page_size == *off {
                last_offset = Some(*off);
                buffer.extend_from_slice(&data);
            } else {
                // Flush the current buffer
                self.flush_buffer(&mut buffer, ino, start_offset.unwrap())?;

                start_offset = Some(*off);
                last_offset = Some(*off);
                buffer.extend_from_slice(&data);
            }
        }

        // flushing last bytes
        self.flush_buffer(&mut buffer, ino, start_offset.unwrap())?;

        Ok(())
    }

    fn flush_buffer(&mut self, buffer: &mut Vec<u8>, ino: u64, offset: u64) -> Result<(), BackendError> {
        if !buffer.is_empty() {
            if buffer.len() > LARGE_FILE_SIZE as usize {
                self.backend.write_stream(ino, offset, buffer.clone())?
            } else {
                self.backend.write_chunk(ino, offset, buffer.clone())?;
            }
        }
        buffer.clear();
        Ok(())
    }
}

impl<B: RemoteBackend> Filesystem for RemoteFS<B> {
    fn init(&mut self,_req: &Request<'_>,_config: &mut fuser::KernelConfig) -> Result<(), libc::c_int> { 
        Ok(())
    }

    fn destroy(&mut self) {
        // pulizia finale, se necessaria
        println!("Fuse layer destroyed.");
    }

    fn lookup(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let timer_start = Instant::now();

        let metadata=match self.backend.lookup(parent,&name.to_string_lossy()) {
            Ok(entry) => entry,
            Err(e) => {
                reply.error(map_error(&e));
                return;
            }
        };

        let attr=entry_to_attr(&metadata,req);
        reply.entry(&TTL_FILE, &attr, 0);
        if self.speed_testing {
            let duration = timer_start.elapsed();
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] lookup for name {} duration: {:?}", name.to_string_lossy(), duration).ok();
            }
        }
    }

    fn getattr(&mut self, req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        let timer_start = Instant::now();
        //fh serve poi quando si fa read/write
        match self.backend.get_attr(ino) {
            Ok(entry) => {
                let attr = entry_to_attr(&entry, req);
                let ttl= if attr.kind == FileType::Directory { TTL_DIR } else { TTL_FILE };
                reply.attr(&ttl, &attr);
            },
            Err(e) => {
                reply.error(map_error(&e));
                return;
            }
        };
        
        if self.speed_testing {
            let duration = timer_start.elapsed();
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] getattr of ino {} duration: {:?}", ino, duration).ok();
            }
        }
    }

    fn readdir(&mut self,_req: &Request<'_>,ino: u64,_fh: u64,offset: i64,mut reply: ReplyDirectory) {
        let timer_start = Instant::now();

        let entries = match self.backend.list_dir(ino) {
            Ok(entries) => entries,
            Err(e) => {
                reply.error(map_error(&e));
                return;
            }
        };

        // entries.sort_by(|a, b| a.name.cmp(&b.name)); // ordina le voci per nome

        // if off == 0 {
        //     let _ = reply.add(ino, 1, FileType::Directory, ".");
        //     let parent_path = dir.parent().unwrap_or(Path::new("/"));
        //     let parent_ino = self.get_or_assign_ino(parent_path);
        //     let _ = reply.add(parent_ino, 2, FileType::Directory, "..");
        //     off = 2;
        // }

        let start = (offset - 2).max(0) as usize;
        for (i, entry) in entries.iter().enumerate().skip(start) {
            let ftype= match entry.kind {
                EntryType::File => FileType::RegularFile,
                EntryType::Directory => FileType::Directory,
                EntryType::Symlink => FileType::Symlink,
            };
            // cookie stabile: 3 + index
            if reply.add(entry.ino, (i as i64) + 3, ftype, &entry.name) {
                break;
            }
        }

        reply.ok();

        if self.speed_testing {
            let duration = timer_start.elapsed();
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] readdir of ino {} duration: {:?}", ino, duration).ok();
            }
        }
    }

    fn create(&mut self,req: &Request<'_>, parent: u64,name: &OsStr,_mode: u32,_umask: u32,_flags: i32,reply: ReplyCreate,) {
        let timer_start = Instant::now();

        match self.backend.create_file(parent, &name.to_string_lossy()) {
            Ok(entry) => {
                let attr = entry_to_attr(&entry,req);
                let fh=self.next_fh;
                self.write_buffers.insert(fh, BTreeMap::new()); // used for buffering writes
                self.next_fh += 1; // incrementa il file handle per il prossimo file
                self.file_handles.insert(fh, ReadMode::SmallPages); // inizializza il
                reply.created(&TTL_FILE, &attr, 0, fh, fuser::consts::FOPEN_DIRECT_IO); // FOPEN_KEEP_CACHE se vuoi mantenere la cache del kernel
            }
            Err(e) => reply.error(map_error(&e)),
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] create of {} duration: {:?}", name.to_string_lossy(), duration).ok();
            }
        }
    }

    fn mkdir(&mut self,req: &Request<'_>,parent: u64,name: &OsStr,_mode: u32,_umask: u32,reply: ReplyEntry) {
        let timer_start = Instant::now();

        match self.backend.create_dir(parent, &name.to_string_lossy()) {
            Ok(entry) => {
                let attr = entry_to_attr(&entry,req);
                reply.entry(&TTL_DIR, &attr, 0);
            }
            Err(e) => reply.error(map_error(&e)),
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] mkdir of {} duration: {:?}", name.to_string_lossy(), duration).ok();
            }
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let timer_start = Instant::now();

        match self.backend.delete_file(parent, &name.to_string_lossy()) {
            Ok(_) => {
                reply.ok();
            }
            Err(e) => reply.error(map_error(&e)),
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] unlink of {} duration: {:?}", name.to_string_lossy(), duration).ok();
            }
        }
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let timer_start = Instant::now();

        match self.backend.delete_dir(parent, &name.to_string_lossy()) {
            Ok(_) => {
                reply.ok();
            }
            Err(e) => reply.error(map_error(&e)),
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] rmdir of {} duration: {:?}", name.to_string_lossy(), duration).ok();
            }
        }
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        let timer_start = Instant::now();

        if (flags & libc::O_TRUNC) != 0 {
            let req = SetAttrRequest {
                perm: None,
                uid: None,
                gid: None,
                size: Some(0),
                flags: None,
            };
            if let Err(e) = self.backend.set_attr(ino, req) {
                reply.error(map_error(&e));
                return;
            }
        }

        let size = match self.backend.get_attr(ino) {
            Ok(entry) => entry.size as u64,
            Err(e) => {
                reply.error(map_error(&e));
                return;
            }
        };

        let fh = self.next_fh;
        self.next_fh += 1;
        let mut fuse_flags = consts::FOPEN_DIRECT_IO; // default, non usare cache del kernel
        if (flags & O_ACCMODE) == O_RDONLY || (flags & O_ACCMODE) == O_RDWR {
            let (ff, mode) = if size > LARGE_FILE_SIZE {
                (consts::FOPEN_DIRECT_IO | FOPEN_NONSEEKABLE, ReadMode::LargeStream(StreamState::new(ino)))
            } else {
                (consts::FOPEN_KEEP_CACHE, ReadMode::SmallPages)
            };
            fuse_flags = ff;
            self.file_handles.insert(fh, mode);
        }
        if (flags & O_ACCMODE) == O_WRONLY || (flags & O_ACCMODE) == O_RDWR {
            self.write_buffers.insert(fh, BTreeMap::new());
            fuse_flags = consts::FOPEN_DIRECT_IO;
        }
        reply.opened(fh, fuse_flags); 

        if self.speed_testing {
            let duration = timer_start.elapsed();
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] open of ino {} duration: {:?}", ino, duration).ok();
            }
        }
    }

    fn read(&mut self,_req: &Request<'_>,ino: u64,fh: u64,offset: i64,size: u32,flags: i32,_lock_owner: Option<u64>,reply: ReplyData,) {
        let timer_start = Instant::now();

        if size == 0 { //come se avessi letto eof
            reply.data(&[]);
            return;
        }
        
        if offset < 0 {
            reply.error(EINVAL);
            return;
        }

        let Some(mut handle) = self.file_handles.get_mut(&fh) else {
            if self.write_buffers.contains_key(&fh) {
                reply.error(EBADF);
                return;
            }
            reply.error(ENOENT);
            return;
        };
        
        match &mut handle {
            ReadMode::LargeStream(state) => {
                let need= size as usize;
                if offset as u64 != state.pos { 
                    reply.error(libc::ESPIPE); 
                    return; 
                }
                if state.stream.is_none() && !state.eof {
                    match self.backend.read_stream(ino, state.pos) {
                        Ok(stream) => {
                            state.stream = Some(stream);
                            state.buffer.clear(); // Pulisci il buffer per il nuovo stream
                        }
                        Err(e) => {
                            reply.error(map_error(&e));
                            return;
                        }
                    }
                }

                if (flags & libc::O_NONBLOCK) != 0 && state.buffer.is_empty() && !state.eof {
                    reply.error(EAGAIN);
                    return;
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
                        Some(Err(e)) => { reply.error(map_error(&e)); return; }
                        None => { // EOF server side
                            state.eof = true;
                            break;
                        }
                    }
                }

                if state.buffer.is_empty() {
                    if !state.eof  && (flags & libc::O_NONBLOCK) != 0 {
                        reply.error(EAGAIN);
                    }
                    else {
                        reply.data(&[]);
                    }
                    return;
                }

                let take = need.min(state.buffer.len());
                let out:Vec<u8>  = state.buffer.drain(..take).collect();
                state.pos = state.pos.saturating_add(take as u64);
                reply.data(&out);
            }
            ReadMode::SmallPages => {
                let want = size as u64;
                match self.backend.read_chunk(ino, offset as u64, want) {
                    Ok(mut data) => {
                        if data.len() > want as usize {data.truncate(want as usize);}
                        reply.data(&data);
                    }
                    Err(e) => reply.error(map_error(&e)),
                }
            },
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] read of ino {} at offset {} with size {} duration: {:?}", ino, offset, size, duration).ok();
            }
        }
    }

    fn release(&mut self, _req: &Request<'_>, _ino: u64, fh: u64, _flags: i32, _lock_owner: Option<u64>, _flush: bool, reply: ReplyEmpty) {
        // Rimuoviamo il file handle dalla mappa, basta per fare drop automatico della stream e chiuderla immediatamente
        self.file_handles.remove(&fh);
        reply.ok();
    }

    fn write(&mut self,_req: &Request<'_>,ino: u64, fh: u64,offset: i64,data: &[u8],_write_flags: u32,flags: i32,_lock_owner: Option<u64>,reply: ReplyWrite,) {
        let timer_start = Instant::now();

        if !self.write_buffers.contains_key(&fh) {
            if self.file_handles.contains_key(&fh) {
                reply.error(EBADF);
                return;
            }
            reply.error(ENOENT);
            return;
        }

        let mut off= offset as u64;
        if flags & libc::O_APPEND != 0 {
            match self.backend.get_attr(ino) {
                Ok(entry) => off = entry.size,
                Err(e) => {
                    reply.error(map_error(&e));
                    return;
                }
            }
        }

        if self.write_buffers.get(&fh).is_none() {
            reply.error(EBADF); // File handle not found
            return;
        }
        // Scope to limit the mutable borrow of write_buffers
        let buffer = self.write_buffers.get_mut(&fh).unwrap();
        buffer.insert(off, data.to_vec());

        reply.written(data.len() as u32);

        
        if self.speed_testing {
            let duration = timer_start.elapsed();
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] write of ino {} at offset {} with size {} duration: {:?}", ino, offset, data.len(), duration).ok();
            }
        }
    }

    fn rename(&mut self,_req: &Request<'_>,parent: u64,name: &OsStr,new_parent: u64,new_name: &OsStr,_flags: u32,reply: ReplyEmpty,) {
        let timer_start = Instant::now();

        match self.backend.rename(parent, &name.to_string_lossy(), new_parent, &new_name.to_string_lossy()) {
            Ok(_) => {
                reply.ok();
            }
            Err(e) => reply.error(map_error(&e)),
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] rename from {} to {} duration: {:?}", name.to_string_lossy(), new_name.to_string_lossy(), duration).ok();
            }
        }
    }

    fn setattr(
        &mut self,
        req: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        let timer_start = Instant::now();

        let perm=mode.map(|m| m & 0o777); // mantiengo solo i permessi, non il setuid/setgid

        let new_set_attr = SetAttrRequest {
            perm,
            uid,
            gid,
            size,
            flags, // flags non sono supportati in questo momento, ancora da implementare
        };

        match self.backend.set_attr(ino, new_set_attr) {
            Ok(entry) => {
                let attr = entry_to_attr(&entry,req);
                let ttl= if entry.kind == EntryType::Directory {TTL_DIR} else {TTL_FILE};
                reply.attr(&ttl, &attr);
            }
            Err(e) => reply.error(map_error(&e)),
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] setattr of ino {} duration: {:?}", ino, duration).ok();
            }
        }
    }

    // called when a fd closes
    fn flush(&mut self,_req: &Request<'_>, ino: u64, fh: u64,_lock_owner: u64,reply: ReplyEmpty) {

        let timer_start = Instant::now();
        
        if self.write_buffers.contains_key(&fh) {
            match self.flush_file(fh, ino) {
                Ok(_bytes_written) => reply.ok(),
                Err(e) => {
                    reply.error(map_error(&e));
                    return;
                }
            }

            self.write_buffers.remove(&fh); // removes the write buffer associated with this file handle
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] flush of ino {} duration: {:?}", ino, duration).ok();
            }
        }

    }

    // Segnalo come non implementati i metodi relativi a link simbolici e hard link
    fn link(&mut self,_req: &Request<'_>,_ino: u64,_new_parent: u64,_new_name: &OsStr,reply: ReplyEntry) {
        reply.error(ENOSYS);
    }

    fn symlink(&mut self,_req: &Request<'_>,_parent: u64,_name: &OsStr,_link: &Path,reply: ReplyEntry) {
        reply.error(ENOSYS);
    }

    fn readlink(&mut self,_req: &Request<'_>,_ino: u64,reply: fuser::ReplyData) {
        reply.error(ENOSYS);
    }
}
