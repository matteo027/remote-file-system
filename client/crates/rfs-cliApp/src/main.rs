use clap::Parser;
use daemonize::Daemonize;
use fuser::{MountOption};
use rfs_fuse::RemoteFS;
use std::{fs::File, sync::{Arc, Condvar, Mutex}};
use rfs_api::HttpBackend;
use rfs_cache::Cache;
use signal_hook::{consts::signal::*, iterator::Signals};
use std::thread;

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
        Err(e) => eprintln!("Error in daemonize: {}", e),
    }

    let cli = Cli::parse();

    let options = vec![
        MountOption::FSName("Remote-FS".to_string()),
        MountOption::RW,
    ];

    let http_backend = HttpBackend::new();
    let cache = Cache::new(http_backend, 100, 100, 50); // Capacità di cache per attributi, directory e chunk di file
    let fs = RemoteFS::new(cache);
    let session = fuser::spawn_mount2(fs, &cli.mount_point, &options)
        .expect("failed to mount");

    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let pair_clone = pair.clone();

    let mut signals = Signals::new(&[SIGINT, SIGTERM, SIGQUIT, SIGHUP]).expect("Unable to create signals to listen to");
    thread::spawn(move || {
        for signal in signals.forever() {
            match signal {
                SIGINT | SIGTERM | SIGQUIT | SIGHUP => {
                    let (lock, cvar) = &*pair_clone;
                    let mut stop = lock.lock().unwrap();
                    *stop = true;
                    cvar.notify_one();
                    eprintln!("\nSignal received. Unmounting...");
                },
                other => {
                    eprintln!("Signal not hanlded: {}", other);
                }
            }
        }
    });

    eprintln!("Remote-FS mounted on {}", cli.mount_point);
    eprintln!("Remote address: {}", cli.remote_address);

    // waits for the signal
    let (lock, cvar) = &*pair;
    let mut stop = lock.lock().unwrap();
    stop = cvar.wait_while(stop, |s|{!*s}).expect("Mutex poisoned");

    drop(session);
    eprintln!("Remote-FS unmounted correctly");
    eprintln!("Remote-FS mounted at {}", cli.mount_point);
    eprintln!("Remote address: {}", cli.remote_address);

    
    eprintln!("Remote-FS unmounted");
    return;
}
