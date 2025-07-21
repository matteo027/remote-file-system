use std::{ffi::OsStr, path::{Path,PathBuf}, sync::{Arc,Mutex,atomic::{AtomicU64,Ordering}},time::{Duration,SystemTime}};
use fuser::{Filesystem, Request, ReplyAttr, ReplyEntry, ReplyDirectory, FileAttr, FileType, ReplyEmpty};
use rfs_models::{DirectoryEntry, BackendError, RemoteBackend};
use libc::{ENOENT, EIO, ENOTEMPTY};
use std::collections::HashMap;

const TTL: Duration = Duration::from_secs(1);
const ROOT_PATH: &str = "/";
const ROOT_INO: u64 = 1;


// ino lo tengo solo in local state, non lo chiedo al backend, creo una "cache" in memoria
// per evitare di fare troppe chiamate al backend. Lo stato di questa cache è gestito con programmazione concorrente perchè fuser fa le callback in multithreading (?).
struct State{
    next_ino: AtomicU64,
    path_to_ino: Mutex<HashMap<PathBuf, u64>>,
    ino_to_entries: Mutex<HashMap<u64, DirectoryEntry>>,
}

pub struct RemoteFS<B: RemoteBackend> {
    backend: B,
    state: Arc<State>,
}

impl<B: RemoteBackend> RemoteFS<B> {
    pub fn new(backend: B) -> Self {
        let state = Arc::new(State {
            next_ino: AtomicU64::new(ROOT_INO+1),
            path_to_ino: Mutex::new(HashMap::new()),
            ino_to_entries: Mutex::new(HashMap::new()),
        });
        state.path_to_ino.lock().unwrap().insert(PathBuf::from(ROOT_PATH), ROOT_INO);
        let root_entry = DirectoryEntry::new(
            ROOT_INO,
            "".to_string(),
            true,
            0,
            0o755,
            2,
            0,
            0,
            SystemTime::now(),
            SystemTime::now(),
            SystemTime::now(),
        );
        state.ino_to_entries.lock().unwrap().insert(ROOT_INO, root_entry);
        RemoteFS { backend, state }
    }

    fn get_or_allocate_ino(&self, path: &Path) -> u64 {
        let mut map = self.state.path_to_ino.lock().unwrap();
        if let Some(&ino) = map.get(path) {
            return ino;
        }
        // se non esiste, lo creo
        let ino = self.state.next_ino.fetch_add(1, Ordering::Relaxed);
        map.insert(path.to_path_buf(), ino);
        ino
    }

    fn entry_to_attr(entry: &DirectoryEntry) -> FileAttr {
        FileAttr {
            ino: entry.ino,
            size: entry.size,
            blocks: (entry.size + 4095)/4096,
            atime: entry.atime,
            mtime: entry.mtime,
            ctime: entry.ctime,
            crtime: entry.ctime, // crtime is usually the same as ctime, to be checked
            kind: if entry.is_dir{ FileType::Directory } else { FileType::RegularFile },
            perm: entry.perms as u16,
            nlink: entry.nlinks,
            uid: entry.uid,
            gid: entry.gid,
            rdev: 0, // theoretically we could use this for special files, but we don't have any
            flags: 0, // not used in this context, only for macOs
            blksize: 4096, // typical block size for linux filesystems based on ext4
        }
    }

    fn fetch_dir(&self, path: &Path) -> Result<Vec<DirectoryEntry>, BackendError> {
        let path_str = path.to_str().unwrap_or("/");
        self.backend.list_dir(path_str)
    }

}

impl <B:RemoteBackend> Filesystem for RemoteFS<B> {
    fn init(&mut self, _req: &Request<'_>, _config: &mut fuser::KernelConfig)  -> Result<(), libc::c_int> {
        let entries = self.backend.list_dir(ROOT_PATH).map_err(|_| EIO)?;
        for mut entry in entries {
            let full_path = PathBuf::from(ROOT_PATH).join(&entry.name);
            let ino = self.get_or_allocate_ino(&full_path);
            entry.ino = ino;
            self.state.ino_to_entries.lock().unwrap().insert(ino, entry);
        }
        Ok(())
    }

    fn getattr(&mut self, _req: &Request, ino: u64, _fh: Option<u64>, reply: ReplyAttr) { //fh serve poi quando si fa read/write
        let map = self.state.ino_to_entries.lock().unwrap();
        if let Some(entry) = map.get(&ino) {
            reply.attr(&TTL, &Self::entry_to_attr(entry));
        } else {
            reply.error(ENOENT);
        }
    }

    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let parent_path = {
            let map = self.state.path_to_ino.lock().unwrap();
            map.iter().find_map(|(path, &entry_ino)| if entry_ino == parent { Some(path.clone()) } else { None })
        }.unwrap_or(PathBuf::from(ROOT_PATH));
        let name_path = parent_path.join(name);
    
