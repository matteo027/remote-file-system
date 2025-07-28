use std::{ffi::OsStr, path::{Path,PathBuf}, sync::{Mutex,atomic::{AtomicU64,Ordering}},time::Duration};
use fuser::{Filesystem, Request, ReplyAttr, ReplyEntry, ReplyDirectory, FileAttr, FileType, ReplyEmpty, ReplyCreate, ReplyData, ReplyOpen, ReplyWrite};
use rfs_models::{FsEntry, FileChunk, RemoteBackend};
use libc::{ENOENT, EIO};
use std::collections::HashMap;

const TTL: Duration = Duration::from_secs(1);
const ROOT_INO: u64 = 1;

pub struct RemoteFS<B: RemoteBackend> {
    backend: B,
    next_ino: AtomicU64, // inode number da allocare, deve essere coerente solo in locale al client
    path_to_ino: Mutex<HashMap<PathBuf, u64>>, // mappa path → inode
}

impl<B: RemoteBackend> RemoteFS<B> {
    pub fn new(backend: B) -> Result<Self, i32> {
        let mut fs = RemoteFS {
            backend,
            next_ino: AtomicU64::new(ROOT_INO + 1), // il primo inode disponibile è ROOT_INO + 1
            path_to_ino: Mutex::new(HashMap::new()),
        };
        fs.path_to_ino.lock().unwrap().insert(PathBuf::from("/"), ROOT_INO);
        match fs.backend.list_dir("/") {
            Ok(entries) => {
                for dto in entries {
                    let path = PathBuf::from("/").join(&dto.name);
                    let ino = fs.next_ino.fetch_add(1, Ordering::Relaxed);
                    fs.path_to_ino.lock().unwrap().insert(path, ino);
                }
                Ok(fs)
            }
            Err(_) => Err(EIO),
        }
    }

    fn get_local_ino(&self, path: &PathBuf) -> u64 {
        if let Some(ino) = self.path_to_ino.lock().unwrap().get(path) {
            return *ino;
        }
        else{
            let ino = self.next_ino.fetch_add(1, Ordering::Relaxed);
            self.path_to_ino.lock().unwrap().insert(path.clone(), ino);
            return ino;
        }
    }

    fn inode_to_path(&self, ino: u64) -> Option<PathBuf> {
        self.path_to_ino.lock().unwrap().iter()
            .find_map(|(path, &entry_ino)| if entry_ino == ino { Some(path.clone()) } else { None })
    }

