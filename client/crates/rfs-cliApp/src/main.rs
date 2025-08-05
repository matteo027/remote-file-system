use clap::Parser;
use daemonize::Daemonize;
use fuser::{MountOption};
use rfs_api::Server;
use rfs_fuse::RemoteFS;
use std::{fs::File, sync::{Arc, Condvar, Mutex}};

#[derive(Parser, Debug)]
#[command(name = "Remote-FS", version = "0.1.0")]
struct Cli {
    #[arg(short, long, default_value = "/home/matteo/mnt/remote")]
    mount_point: String,

    #[arg(short, long, default_value = "http://localhost:3000")]
    remote_address: String,
}

fn main() {

    let stdout = File::create("/tmp/remote-fs.out").unwrap();
    let stderr = File::create("/tmp/remote-fs.err").unwrap();

    let daemonize = Daemonize::new()
        .pid_file("/tmp/remote-fs.pid") // saves PID
        .stdout(stdout) // log stdout
        .stderr(stderr) // log stderr
        .working_directory("/")
        .umask(0o027); // file's default permissions

    match daemonize.start() {
        Ok(_) => eprintln!("Remote-FS daemonizzato"),
        Err(e) => eprintln!("Errore nel daemonize: {}", e),
    }

    let cli = Cli::parse();

    let mount_point = cli.mount_point.clone();
    let options = vec![
        MountOption::FSName("Remote-FS".to_string()),
        MountOption::RW,
    ];

    let fs = RemoteFS::new(Server::new());

    let session = fuser::spawn_mount2(fs, &mount_point, &options)
        .expect("Failed to mount FUSE");

    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let pair_clone = pair.clone();

    // Ctrl+C
    ctrlc::set_handler(move || {
        let (lock, cvar) = &*pair_clone;
        let mut stop = lock.lock().unwrap();
        *stop = true;
        cvar.notify_one();
        eprintln!("\nSignal received. Unmounting...");
    })
    .expect("Errore nel set_handler");

    eprintln!("Remote-FS mounted on {}", cli.mount_point);
    eprintln!("Remote address: {}", cli.remote_address);

    // waits for the signal
    let (lock, cvar) = &*pair;
    let mut stop = lock.lock().unwrap();
    stop = cvar.wait_while(stop, |s|{!*s}).expect("Mutex poisoned");

    drop(session);
    eprintln!("Remote-FS unmounted correctly");
}
