use clap::Parser;
use fuser::MountOption;
use rfs_fuse::RemoteFS;
use rfs_models::{RemoteBackend,DirectoryEntry, BackendError};
use std::time::SystemTime;
use std::collections::HashMap;


#[derive(Parser, Debug)]
#[command(name = "Remote-FS", version = "0.1.0")]
struct Cli {
    #[arg(short, long, default_value = "/home/andrea/mnt/remote")]
    mount_point: String,

    #[arg(short, long, default_value = "http://localhost:8080")]
    remote_address: String,
}

struct StubBackend{
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

    fn create_dir(&mut self, path: &str) -> Result<(), BackendError> {
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


fn main() {
    let cli = Cli::parse();
    let options = vec![MountOption::FSName("Remote-FS".to_string()), MountOption::RW];
    fuser::mount2(
        RemoteFS::new(StubBackend::new()),
        cli.mount_point,
        &options,
    ).expect("failed to mount");
}