    fn entry_to_attr(&self, ino: u64, entry: &FsEntry) -> FileAttr {
        FileAttr {
            ino,
            size: entry.size,
            blocks: (entry.size + 4095) / 4096, // blocchi di 4096 byte
            atime: entry.atime,
            mtime: entry.mtime,
            ctime: entry.ctime,
            crtime: entry.ctime, // crtime is usually the same as ctime, to be checked
            kind: if entry.is_dir { FileType::Directory } else { FileType::RegularFile },
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

impl <B:RemoteBackend> Filesystem for RemoteFS<B> {
    fn init(&mut self, _req: &Request<'_>, _config: &mut fuser::KernelConfig)  -> Result<(), libc::c_int> {
        // inizializza lo stato del filesystem, ad esempio caricando le directory radice, già fatto in new
        Ok(())
    }

    fn destroy(&mut self) {
        // pulizia finale, se necessaria
        eprintln!("Remote-FS unmounted");
    }

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let dir= self.inode_to_path(parent).unwrap_or_else(|| PathBuf::from("/"));
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
            Err(_) => reply.error(EIO),
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) { //fh serve poi quando si fa read/write
        if let Some(path) = self.inode_to_path(ino) {
            match self.backend.get_attr(path.to_str().unwrap()) {
                Ok(entry) => {
                    let attr = self.entry_to_attr(ino, &entry);
                    reply.attr(&TTL, &attr);
                }
                Err(_) => reply.error(EIO),
            }
        } else {
            reply.error(ENOENT);
        }
    }

    fn readdir(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
       let dir= self.inode_to_path(ino).unwrap_or_else(|| PathBuf::from("/"));

        match self.backend.list_dir(dir.to_str().unwrap()) {
            Ok(entries) => {
                if offset == 0 { reply.add(ino, 1, FileType::Directory, "."); }
                if offset == 1 {
                    let parent = Path::new(&dir).parent().unwrap_or(Path::new("/"));
                    let parent_ino = *self.path_to_ino.lock().unwrap().get(parent).unwrap_or(&ROOT_INO);
                    reply.add(parent_ino, 2, FileType::Directory, "..");
                }
                let start = (offset - 2).max(0) as usize;
                for (i, entry) in entries.iter().enumerate().skip(start) {
                    let full= dir.join(&entry.name);
                    let ino = self.get_local_ino(&full);
                    let kind = if entry.is_dir { FileType::Directory } else { FileType::RegularFile };
                    reply.add(ino, (i as i64) + 3, kind, &entry.name);
                }
                reply.ok();
            }
            Err(_) => reply.error(EIO),
        }
    }

    fn create(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, mode: u32, umask:u32, flags: i32, reply: ReplyCreate) {
        let dir = self.inode_to_path(parent).unwrap_or_else(|| PathBuf::from("/"));
        let path= dir.join(name);
        match self.backend.create_file(path.to_str().unwrap()) {
            Ok(entry) => {
                let ino = self.get_local_ino(&path);
                let attr = self.entry_to_attr(ino, &entry);
                reply.created(&TTL, &attr, 0, 0, flags as u32);
            }
            Err(_) => reply.error(EIO),
        }
    }

    fn mkdir(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, mode: u32, umask:u32, reply: ReplyEntry) {
        let dir = self.inode_to_path(parent).unwrap_or_else(|| PathBuf::from("/"));
        let path = dir.join(name);
        match self.backend.create_dir(path.to_str().unwrap()) {
            Ok(entry) => {
                let ino = self.get_local_ino(&path);
                let attr = self.entry_to_attr(ino, &entry);
                reply.entry(&TTL, &attr, 0);
            }
            Err(_) => reply.error(EIO),
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let dir = self.inode_to_path(parent).unwrap_or_else(|| PathBuf::from("/"));
        let path = dir.join(name);
        if self.backend.delete_file(path.to_str().unwrap()).is_ok() {
            self.path_to_ino.lock().unwrap().remove(&path);
            reply.ok();
        } else {
            reply.error(EIO);
        }
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let dir = self.inode_to_path(parent).unwrap_or_else(|| PathBuf::from("/"));
        let path = dir.join(name);
        if self.backend.delete_dir(path.to_str().unwrap()).is_ok() {
            self.path_to_ino.lock().unwrap().remove(&path);
            reply.ok();
        } else {
            reply.error(EIO);
        }
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        // DA CONTROLLARE
        reply.opened(0, flags as u32); // 0 è il file handle, non usato in questo contesto
    }

    fn read(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, offset: i64, size: u32, _flags: i32, lock_owner: Option<u64>, reply: ReplyData) {
        if let Some(path) = self.inode_to_path(ino) {
            match self.backend.read_chunk(path.to_str().unwrap(), offset as u64, size as u64) {
                Ok(FileChunk { data, .. }) => reply.data(&data),
                Err(_) => reply.error(EIO),
            }
        } else {
            reply.error(ENOENT);
        }
    }

    fn write(&mut self, req: &Request<'_>, ino: u64, _fh:u64, offset: i64, data: &[u8], write_flags: u32, _flags:i32, lock_owner: Option<u64>, reply: ReplyWrite) {
        if let Some(path) = self.inode_to_path(ino) {
           if let Ok(bytes_written) = self.backend.write_chunk(path.to_str().unwrap(), offset as u64, data.to_vec()) {
                reply.written(bytes_written as u32);
            } else {
                reply.error(EIO);
            }
        } else {
            reply.error(ENOENT);
        }
    }
}