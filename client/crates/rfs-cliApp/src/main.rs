use clap::Parser;
use fuser::{MountOption};
use rfs_fuse::RemoteFS;
use rfs_api::{Server};
use rfs_models::RemoteBackend;


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
    let options = vec![MountOption::FSName("Remote-FS".to_string()), MountOption::RW];
    eprintln!("Remote-FS mounted at {}", cli.mount_point);
    eprintln!("Remote address: {}", cli.remote_address);
    fuser::mount2(RemoteFS::new(Server::new()).expect("failed to create RemoteFS"), cli.mount_point, &options).expect("failed to mount");
    eprintln!("Remote-FS unmounted");
    return;
}