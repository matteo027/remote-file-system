use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};
use rfs_models::{FsEntry, FileChunk, BackendError, RemoteBackend, SetAttrRequest};

pub struct StubBackend {
    entries: Arc<Mutex<HashMap<PathBuf, FsEntry>>>,
    data: Arc<Mutex<HashMap<PathBuf, Vec<u8>>>>,
    next_ino: Arc<Mutex<u64>>,
}

impl StubBackend {
    pub fn new() -> Self {
        let mut entries = HashMap::new();
        let root_path = PathBuf::from("/");

        entries.insert(root_path.clone(), FsEntry {
            path: "/".into(),
            name: "/".into(),
            is_dir: true,
            ino: 1,
            size: 0,
            atime: SystemTime::now(),
            mtime: SystemTime::now(),
            ctime: SystemTime::now(),
            perms: 0o755,
            nlinks: 2,
            uid: 0,
            gid: 0,
        });

        // Cartelle
        let dir1 = PathBuf::from("/cartella1");
        let dir2 = PathBuf::from("/cartella2");
        entries.insert(dir1.clone(), FsEntry {
            path: dir1.to_string_lossy().to_string(),
            name: dir1.file_name().unwrap_or_default().to_string_lossy().to_string(),
            is_dir: true,
            ino: 2,
            size: 0,
            atime: SystemTime::now(),
            mtime: SystemTime::now(),
            ctime: SystemTime::now(),
            perms: 0o755,
            nlinks: 2,
            uid: 0,
            gid: 0,
        });
        entries.insert(dir2.clone(), FsEntry {
            path: dir2.to_string_lossy().to_string(),
            name: dir2.file_name().unwrap_or_default().to_string_lossy().to_string(),
            is_dir: true,
            ino: 3,
            size: 0,
            atime: SystemTime::now(),
            mtime: SystemTime::now(),
            ctime: SystemTime::now(),
            perms: 0o755,
            nlinks: 2,
            uid: 0,
            gid: 0,
        });

        // File vuoti
        let file1 = PathBuf::from("/file1.txt");
        let file2 = PathBuf::from("/file2.txt");
        entries.insert(file1.clone(), FsEntry {
            path: file1.to_string_lossy().to_string(),
            name: file1.file_name().unwrap_or_default().to_string_lossy().to_string(),
            is_dir: false,
            ino: 4,
            size: 0,
            atime: SystemTime::now(),
            mtime: SystemTime::now(),
            ctime: SystemTime::now(),
            perms: 0o644,
            nlinks: 1,
            uid: 0,
            gid: 0,
        });
        
        entries.insert(file2.clone(), FsEntry {
            path: file2.to_string_lossy().to_string(),
            name: file2.file_name().unwrap_or_default().to_string_lossy().to_string(),
            is_dir: false,
            ino: 5,
            size: 0,
            atime: SystemTime::now(),
            mtime: SystemTime::now(),
            ctime: SystemTime::now(),
            perms: 0o644,
            nlinks: 1,
            uid: 0,
            gid: 0,
        });

        let mut data = HashMap::new();
        data.insert(file1, Vec::new());
        data.insert(file2, Vec::new());

        StubBackend {
            entries: Arc::new(Mutex::new(entries)),
            data: Arc::new(Mutex::new(data)),
            next_ino: Arc::new(Mutex::new(2)),
        }
    }

    fn allocate_ino(&self) -> u64 {
        let mut ino = self.next_ino.lock().unwrap();
        let val = *ino;
        *ino += 1;
        val
    }

    fn get_parent_dir(path: &Path) -> PathBuf {
        path.parent().unwrap_or(Path::new("/")).to_path_buf()
    }

    fn validate_parent_exists(&self, path: &Path) -> Result<(), BackendError> {
        let entries = self.entries.lock().unwrap();
        let parent = Self::get_parent_dir(path);
        if entries.contains_key(&parent) {
            Ok(())
        } else {
            Err(BackendError::NotFound(format!(
                "Parent directory {:?} not found",
                parent
            )))
        }
    }
}

impl RemoteBackend for StubBackend {
    fn list_dir(&mut self, path: &str) -> Result<Vec<FsEntry>, BackendError> {
        let path = PathBuf::from(path);
        let entries = self.entries.lock().unwrap();

        if let Some(entry) = entries.get(&path) {
            if entry.is_dir {
                let mut result = Vec::new();
                for (p, e) in entries.iter() {
                    if Self::get_parent_dir(p) == path && p != &path {
                        result.push(e.clone());
                    }
                }
                Ok(result)
            } else {
                Err(BackendError::NotFound(format!("{:?} is not a directory", path)))
            }
        } else {
            Err(BackendError::NotFound(format!("Directory {:?} not found", path)))
        }
    }

    fn get_attr(&mut self, path: &str) -> Result<FsEntry, BackendError> {
        let path = PathBuf::from(path);
        let entries = self.entries.lock().unwrap();
        entries.get(&path).cloned().ok_or_else(|| BackendError::NotFound(format!("Path {:?} not found", path)))
    }

