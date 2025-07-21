use clap::Parser;
use fuser::MountOption;
use rfs_fuse::RemoteFS;
use rfs_api::StubBackend;
use rfs_models::RemoteBackend;


#[derive(Parser, Debug)]
#[command(name = "Remote-FS", version = "0.1.0")]
struct Cli {
    #[arg(short, long, default_value = "/home/andrea/mnt/remote")]
    mount_point: String,

    #[arg(short, long, default_value = "http://localhost:8080")]
    remote_address: String,
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