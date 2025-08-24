use fuser::{FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow};
use rfs_models::{FileEntry, RemoteBackend, SetAttrRequest, BackendError, ByteStream};
use libc::{EAGAIN, EBADF, EILSEQ, EINVAL, ENOENT, ESTALE};
use std::{
    collections::HashMap, ffi::OsStr, fs::File, path::{Path, PathBuf}, sync::Arc, time::{Duration, Instant, SystemTime}
};
use tokio::runtime::Runtime;
use tokio_stream::StreamExt;

const TTL: Duration = Duration::from_secs(1);
const ROOT_INO: u64 = 1;

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

struct StreamState{
    path: String,
    pos: u64,
    buffer: Vec<u8>,
    stream: Option<ByteStream>,
    eof: bool,
}

impl StreamState{
    fn new(path: String)->Self{
        Self{
            path,
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
    next_ino: u64, // inode number da allocare, deve essere coerente solo in locale al client, PER ORA CONTINUA AD INCREMENTARE, con generation si può riutilizzare
    path_to_ino: HashMap<PathBuf, u64>, // mappa path → inode, per ora è inefficiente ricerca al contrario di inode to path, magari mettere altra mappa
    ino_to_path: HashMap<u64, PathBuf>, // mappa inode → path, per risolvere lookup e getattr
    nlookup: HashMap<u64, u64>, //tiene riferimento al numero di riferimenti di naming per uno specifico inode, per gestire il caso di lookup multipli
    next_fh: u64, // file handle da allocare
    file_handles: HashMap<u64, ReadMode>, // mappa file handle, per gestire read in streaming continuo su file già aperti
    speed_testing: bool,
    speed_file: Option<File>,
}

impl<B: RemoteBackend> RemoteFS<B> {
    pub fn new(
        backend: B,
        runtime: Arc<Runtime>,
        speed_testing: bool,
        speed_file: Option<File>,
    ) -> Self {
        Self {
            backend,
            rt: runtime,
            next_ino: ROOT_INO + 1,
            path_to_ino: HashMap::new(),
            ino_to_path: HashMap::new(),
            nlookup: HashMap::new(),
            next_fh: 3, //0,1,2 di solito sono assegnati, da controllare
            file_handles: HashMap::new(),
            speed_testing,
            speed_file,
        }
    }

    fn as_backend_str<'a>(&self, path: &'a Path) -> Result<&'a str, libc::c_int> {
        path.to_str().ok_or(EILSEQ)
    }

    fn get_or_assign_ino(&mut self, path: &Path) -> u64 {
        if let Some(&ino) = self.path_to_ino.get(path) {
            return ino;
        }
        let ino = self.next_ino;
        self.next_ino += 1;
        self.path_to_ino.insert(path.to_path_buf(), ino);
        self.ino_to_path.insert(ino, path.to_path_buf());
        ino
    }

    fn bump_lookup(&mut self, ino: u64) {
        let count = self.nlookup.entry(ino).or_insert(0);
        *count += 1;
    }

    fn inode_to_path(&self, ino: u64) -> Option<PathBuf> {
        self.ino_to_path.get(&ino).cloned()
    }

    fn entry_to_attr(&self, ino: u64, entry: &FileEntry) -> FileAttr {
        FileAttr {
            ino,
            size: entry.size,
            blocks: (entry.size + 511) / 512, // blocchi di 512 byte
            atime: entry.atime,
            mtime: entry.mtime,
            ctime: entry.ctime,
            crtime: entry.btime,
            kind: if entry.is_dir {FileType::Directory} else {FileType::RegularFile},
            perm: entry.perms,
            nlink: entry.nlinks,
            uid: entry.uid,
            gid: entry.gid,
            rdev: 0, // theoretically we could use this for special files, but we don't have any
            flags: 0, // not used in this context, only for macOs
            blksize: 4096, // typical block size for linux filesystems based on ext4
        }
    }
}

