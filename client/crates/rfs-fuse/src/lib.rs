use fuser::{FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow};
use rfs_models::{FileEntry, RemoteBackend, SetAttrRequest, BackendError};
use std::collections::HashMap;
use libc::ENOENT;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use rfs_models::ByteStream;

const TTL: Duration = Duration::from_secs(1);
const ROOT_INO: u64 = 1;

const LARGE_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100 MB

fn map_error(error: &BackendError) -> libc::c_int {
    use libc::{EIO, EACCES, EEXIST, EHOSTUNREACH};
    match error {
        BackendError::NotFound(_) => ENOENT,
        BackendError::Unauthorized => EACCES,
        BackendError::Conflict(_) => EEXIST,
        BackendError::InternalServerError => EIO,
        BackendError::BadAnswerFormat => EIO,
        BackendError::ServerUnreachable => EHOSTUNREACH,
        BackendError::Other(_) => EIO, // consider other errors as I/O errors
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
    next_ino: u64, // inode number da allocare, deve essere coerente solo in locale al client
    path_to_ino: HashMap<PathBuf, u64>, // mappa path → inode, per ora è inefficiente ricerca al contrario di inode to path, magari mettere altra mappa
    next_fh: u64, // file handle da allocare
    file_handles: HashMap<u64, ReadMode>, // mappa file handle, per gestire read in streaming continuo su file già aperti
}

impl<B: RemoteBackend> RemoteFS<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            next_ino: ROOT_INO + 1, // il primo inode disponibile è ROOT_INO + 1
            path_to_ino: HashMap::new(),
            next_fh: 1, // il primo file handle è 1
            file_handles: HashMap::new(),
        }
    }

    fn get_local_ino(&mut self, path: &PathBuf) -> u64 {
        if let Some(ino) = self.path_to_ino.get(path) {
            return *ino;
        } else {
            let ino = self.next_ino;
            self.path_to_ino.insert(path.clone(), ino);
            self.next_ino += 1;
            return ino;
        }
    }

    fn inode_to_path(&self, ino: u64) -> Option<PathBuf> {
        self.path_to_ino
            .iter()
            .find_map(|(path, &entry_ino)| {
                if entry_ino == ino {
                    Some(path.clone())
                } else {
                    None
                }
            })
    }

    fn entry_to_attr(&self, ino: u64, entry: &FileEntry) -> FileAttr {
        FileAttr {
            ino,
            size: entry.size,
            blocks: (entry.size + 4095) / 4096, // blocchi di 4096 byte
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
        self.path_to_ino
            .insert(PathBuf::from("/"), ROOT_INO);
        match self.backend.list_dir("/") {
            Ok(entries) => {
                for entry in entries {
                    let path = PathBuf::from("/").join(&entry.path);                    
                    self.path_to_ino.insert(path, self.next_ino);
                    self.next_ino += 1;
                }
                Ok(())
            }
            Err(e) => Err(map_error(&e)),
        }
    }

    fn destroy(&mut self) {
        // pulizia finale, se necessaria
        eprintln!("Remote-FS unmounted");
    }

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let dir = self
            .inode_to_path(parent)
            .unwrap_or_else(|| PathBuf::from("/"));
        match self.backend.list_dir(dir.to_str().unwrap()) {
            Ok(entries) => {
                if let Some(entry) = entries.iter().find(|e| e.name == name.to_string_lossy()) {
                    let full = dir.join(&entry.name);
                    let ino = self.get_local_ino(&full);
                    let attr = self.entry_to_attr(ino, entry);
                    self.path_to_ino.insert(full, ino);
                    reply.entry(&TTL, &attr, 0);
                } else {
                    reply.error(ENOENT);
                }
            }
            Err(e) => reply.error(map_error(&e)),
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        //fh serve poi quando si fa read/write
        if let Some(path) = self.inode_to_path(ino) {
            match self.backend.get_attr(path.to_str().unwrap()) {
                Ok(entry) => {
                    let attr = self.entry_to_attr(ino, &entry);
                    reply.attr(&TTL, &attr);
                }
                Err(e) => reply.error(map_error(&e)),
            }
        } else {
            reply.error(ENOENT);
        }
    }

    fn readdir(&mut self,_req: &Request<'_>,ino: u64,_fh: u64,offset: i64,mut reply: ReplyDirectory) {
        let dir = self
            .inode_to_path(ino)
            .unwrap_or_else(|| PathBuf::from("/"));

        match self.backend.list_dir(dir.to_str().unwrap()) {
            Ok(entries) => {
                if offset == 0 {
                    let _ = reply.add(ino, 1, FileType::Directory, ".");
                }
                if offset == 1 {
                    let parent = Path::new(&dir).parent().unwrap_or(Path::new("/"));
                    let parent_ino = *self
                        .path_to_ino
                        .get(parent)
                        .unwrap_or(&ROOT_INO);
                    let _ = reply.add(parent_ino, 2, FileType::Directory, "..");
                }
                let start = (offset - 2).max(0) as usize;
                for (i, entry) in entries.iter().enumerate().skip(start) {
                    let full = dir.join(&entry.name);
                    let ino = self.get_local_ino(&full);
                    let kind = if entry.is_dir {
                        FileType::Directory
                    } else {
                        FileType::RegularFile
                    };
                    let _ = reply.add(ino, (i as i64) + 3, kind, &entry.name);
                }
                reply.ok();
            }
            Err(e) => reply.error(map_error(&e)),
        }
    }

    fn create(&mut self,_req: &Request<'_>, parent: u64,name: &OsStr,_mode: u32,_umask: u32,flags: i32,reply: ReplyCreate,) {
        let dir = self
            .inode_to_path(parent)
            .unwrap_or_else(|| PathBuf::from("/"));
        let path = dir.join(name);
        match self.backend.create_file(path.to_str().unwrap()) {
            Ok(entry) => {
                let ino = self.get_local_ino(&path);
                let attr = self.entry_to_attr(ino, &entry);
                reply.created(&TTL, &attr, 0, 0, flags as u32);
            }
            Err(e) => reply.error(map_error(&e)),
        }
    }

    fn mkdir(&mut self,_req: &Request<'_>,parent: u64,name: &OsStr,_mode: u32,_umask: u32,reply: ReplyEntry) {
        let dir = self
            .inode_to_path(parent)
            .unwrap_or_else(|| PathBuf::from("/"));
        let path = dir.join(name);
        match self.backend.create_dir(path.to_str().unwrap()) {
            Ok(entry) => {
                let ino = self.get_local_ino(&path);
                let attr = self.entry_to_attr(ino, &entry);
                reply.entry(&TTL, &attr, 0);
            }
            Err(e) => reply.error(map_error(&e)),
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let dir = self
            .inode_to_path(parent)
            .unwrap_or_else(|| PathBuf::from("/"));
        let path = dir.join(name);
        match self.backend.delete_file(path.to_str().unwrap()) {
            Ok(_) => {
                self.path_to_ino.remove(&path);
                reply.ok();
            }
            Err(e) => reply.error(map_error(&e)),
        }
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let dir = self
            .inode_to_path(parent)
            .unwrap_or_else(|| PathBuf::from("/"));
        let path = dir.join(name);
        match self.backend.delete_dir(path.to_str().unwrap()) {
            Ok(_) => {
                self.path_to_ino.remove(&path);
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

        let size = match self.backend.get_attr(path.to_str().unwrap()) {
            Ok(entry) => entry.size as u64,
            Err(e) => {
                reply.error(map_error(&e));
                return;
            }
        };

        let mode = if size > LARGE_FILE_SIZE {
            ReadMode::LargeStream(StreamState {
                path: path.to_str().unwrap().to_string(),
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
        if offset < 0 {return reply.error(libc::EINVAL);}
        let Some(path) = self.inode_to_path(ino) else {
            return reply.error(ENOENT);
        };


        let file_size = match self.backend.get_attr(path.to_str().unwrap()) {
            Ok(entry) => entry.size as u64,
            Err(e) => {
                return reply.error(map_error(&e));
            }
        };

        let off = offset as u64;
        let need= (size as u64).min(file_size - off) as usize;
        if off >= file_size || need == 0 {
            return reply.data(&[]); // Se l'offset è oltre la fine del file, ritorniamo un array vuoto
        }

        match self.file_handles.get_mut(&fh) {
            Some(ReadMode::LargeStream(state)) => {
                if state.stream.is_none() || state.pos != off{
                    match self.backend.read_stream(&state.path, off) {
                        Ok(stream) => {
                            state.stream = Some(stream);
                            state.buffer.clear(); // Pulisci il buffer per il nuovo stream
                            state.pos = off;
                        }
                        Err(e) => return reply.error(map_error(&e)),
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
            Some(ReadMode::SmallPages) => {
                match self.backend.read_chunk(path.to_str().unwrap(), off, need as u64) {
                    Ok(mut data) => {
                        if data.len() > need {data.truncate(need);}
                        reply.data(&data);
                    }
                    Err(e) => return reply.error(map_error(&e)),
                }
            }
            None => {
                return reply.error(libc::EBADF); // File handle non trovato
            }
        }

    }

    fn release(&mut self, _req: &Request<'_>, _ino: u64, fh: u64, _flags: i32, _lock_owner: Option<u64>, _flush: bool, reply: ReplyEmpty) {
        // Rimuoviamo il file handle dalla mappa, basta per fare drop automatico della stream e chiuderla immediatamente
        self.file_handles.remove(&fh);
        reply.ok();
    }

    fn write(&mut self,_req: &Request<'_>,ino: u64,_fh: u64,offset: i64,data: &[u8],_write_flags: u32,_flags: i32,_lock_owner: Option<u64>,reply: ReplyWrite,) {
        if let Some(path) = self.inode_to_path(ino) {
            match self.backend.write_chunk(path.to_str().unwrap(), offset as u64, data.to_vec()) {
                Ok(bytes_written) => reply.written(bytes_written as u32),
                Err(e) => reply.error(map_error(&e)),
            }
        } else {
            reply.error(ENOENT);
        }
    }

    fn rename(&mut self,_req: &Request<'_>,parent: u64,name: &OsStr,new_parent: u64,new_name: &OsStr,_flags: u32,reply: ReplyEmpty,) {
        let old_dir = self
            .inode_to_path(parent)
            .unwrap_or_else(|| PathBuf::from("/"));
        let old_path = old_dir.join(name);

        // Recupero il nuovo path
        let new_dir = self
            .inode_to_path(new_parent)
            .unwrap_or_else(|| PathBuf::from("/"));
        let new_path = new_dir.join(new_name);

        match self.backend.rename(old_path.to_str().unwrap(), new_path.to_str().unwrap()){
            Ok(_) => {
                if let Some(ino) = self.path_to_ino.remove(&old_path) {
                    self.path_to_ino.insert(new_path, ino);
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
        let path = match self.inode_to_path(ino) {
            Some(p) => p,
            None => {
                return reply.error(ENOENT);
            }
        };

        let perm = if let Some(m) = mode {Some(m & 0o777)} else {None};

        let new_set_attr = SetAttrRequest {
            perm,
            uid,
            gid,
            size,
            flags, // flags non sono supportati in questo momento, ancora da implementare
        };

        match self.backend.set_attr(path.to_str().unwrap(), new_set_attr) {
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
