use clap::Parser;
use fuser::MountOption;
use rfs_api::HttpBackend;
use rfs_fuse::RemoteFS;
use rfs_cache::Cache;

#[derive(Parser, Debug)]
#[command(name = "Remote-FS", version = "0.1.0")]
struct Cli {
    #[arg(short, long, default_value = "/home/andrea/mnt/remote")]
    mount_point: String,

    #[arg(short, long, default_value = "http://localhost:3000")]
    remote_address: String,
}

fn main() {
    let cli = Cli::parse();
    let options = vec![
        MountOption::FSName("Remote-FS".to_string()),
        MountOption::RW,
    ];

    eprintln!("Remote-FS mounted at {}", cli.mount_point);
    eprintln!("Remote address: {}", cli.remote_address);

    let http_backend = HttpBackend::new();
    let cache = Cache::new(http_backend, 100, 100, 50); // Capacità di cache per attributi, directory e chunk di file
    fuser::mount2(RemoteFS::new(cache), cli.mount_point, &options)
        .expect("failed to mount");
    eprintln!("Remote-FS unmounted");
    return;
}