    fn create_file(&mut self, path: &str) -> Result<FsEntry, BackendError> {
        let path = PathBuf::from(path);
        self.validate_parent_exists(&path)?;

        let mut entries = self.entries.lock().unwrap();
        if entries.contains_key(&path) {
            return Err(BackendError::Conflict(format!("File {:?} already exists", path)));
        }

        let entry = FsEntry {
            path: path.to_string_lossy().to_string(),
            name: path.file_name().unwrap_or_default().to_string_lossy().to_string(),
            is_dir: false,
            ino: self.allocate_ino(),
            size: 0,
            atime: SystemTime::now(),
            mtime: SystemTime::now(),
            ctime: SystemTime::now(),
            perms: 0o644,
            nlinks: 1,
            uid: 0,
            gid: 0,
        };

        entries.insert(path.clone(), entry.clone());
        self.data.lock().unwrap().insert(path, Vec::new());
        Ok(entry)
    }

    fn create_dir(&mut self, path: &str) -> Result<FsEntry, BackendError> {
        let path = PathBuf::from(path);
        self.validate_parent_exists(&path)?;

        let mut entries = self.entries.lock().unwrap();
        if entries.contains_key(&path) {
            return Err(BackendError::Conflict(format!("Dir {:?} already exists", path)));
        }

        let entry = FsEntry {
            path: path.to_string_lossy().to_string(),
            name: path.file_name().unwrap_or_default().to_string_lossy().to_string(),
            is_dir: true,
            ino: self.allocate_ino(),
            size: 0,
            atime: SystemTime::now(),
            mtime: SystemTime::now(),
            ctime: SystemTime::now(),
            perms: 0o755,
            nlinks: 2,
            uid: 0,
            gid: 0,
        };

        entries.insert(path, entry.clone());
        Ok(entry)
    }

    fn delete_file(&mut self, path: &str) -> Result<(), BackendError> {
        let path = PathBuf::from(path);
        let mut entries = self.entries.lock().unwrap();
        if entries.remove(&path).is_some() {
            self.data.lock().unwrap().remove(&path);
            Ok(())
        } else {
            Err(BackendError::NotFound(format!("File {:?} not found", path)))
        }
    }

    fn delete_dir(&mut self, path: &str) -> Result<(), BackendError> {
        let path = PathBuf::from(path);
        let mut entries = self.entries.lock().unwrap();
        if entries.remove(&path).is_some() {
            Ok(())
        } else {
            Err(BackendError::NotFound(format!("Dir {:?} not found", path)))
        }
    }

    fn read_chunk(&mut self, path: &str, offset: u64, size: u64) -> Result<FileChunk, BackendError> {
        let path = PathBuf::from(path);
        let data = self.data.lock().unwrap();
        if let Some(content) = data.get(&path) {
            let start = offset as usize;
            if start > content.len() {
                return Ok(FileChunk { data: vec![], offset });
            }
            let end = (start + size as usize).min(content.len());
            let chunk = content[start..end].to_vec();
            Ok(FileChunk { data: chunk, offset })
        } else {
            Err(BackendError::NotFound(format!("File {:?} not found", path)))
        }
    }

    fn write_chunk(&mut self, path: &str, offset: u64, data: Vec<u8>) -> Result<u64, BackendError> {
        let path = PathBuf::from(path);
        let mut files = self.data.lock().unwrap();
        let mut entries = self.entries.lock().unwrap();

        if let Some(buf) = files.get_mut(&path) {
            let offset = offset as usize;
            if buf.len() < offset + data.len() {
                buf.resize(offset + data.len(), 0);
            }
            buf[offset..offset + data.len()].copy_from_slice(&data);

            if let Some(entry) = entries.get_mut(&path) {
                entry.size = buf.len() as u64;
                entry.mtime = SystemTime::now();
            }
            Ok(data.len() as u64)
        } else {
            Err(BackendError::NotFound(format!("File {:?} not found", path)))
        }
    }

    fn rename(&mut self, old_path: &str, new_path: &str) -> Result<FsEntry, BackendError> {
        let old_path = PathBuf::from(old_path);
        let new_path = PathBuf::from(new_path);

        let mut entries = self.entries.lock().unwrap();
        let mut data = self.data.lock().unwrap();

        if let Some(mut entry) = entries.remove(&old_path) {
            entry.path = new_path.to_string_lossy().into_owned();

            entries.insert(new_path.clone(), entry.clone());

            if let Some(file_data) = data.remove(&old_path) {
                data.insert(new_path, file_data);
            }
            Ok(entry)
        } else {
            Err(BackendError::NotFound(format!(
                "Path {:?} not found",
                old_path
            )))
        }
    }

    fn set_attr(&mut self, path: &str, attrs: SetAttrRequest) -> Result<FsEntry, BackendError> {
        let path = PathBuf::from(path);
        let mut entries = self.entries.lock().unwrap();
        let mut data = self.data.lock().unwrap();

        let mut entry = if let Some(entry) = entries.get(&path) {
            entry.clone()
        } else {
            return Err(BackendError::NotFound(format!("Path {:?} not found", path)));
        };

        if let Some(m) = attrs.mode {
            entry.perms = (m & 0o777) as u16;
        }
        if let Some(u) = attrs.uid {
            entry.uid = u;
        }
        if let Some(g) = attrs.gid {
            entry.gid = g;
        }
        // Update atime/mtime
        if let Some(at) = attrs.atime {
            entry.atime = at;
        }
        if let Some(mt) = attrs.mtime {
            entry.mtime = mt;
        }
        if let Some(size) = attrs.size {
            let buf = data.entry(path.clone()).or_insert_with(Vec::new);
            let cur = buf.len() as u64;
            if size < cur {
                buf.truncate(size as usize);
            } else if size > cur {
                buf.resize(size as usize, 0);
            }
            entry.size = size;
        }
        entries.insert(path.clone(), entry.clone());
        Ok(entry)
    }
}