impl<B: RemoteBackend> Filesystem for RemoteFS<B> {
    fn init(&mut self,_req: &Request<'_>,_config: &mut fuser::KernelConfig) -> Result<(), libc::c_int> {
        
        let timer_start = Instant::now();
        
        self.path_to_ino.insert(PathBuf::from("/"), ROOT_INO);
        self.ino_to_path.insert(ROOT_INO, PathBuf::from("/"));
        self.nlookup.insert(ROOT_INO, 1); // inizializza il numero di lookup per la root

        if self.speed_testing {
            let duration = timer_start.elapsed();
            println!("[speed] init duration: {:?}", duration);
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] init duration: {:?}", duration).ok();
            }
        }

        Ok(())
    }

    fn destroy(&mut self) {
        let timer_start = Instant::now();
        // pulizia finale, se necessaria
        println!("Fuse layer destroyed.");
        if self.speed_testing {
            let duration = timer_start.elapsed();
            println!("[speed] destroy duration: {:?}", duration);
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] destroy duration: {:?}", duration).ok();
            }
        }
    }

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let timer_start = Instant::now();
        let Some(dir)=self.inode_to_path(parent) else{
            reply.error(ESTALE);
            return;
        };
        let full=dir.join(name);

        let metadata= match self.as_backend_str(&full).and_then(|s|{
            match self.backend.get_attr(s){
                Ok(entry)=>Ok(entry),
                Err(e) => Err(map_error(&e)),
            }
        }) {
            Ok(entry) => entry,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let ino=self.get_or_assign_ino(&full);
        let attr=self.entry_to_attr(ino, &metadata);
        self.bump_lookup(ino); // incrementa il numero di lookup per questo inode
        reply.entry(&TTL, &attr, 0);
        if self.speed_testing {
            let duration = timer_start.elapsed();
            println!("[speed] lookup of {} duration: {:?}", full.display(), duration);
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] lookup of {} duration: {:?}", full.display(), duration).ok();
            }
        }
    }

    fn forget(&mut self, _req: &Request<'_>, ino: u64, nlookup: u64) {
        let timer_start = Instant::now();
        if ino == ROOT_INO {
            // Non dimentichiamo mai la root
            return;
        }

        if let Some(count) = self.nlookup.get_mut(&ino) {
            *count = count.saturating_sub(nlookup);
            if *count == 0 {
                self.nlookup.remove(&ino);
                if let Some(path) = self.ino_to_path.remove(&ino) {
                    self.path_to_ino.remove(&path);
                }
            }
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            println!("[speed] forget of {} duration: {:?}", ino, duration);
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] forget of {} duration: {:?}", ino, duration).ok();
            }
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        let timer_start = Instant::now();
        //fh serve poi quando si fa read/write
        let Some(path) = self.inode_to_path(ino) else {
            reply.error(ENOENT);
            return;
        };
        let path_str= match self.as_backend_str(&path) {
            Ok(s) => s,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        match self.backend.get_attr(path_str) {
            Ok(entry) => {
                let attr = self.entry_to_attr(ino, &entry);
                reply.attr(&TTL, &attr);
            }
            Err(e) => reply.error(map_error(&e)),
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            println!("[speed] getattr of {} duration: {:?}", path_str, duration);
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] getattr of {} duration: {:?}", path_str, duration).ok();
            }
        }
    }

    fn readdir(&mut self,_req: &Request<'_>,ino: u64,_fh: u64,offset: i64,mut reply: ReplyDirectory) {
        let timer_start = Instant::now();

        let Some(dir)=self.inode_to_path(ino) else {
            reply.error(ESTALE);
            return;
        };
        let dir_str = match self.as_backend_str(&dir) {
            Ok(s) => s,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };

        let entries = match self.backend.list_dir(dir_str) {
            Ok(entries) => entries,
            Err(e) => {
                reply.error(map_error(&e));
                return;
            }
        };

        // entries.sort_by(|a, b| a.name.cmp(&b.name)); // ordina le voci per nome

        let mut off = offset;

        if off == 0 {
            let _ = reply.add(ino, 1, FileType::Directory, ".");
            let parent_path = dir.parent().unwrap_or(Path::new("/"));
            let parent_ino = self.get_or_assign_ino(parent_path);
            let _ = reply.add(parent_ino, 2, FileType::Directory, "..");
            off = 2;
        }

        let start = (off - 2).max(0) as usize;
        for (i, entry) in entries.iter().enumerate().skip(start) {
            let full = dir.join(&entry.name);
            let child_ino = self.get_or_assign_ino(&full);
            let kind = if entry.is_dir {
                FileType::Directory
            } else {
                FileType::RegularFile
            };
            // cookie stabile: 3 + index
            if reply.add(child_ino, (i as i64) + 3, kind, &entry.name) {
                break;
            }
        }

        reply.ok();

        if self.speed_testing {
            let duration = timer_start.elapsed();
            println!("[speed] readdir of {} duration: {:?}", dir_str, duration);
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] readdir of {} duration: {:?}", dir_str, duration).ok();
            }
        }
    }

    fn create(&mut self,_req: &Request<'_>, parent: u64,name: &OsStr,_mode: u32,_umask: u32,_flags: i32,reply: ReplyCreate,) {
        let timer_start = Instant::now();

        let Some(dir) = self.inode_to_path(parent) else {
            reply.error(ESTALE);
            return;
        };
        let path = dir.join(name);
        let path_str = match self.as_backend_str(&path) {
            Ok(s) => s,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        match self.backend.create_file(path_str) {
            Ok(entry) => {
                let ino = self.get_or_assign_ino(&path);
                let attr = self.entry_to_attr(ino, &entry);
                self.bump_lookup(ino); // incrementa il numero di lookup per questo inode
                let fh=self.next_fh;
                self.next_fh += 1; // incrementa il file handle per il prossimo file
                self.file_handles.insert(fh, ReadMode::SmallPages); // inizializza il
                reply.created(&TTL, &attr, 0, fh, fuser::consts::FOPEN_DIRECT_IO); // FOPEN_KEEP_CACHE se vuoi mantenere la cache del kernel
            }
            Err(e) => reply.error(map_error(&e)),
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            println!("[speed] create of {} duration: {:?}", path_str, duration);
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] create of {} duration: {:?}", path_str, duration).ok();
            }
        }
    }

    fn mkdir(&mut self,_req: &Request<'_>,parent: u64,name: &OsStr,_mode: u32,_umask: u32,reply: ReplyEntry) {
        let timer_start = Instant::now();
        
        let Some(dir) = self.inode_to_path(parent) else {
            reply.error(ESTALE);
            return;
        };
        let path = dir.join(name);
        let path_str = match self.as_backend_str(&path) {
            Ok(s) => s,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        match self.backend.create_dir(path_str) {
            Ok(entry) => {
                let ino = self.get_or_assign_ino(&path);
                let attr = self.entry_to_attr(ino, &entry);
                self.bump_lookup(ino); // incrementa il numero di lookup per questo inode
                reply.entry(&TTL, &attr, 0);
            }
            Err(e) => reply.error(map_error(&e)),
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            println!("[speed] mkdir of {} duration: {:?}", path_str, duration);
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] mkdir of {} duration: {:?}", path_str, duration).ok();
            }
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let timer_start = Instant::now();
        
        let Some(dir) = self.inode_to_path(parent) else {
            reply.error(ESTALE);
            return;
        };
        let path = dir.join(name);
        let path_str = match self.as_backend_str(&path) {
            Ok(s) => s,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        match self.backend.delete_file(path_str) {
            Ok(_) => {
                if let Some(ino) = self.path_to_ino.remove(&path) {
                    self.ino_to_path.remove(&ino);
                    self.nlookup.remove(&ino); // rimuove il numero di lookup per questo inode
                }
                reply.ok();
            }
            Err(e) => reply.error(map_error(&e)),
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            println!("[speed] unlink of {} duration: {:?}", path_str, duration);
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] unlink of {} duration: {:?}", path_str, duration).ok();
            }
        }
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let timer_start = Instant::now();

        let Some(dir)= self.inode_to_path(parent) else {
            reply.error(ESTALE);
            return;
        };
        let path = dir.join(name);
        let path_str = match self.as_backend_str(&path) {
            Ok(s) => s,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        match self.backend.delete_dir(path_str) {
            Ok(_) => {
                if let Some(ino) = self.path_to_ino.remove(&path) {
                    self.ino_to_path.remove(&ino);
                    self.nlookup.remove(&ino); // rimuove il numero di lookup per questo inode
                }
                reply.ok();
            }
            Err(e) => reply.error(map_error(&e)),
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            println!("[speed] rmdir of {} duration: {:?}", path_str,duration);
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] rmdir of {} duration: {:?}", path_str,duration).ok();
            }
        }
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, _flags: i32, reply: ReplyOpen) {
        let timer_start = Instant::now();

        let Some(path) = self.inode_to_path(ino) else {
            reply.error(ENOENT);
            return;
        };

        let path_str = match self.as_backend_str(&path) {
            Ok(s) => s,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };

        // if (flags & libc::O_TRUNC) != 0 {
        //     let req = SetAttrRequest {
        //         perm: None,
        //         uid: None,
        //         gid: None,
        //         size: Some(0),
        //         flags: None,
        //     };
        //     if let Err(e) = self.backend.set_attr(s, req) {
        //         reply.error(map_error(&e));
        //         return;
        //     }
        // }

        let size = match self.backend.get_attr(&path_str) {
            Ok(entry) => entry.size as u64,
            Err(e) => {
                reply.error(map_error(&e));
                return;
            }
        };
        
        let fuse_flags;
        let mode = if size > LARGE_FILE_SIZE {
            fuse_flags= fuser::consts::FOPEN_DIRECT_IO | FOPEN_NONSEEKABLE; // bisognerebbe abilitare flag stream e nonseekable ma non sono definiti in fuser
            ReadMode::LargeStream(StreamState::new(path_str.to_string()))
        } else {
            fuse_flags= fuser::consts::FOPEN_KEEP_CACHE; // mantiene la cache anche a livello di kernel per file piccoli
            ReadMode::SmallPages
        };
        let fh= self.next_fh;
        self.next_fh += 1;
        self.file_handles.insert(fh, mode);
        // Siccome abbiamo un layer di cache apposito disabilitiamo quello del kernel con direct io
        reply.opened(fh, fuse_flags); //FOPEN_KEEP_CACHE se vogliamo mantenere la cache del kernel

        if self.speed_testing {
            let duration = timer_start.elapsed();
            println!("[speed] open of {} duration: {:?}", path_str, duration);
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] open of {} duration: {:?}", path_str, duration).ok();
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

        let Some(path) = self.inode_to_path(ino) else {
            reply.error(ENOENT);
            return;
        };

        let path_str = match self.as_backend_str(&path) {
            Ok(s) => s,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };

        let Some(mut handle) = self.file_handles.get_mut(&fh) else {
            reply.error(EBADF); // File handle non trovato
            return;
        };
        
        match &mut handle {
            ReadMode::LargeStream(state) => {
                let need= size as usize;
                if state.stream.is_none() && !state.eof {
                    match self.backend.read_stream(&state.path, state.pos) {
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
                match self.backend.read_chunk(&path_str, offset as u64, want) {
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
            println!("[speed] read of {} at offset {} with size {} duration: {:?}", path_str, offset, size, duration);
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] read of {} at offset {} with size {} duration: {:?}", path_str, offset, size, duration).ok();
            }
        }
    }

    fn release(&mut self, _req: &Request<'_>, _ino: u64, fh: u64, _flags: i32, _lock_owner: Option<u64>, _flush: bool, reply: ReplyEmpty) {
        let timer_start = Instant::now();
        // Rimuoviamo il file handle dalla mappa, basta per fare drop automatico della stream e chiuderla immediatamente
        self.file_handles.remove(&fh);
        reply.ok();

        if self.speed_testing {
            let duration = timer_start.elapsed();
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] release of file handle {} duration: {:?}", fh, duration).ok();
            }
        }
    }

    fn write(&mut self,_req: &Request<'_>,ino: u64,_fh: u64,offset: i64,data: &[u8],_write_flags: u32,_flags: i32,_lock_owner: Option<u64>,reply: ReplyWrite,) {
        let timer_start = Instant::now();
        
        let Some(path) = self.inode_to_path(ino) else{
            reply.error(ENOENT);
            return;
        };

        let path_str = match self.as_backend_str(&path) {
            Ok(s) => s,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        match self.backend.write_chunk(path_str, offset as u64, data.to_vec()) {
            Ok(bytes_written) => reply.written(bytes_written as u32),
            Err(e) => reply.error(map_error(&e)),
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            println!("[speed] write of {} at offset {} with size {} duration: {:?}", path_str, offset, data.len(), duration);
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] write of {} at offset {} with size {} duration: {:?}", path_str, offset, data.len(), duration).ok();
            }
        }
    }

    fn rename(&mut self,_req: &Request<'_>,parent: u64,name: &OsStr,new_parent: u64,new_name: &OsStr,_flags: u32,reply: ReplyEmpty,) {
        let timer_start = Instant::now();

        let Some(old_dir)=self.inode_to_path(parent) else {
            reply.error(ESTALE);
            return;
        };
        let Some(new_dir)=self.inode_to_path(new_parent) else {
            reply.error(ESTALE);
            return;
        };
        let old_path = old_dir.join(name);
        let new_path = new_dir.join(new_name);

        let old_path_str = match self.as_backend_str(&old_path) {
            Ok(s) => s,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let new_path_str = match self.as_backend_str(&new_path) {
            Ok(s) => s,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };

        // if (flags & libc::RENAME_NOREPLACE as u32) != 0 {
        //     match self.backend.get_attr(new_s) {
        //         Ok(_) => {
        //             reply.error(libc::EEXIST);
        //             return;
        //         }
        //         Err(BackendError::NotFound(_)) => {}
        //         Err(e) => {
        //             reply.error(map_error(&e));
        //             return;
        //         }
        //     }
        // }

        match self.backend.rename(old_path_str, new_path_str) {
            Ok(_) => {
                if let Some(ino) = self.path_to_ino.remove(&old_path) {
                    self.ino_to_path.remove(&ino);
                    self.path_to_ino.insert(new_path.clone(), ino);
                    self.ino_to_path.insert(ino, new_path.clone());
                }
                reply.ok();
            }
            Err(e) => reply.error(map_error(&e)),
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            println!("[speed] rename from {} to {} duration: {:?}", old_path_str, new_path_str, duration);
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] rename from {} to {} duration: {:?}", old_path_str, new_path_str, duration).ok();
            }
        }
    }

    fn setattr(
        &mut self,
        _req: &Request<'_>,
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

        let Some(path)= self.inode_to_path(ino) else {
            reply.error(ENOENT);
            return;
        };  
        let path_str = match self.as_backend_str(&path) {
            Ok(s) => s,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let perm=mode.map(|m| m & 0o777); // mantiengo solo i permessi, non il setuid/setgid

        let new_set_attr = SetAttrRequest {
            perm,
            uid,
            gid,
            size,
            flags, // flags non sono supportati in questo momento, ancora da implementare
        };

        match self.backend.set_attr(path_str, new_set_attr) {
            Ok(entry) => {
                let attr = self.entry_to_attr(ino, &entry);
                reply.attr(&TTL, &attr);
            }
            Err(e) => reply.error(map_error(&e)),
        }

        if self.speed_testing {
            let duration = timer_start.elapsed();
            println!("[speed] setattr of {} duration: {:?}", path_str, duration);
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] setattr of {} duration: {:?}", path_str, duration).ok();
            }
        }
    }

    fn flush(&mut self,_req: &Request<'_>,_ino: u64,_fh: u64,_lock_owner: u64,reply: ReplyEmpty) {
        let start = Instant::now();
        // Non facciamo nulla di particolare al flush per ora, CONTROLLARE SE SERVE PIÙ AVANTI
        reply.ok();

        if self.speed_testing {
            let duration = start.elapsed();
            println!("[speed] flush duration: {:?}", duration);
            if let Some(file) = self.speed_file.as_mut() {
                use std::io::Write;
                writeln!(file, "[speed] flush duration: {:?}", duration).ok();
            }
        }
    }
}
