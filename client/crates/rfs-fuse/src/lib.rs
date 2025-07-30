use fuser::{FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow};
use rfs_models::{FileChunk, FileEntry, RemoteBackend, SetAttrRequest, BackendError};
use std::collections::HashMap;
use libc::ENOENT;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::{Mutex, atomic::{AtomicU64, Ordering}},
    time::{Duration, SystemTime},
};

const TTL: Duration = Duration::from_secs(1);
const ROOT_INO: u64 = 1;

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

pub struct RemoteFS<B: RemoteBackend> {
    backend: B,
    next_ino: AtomicU64, // inode number da allocare, deve essere coerente solo in locale al client
    path_to_ino: Mutex<HashMap<PathBuf, u64>>, // mappa path → inode, per ora è inefficiente ricerca al contrario di inode to path, magari mettere altra mappa
}

impl<B: RemoteBackend> RemoteFS<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            next_ino: AtomicU64::new(ROOT_INO + 1), // il primo inode disponibile è ROOT_INO + 1
            path_to_ino: Mutex::new(HashMap::new()),
        }
    }

    fn get_local_ino(&self, path: &PathBuf) -> u64 {
        if let Some(ino) = self.path_to_ino.lock().unwrap().get(path) {
            return *ino;
        } else {
            let ino = self.next_ino.fetch_add(1, Ordering::Relaxed);
            self.path_to_ino.lock().unwrap().insert(path.clone(), ino);
            return ino;
        }
    }

    fn inode_to_path(&self, ino: u64) -> Option<PathBuf> {
        self.path_to_ino
            .lock()
            .unwrap()
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
            crtime: entry.btime, // crtime is usually the same as ctime, to be checked
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
            .lock()
            .unwrap()
            .insert(PathBuf::from("/"), ROOT_INO);
        match self.backend.list_dir("/") {
            Ok(entries) => {
                for entry in entries {
                    let path = PathBuf::from("/").join(&entry.path);
                    let ino = self.next_ino.fetch_add(1, Ordering::Relaxed);
                    self.path_to_ino.lock().unwrap().insert(path, ino);
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
                    self.path_to_ino.lock().unwrap().insert(full, ino);
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
                        .lock()
                        .unwrap()
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
                self.path_to_ino.lock().unwrap().remove(&path);
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
                self.path_to_ino.lock().unwrap().remove(&path);
                reply.ok();
            }
            Err(e) => reply.error(map_error(&e)),
        }
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        // DA CONTROLLARE
        if let Some(path) = self.inode_to_path(ino) {
            // Se il flag contiene O_TRUNC, tronca il file
            if flags & libc::O_TRUNC != 0 {
                if let Ok(_) = self.backend.get_attr(path.to_str().unwrap()) {
                    let _ = self
                        .backend
                        .write_chunk(path.to_str().unwrap(), 0, Vec::new());
                }
            }
            reply.opened(0, flags as u32);
        } else {
            reply.error(ENOENT);
        }
    }

    fn read(&mut self,_req: &Request<'_>,ino: u64,_fh: u64,offset: i64,size: u32,_flags: i32,_lock_owner: Option<u64>,reply: ReplyData,) {
        if let Some(path) = self.inode_to_path(ino) {
            match self.backend.read_chunk(path.to_str().unwrap(), offset as u64, size as u64) {
                Ok(FileChunk { data, .. }) => reply.data(&data),
                Err(e) => reply.error(map_error(&e)),
            }
        } else {
            reply.error(ENOENT);
        }
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
                let mut map = self.path_to_ino.lock().unwrap();
                if let Some(ino) = map.remove(&old_path) {
                    map.insert(new_path, ino);
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
