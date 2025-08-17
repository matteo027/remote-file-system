use clap::Parser;
use daemonize::Daemonize;
use fuser::{MountOption};
use rfs_fuse::RemoteFS;
use rfs_fuse_macos::RemoteFS as RemoteFSMacOS;
use std::{fs::File, sync::{Arc, Condvar, Mutex}};
use rfs_api::HttpBackend;
use rfs_cache::Cache;
use signal_hook::{consts::signal::*, iterator::Signals};
use std::thread;

#[derive(Parser, Debug)]
#[command(name = "Remote-FS", version = "0.1.0")]
struct Cli {
    #[arg(short, long, default_value = "/Users/matteo/mnt/remote")]
    mount_point: String,

    #[arg(short, long, default_value = "https://educational-shannen-politecnico-di-torino-b6588608.koyeb.app")]
    remote_address: String,
}

fn main() {

    let cli = Cli::parse();
    // authentication (actually saving cookies)
    if let Err(e) = HttpBackend::new(cli.remote_address.clone(), true) {
        eprintln!("Unable to get the authhenticate: {}", e);
        std::process::exit(1);
    }

    #[cfg(target_os = "linux")]
    {
        let stdout = File::create("/tmp/remote-fs.out").unwrap();
        let stderr = File::create("/tmp/remote-fs.err").unwrap();
        let daemonize = Daemonize::new()
            .pid_file("/tmp/remote-fs.pid") // saves PID
            .stdout(stdout) // log stdout
            .stderr(stderr) // log stderr
            .working_directory("/")
            .umask(0o027); // file's default permissions
        match daemonize.start() {
            Ok(_) => {},
            Err(e) => {
            eprintln!("Error in daemonize: {}", e);
            std::process::exit(1);
            }
        }
    }

    let options = vec![
        MountOption::FSName("Remote-FS".to_string()),
        MountOption::RW,
        MountOption::AllowOther, // <--- aggiungi questa
    ];

    // real backend: reads previously saved cookies
    let http_backend;
    match HttpBackend::new(cli.remote_address.clone(), false) {
        Ok(be) => http_backend = be,
        Err(_) => {
            eprintln!("Unable to get the cookies from /tmp/rfs-token");
            std::process::exit(1);
        }
    }
    let cache = Cache::new(http_backend, 100, 100, 50); // Capacità di cache per attributi, directory e chunk di file
    let fs;
    #[cfg(target_os = "linux")]
    {
        fs = RemoteFS::new(cache);
    }
    #[cfg(target_os = "macos")]
    {
        fs = RemoteFSMacOS::new(cache);
    }
     
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

    return;
}
