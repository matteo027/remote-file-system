use rfs_models::{RemoteBackend,DirectoryEntry, BackendError};
use std::time::SystemTime;
use std::collections::HashMap;

pub struct StubBackend{
    //test purposes
    dirs: HashMap<String,Vec<DirectoryEntry>>,
}

fn now() -> SystemTime {
    SystemTime::now()
}

impl RemoteBackend for StubBackend {
    fn new() -> Self {
        let mut dirs = HashMap::new();
        // la root ("" o "/") contiene file1, file2 e dir1
        dirs.insert("".into(), vec![
        DirectoryEntry::new(2, "file1.txt".into(), false, 1024, 0o644, 1, 0, 0, now(), now(), now()),
        DirectoryEntry::new(3, "file2.txt".into(), false, 2048, 0o644, 1, 0, 0, now(), now(), now()),
        DirectoryEntry::new(4, "dir1".into(), true,    0,    0o755, 1, 0, 0, now(), now(), now()),
        ]);
        // dentro /dir1 c'è dir2
        dirs.insert("/dir1".into(), vec![
        DirectoryEntry::new(5, "dir2".into(), true, 0, 0o755, 1, 0, 0, now(), now(), now()),
        ]);
        // /dir1/dir2 è vuota
        dirs.insert("/dir1/dir2".into(), vec![]);

        StubBackend { dirs }
    }

    fn create_dir(&mut self, mut entry:DirectoryEntry) -> Result<(), BackendError> {
        let full=entry.name.clone();

        let parent=match full.rfind('/') {
            Some(idx) => full[..idx].to_string(),
            _ => "".to_string(),
        };
        let name=full.split('/').last().unwrap().to_string();

        entry.name = name.clone();

        self.dirs.entry(parent).or_default().push(entry);
        self.dirs.entry(full).or_insert_with(Vec::new);
        Ok(())
    }

    fn list_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>, BackendError> {
        let key = if path == "/" { "".to_string() } else { path.to_string() };
        match self.dirs.get(&key) {
        Some(v) => Ok(v.clone()),
        None    => Err(BackendError::NotFound(path.to_string())),
        }
    }
}
