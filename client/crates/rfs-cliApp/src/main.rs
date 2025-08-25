use clap::Parser;
use daemonize::Daemonize;
use fuser::{MountOption};
use rfs_fuse::RemoteFS;
#[cfg(target_os = "macos")]
use rfs_fuse_macos::RemoteFS as RemoteFSMacOS;
use std::{fs::{create_dir_all, File}, sync::{Arc, Condvar, Mutex}};
use rfs_api::HttpBackend;
use rfs_cache::Cache;
use signal_hook::{consts::signal::*, iterator::Signals};
use std::thread;
use tokio::runtime::Builder;

#[derive(Parser, Debug)]
#[command(name = "Remote-FS", version = "0.1.0")]
struct Cli {
    #[arg(short, long, default_value = "/home/matteo/mnt/remote")]
    mount_point: String,

    #[arg(short, long, default_value = "http:/fzucca.com:25570")]
    remote_address: String,

    #[arg(long, action = clap::ArgAction::SetTrue)]
    speed_testing: bool,
}

fn main() {

    let cli = Cli::parse();

    // authentication
    let (credentials, sessionid) = match rfs_api::Credentials::first_authentication(cli.remote_address.clone()) {
        Ok(creds) => creds,
        Err(e) => {
            eprintln!("Error reading credentials: {}", e);
            std::process::exit(1);
        }
    };
    println!("Authentication successful.");

    
    #[cfg(target_os = "linux")]
    {
        let stdout = File::create("/tmp/remote-fs.log").expect("Failed to create log file");
        let stderr = File::create("/tmp/remote-fs.err").expect("Failed to create error log file");
        if cli.speed_testing {
            println!("Speed testing mode enabled.");
            let _speed = File::create("/tmp/remote-fs.speed-test.out").expect("Failed to create speed test log file");
        }
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

    let options = vec![MountOption::FSName("Remote-FS".to_string()),MountOption::RW];

    let runtime= Arc::new(Builder::new_multi_thread().enable_all().thread_name("rfs-runtime").build().expect("Unable to build a Runtime object"));

    let http_backend;
    match HttpBackend::new(cli.remote_address.clone(), credentials, sessionid, runtime.clone()) {
        Ok(be) => http_backend = be,
        Err(_) => {
            std::process::exit(1);
        }
    }
    let cache = Cache::new(http_backend, 256, 16, 64, 16); // 256 attr, 16 dir, 64 blocchi per file (da 16 Kb), 16 file
    let fs;
    #[cfg(target_os = "linux")]
    {
        let speed_file = if cli.speed_testing {
            Some(File::create("/tmp/remote-fs.speed-test.out").expect("Failed to create speed test log file"))
        } else {
            None
        };

        fs = RemoteFS::new(cache, runtime.clone(), cli.speed_testing, speed_file);
    }
    #[cfg(target_os = "macos")]
    {
        fs = RemoteFSMacOS::new(cache);
    }

    create_dir_all(&cli.mount_point).expect("mount point does not exist and cannot be created");
    let session = fuser::spawn_mount2(fs, &cli.mount_point, &options).expect("failed to mount");

    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let pair_clone = pair.clone();

    let mut signals = Signals::new(&[SIGINT, SIGTERM, SIGQUIT, SIGHUP]).expect("Unable to create signals to listen to");
    let th=thread::spawn(move || {
        for signal in signals.forever() {
            match signal {
                SIGINT | SIGTERM | SIGQUIT | SIGHUP => {
                    let (lock, cvar) = &*pair_clone;
                    let mut stop = lock.lock().unwrap();
                    *stop = true;
                    cvar.notify_one();
                    println!("\nSignal received");
                    break;
                },
                other => {
                    eprintln!("Signal not handled: {}", other);
                }
            }
        }
    });

    println!("Remote-FS mounted on {}", cli.mount_point);
    println!("Remote address: {}", cli.remote_address);

    // waits for the signal
    let (lock, cvar) = &*pair;
    let _stop = cvar.wait_while(lock.lock().unwrap(), |s|{!*s}).expect("Mutex poisoned");
    println!("Unmounting Remote-FS...");
    drop(session);
    th.join().expect("Thread join failed");
    println!("Remote-FS unmounted correctly");
}