        match self.fetch_dir(&parent_path) {
            Ok(children) => {
                if let Some(mut entry) = children.into_iter().find(|e| e.name == name.to_string_lossy()) {
                    entry.ino = self.get_or_allocate_ino(&name_path);
                    self.state.ino_to_entries.lock().unwrap().insert(entry.ino, entry.clone());
                    reply.entry(&TTL, &Self::entry_to_attr(&entry), 0);
                } else {
                    reply.error(ENOENT);
                }
            }
            Err(_) => reply.error(EIO),
        }

    }

    fn readdir(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
       let dir_path ={
            let map = self.state.path_to_ino.lock().unwrap();
            map.iter().find_map(|(path, &entry_ino)| if entry_ino == ino { Some(path.clone()) } else { None }).unwrap_or(PathBuf::from(ROOT_PATH))
        };

        match self.fetch_dir(&dir_path) {
            Ok(children) => {
                if offset < 1 { reply.add(ino, 1, FileType::Directory, "."); }
                if offset < 2 {
                    let parent_ino = if dir_path == PathBuf::from(ROOT_PATH) { ROOT_INO }
                        else { *self.state.path_to_ino.lock().unwrap().get(&dir_path.parent().unwrap().to_path_buf()).unwrap() };
                    reply.add(parent_ino, 2, FileType::Directory, "..");
                }
                let start = (offset - 2).max(0) as usize;
                for (i, mut entry) in children.into_iter().enumerate().skip(start) {
                    let full_path = dir_path.join(&entry.name);
                    let ino = self.get_or_allocate_ino(&full_path);
                    entry.ino = ino;
                    self.state.ino_to_entries.lock().unwrap().insert(ino, entry.clone());
                    let cookie = (i as i64) + 3;
                    let kind = if entry.is_dir { FileType::Directory } else { FileType::RegularFile };
                    reply.add(ino, cookie, kind, entry.name);
                }
                reply.ok();
            }
            Err(_) => reply.error(EIO),
        }
    }

    fn mkdir(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, mode: u32, umask:u32, reply: ReplyEntry) {
        let parent_path = {
            let map = self.state.path_to_ino.lock().unwrap();
            map.iter().find_map(|(path, &entry_ino)| if entry_ino == parent { Some(path.clone()) } else { None }).unwrap_or(PathBuf::from(ROOT_PATH))
        };

        let full_path = parent_path.join(name);

        let fmode = mode & !umask; // applico il umask al mode
        let mut new_entry = DirectoryEntry::new(
            0, // ino fittizio, lo setto dopo
            full_path.to_string_lossy().into_owned(),
            true,
            0,
            fmode as u16,
            1, // nlinks
            req.uid(), // uid
            req.gid(), // gid
            SystemTime::now(),
            SystemTime::now(),
            SystemTime::now(),
        );
        
        if let Err(_) = self.backend.create_dir(new_entry.clone()) {
            reply.error(EIO);
            return;
        }

        let ino = self.get_or_allocate_ino(&full_path);
        new_entry.ino = ino;
        self.state.ino_to_entries.lock().unwrap().insert(ino, new_entry.clone());

        let attr = Self::entry_to_attr(&new_entry);
        reply.entry(&TTL, &attr, 0);
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let parent_path = {
            let m = self.state.path_to_ino.lock().unwrap();
            m.iter()
             .find_map(|(p,&i)| if i == parent { Some(p.clone()) } else { None })
             .unwrap_or(PathBuf::from(ROOT_PATH))
        };
        let full_path = parent_path.join(name);
        let path_str = full_path.to_str().unwrap_or("/");

        // 2) verifica se la dir è vuota: fai una list
        match self.backend.list_dir(path_str) {
            Ok(children) if !children.is_empty() => {
                // non vuota → ENOTEMPTY
                return reply.error(ENOTEMPTY);
            }
            Err(BackendError::NotFound(_)) => {
                // non esiste → ENOENT
                return reply.error(ENOENT);
            }
            Err(_) => {
                // altro errore backend → EIO
                return reply.error(EIO);
            }
            Ok(_) => {} // directory vuota → prosegui
        }

        // 3) chiedi al backend di cancellare
        if let Err(_) = self.backend.delete_dir(path_str) {
            // se delete_dir fallisce con NotFound => ENOENT, altrimenti EIO
            return reply.error(ENOENT);
        }

        // 4) aggiorna le mappe locali
        
        // rimuovi entry da ino_to_entries
        let mut i2e = self.state.ino_to_entries.lock().unwrap();
        if let Some(entry) = i2e.remove(&self.get_or_allocate_ino(&full_path)) {
            // rimuovi mapping path→ino
            self.state.path_to_ino.lock().unwrap().remove(&full_path);

            // ed elimina la voce dal vettore dei figli nel genitore
            let mut parent_children = self.state.path_to_ino.lock().unwrap();
            // ma in realtà, se gestisci un cache dei figli, dovrai eliminare entry.name lì
            // ad esempio se hai un map path->vec figli, va fatto qui
        } else {
            return reply.error(ENOENT);
        }
    
        // 5) tutto ok
        reply.ok();
    }
}