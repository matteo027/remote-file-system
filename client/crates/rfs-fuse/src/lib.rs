use fuser::{FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow};
use rfs_models::{FileEntry, RemoteBackend, SetAttrRequest, BackendError, ByteStream};
use libc::{EBADF, EILSEQ, EINVAL, ENOENT, ESTALE};
use std::{
    ffi::OsStr,
    collections::HashMap,
    sync::Arc,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use tokio::runtime::Runtime;
use tokio_stream::StreamExt;

const TTL: Duration = Duration::from_secs(1);
const ROOT_INO: u64 = 1;

const LARGE_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100 MB

fn map_error(error: &BackendError) -> libc::c_int {
    use libc::{EIO, EACCES, EEXIST, EHOSTUNREACH};
    match error {
        BackendError::NotFound(_) => {
            ENOENT
        },
        BackendError::Unauthorized => {
            eprintln!("Unauthorized error.");
            EACCES
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
            EIO
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
}

impl<B: RemoteBackend> RemoteFS<B> {
    pub fn new(backend: B, runtime: Arc<Runtime>) -> Self {
        Self {
            backend,
            rt: runtime,
            next_ino: ROOT_INO + 1, // il primo inode disponibile è ROOT_INO + 1
            path_to_ino: HashMap::new(),
            ino_to_path: HashMap::new(),
            nlookup: HashMap::new(),
            next_fh: 1, // il primo file handle è 1
            file_handles: HashMap::new(),
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
        self.path_to_ino.insert(PathBuf::from("/"), ROOT_INO);
        self.ino_to_path.insert(ROOT_INO, PathBuf::from("/"));
        self.nlookup.insert(ROOT_INO, 1); // inizializza il numero di lookup per la root
        Ok(())
    }

    fn destroy(&mut self) {
        // pulizia finale, se necessaria
        println!("Fuse layer destroyed.");
    }

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
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
    }

    fn forget(&mut self, _req: &Request<'_>, ino: u64, nlookup: u64) {
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
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
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
    }

    fn readdir(&mut self,_req: &Request<'_>,ino: u64,_fh: u64,offset: i64,mut reply: ReplyDirectory) {
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
    }

    fn create(&mut self,_req: &Request<'_>, parent: u64,name: &OsStr,_mode: u32,_umask: u32,_flags: i32,reply: ReplyCreate,) {
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
    }

    fn mkdir(&mut self,_req: &Request<'_>,parent: u64,name: &OsStr,_mode: u32,_umask: u32,reply: ReplyEntry) {
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
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
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
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
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
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, _flags: i32, reply: ReplyOpen) {
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

        let mode = if size > LARGE_FILE_SIZE {
            ReadMode::LargeStream(StreamState {
                path: path_str.to_string(),
                pos: 0,
                buffer: Vec::new(),
                stream: None,
            })
        } else {
            ReadMode::SmallPages
        };
        let fh= self.next_fh;
        self.next_fh += 1;
        self.file_handles.insert(fh, mode);
        // Siccome abbiamo un layer di cache apposito disabilitiamo quello del kernel con direct io
        reply.opened(fh, fuser::consts::FOPEN_DIRECT_IO); //FOPEN_KEEP_CACHE se vogliamo mantenere la cache del kernel
    }

    fn read(&mut self,_req: &Request<'_>,ino: u64,fh: u64,offset: i64,size: u32,_flags: i32,_lock_owner: Option<u64>,reply: ReplyData,) {
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


        let file_size = match self.backend.get_attr(&path_str) {
            Ok(entry) => entry.size as u64,
            Err(e) => {
                reply.error(map_error(&e));
                return;
            }
        };

        let Some(mut handle) = self.file_handles.get_mut(&fh) else {
            reply.error(EBADF); // File handle non trovato
            return;
        };

        let off = offset as u64;
        if off >= file_size {
            return reply.data(&[]); // Se l'offset è oltre la fine del file, ritorniamo un array vuoto
        }
        let need= (size as u64).min(file_size - off) as usize;
        
        match &mut handle {
            ReadMode::LargeStream(state) => {
                if state.stream.is_none() || state.pos != off {
                    match self.backend.read_stream(&state.path, off) {
                        Ok(stream) => {
                            state.stream = Some(stream);
                            state.buffer.clear(); // Pulisci il buffer per il nuovo stream
                            state.pos = off;
                        }
                        Err(e) => {
                            reply.error(map_error(&e));
                            return;
                        }
                    }
                }

                while state.buffer.len() < need {
                    let s = match state.stream.as_mut() {
                        Some(s) => s,
                        None    => break, // stream assente → consegna quello che hai
                    };
                    let next = self.rt.block_on(async { s.next().await });
                    match next {
                        Some(Ok(bytes)) => state.buffer.extend_from_slice(&bytes),
                        Some(Err(e))    => { reply.error(map_error(&e)); return; }
                        None            => break, // EOF lato server
                    }
                }

                let take = need.min(state.buffer.len());
                let out  = state.buffer.drain(..take).collect::<Vec<u8>>();
                state.pos = state.pos.saturating_add(take as u64);
                reply.data(&out);
            }
            ReadMode::SmallPages => {
                match self.backend.read_chunk(&path_str, off, need as u64) {
                    Ok(mut data) => {
                        if data.len() > need {data.truncate(need);}
                        reply.data(&data);
                    }
                    Err(e) => reply.error(map_error(&e)),
                }
            },
        }
    }

    fn release(&mut self, _req: &Request<'_>, _ino: u64, fh: u64, _flags: i32, _lock_owner: Option<u64>, _flush: bool, reply: ReplyEmpty) {
        // Rimuoviamo il file handle dalla mappa, basta per fare drop automatico della stream e chiuderla immediatamente
        self.file_handles.remove(&fh);
        reply.ok();
    }

    fn write(&mut self,_req: &Request<'_>,ino: u64,_fh: u64,offset: i64,data: &[u8],_write_flags: u32,_flags: i32,_lock_owner: Option<u64>,reply: ReplyWrite,) {
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
    }

    fn rename(&mut self,_req: &Request<'_>,parent: u64,name: &OsStr,new_parent: u64,new_name: &OsStr,_flags: u32,reply: ReplyEmpty,) {
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
                    self.ino_to_path.insert(ino, new_path);
                }
                reply.ok();
            }
            Err(e) => reply.error(map_error(&e)),
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
    }

    fn flush(&mut self,_req: &Request<'_>,_ino: u64,_fh: u64,_lock_owner: u64,reply: ReplyEmpty) {
        // Non facciamo nulla di particolare al flush per ora, CONTROLLARE SE SERVE PIÙ AVANTI
        reply.ok();
    }
}
