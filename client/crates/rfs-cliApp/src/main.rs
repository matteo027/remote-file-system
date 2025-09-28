use clap::Parser;
#[cfg(not(target_os = "windows"))]
use daemonize::Daemonize;
#[cfg(not(target_os = "windows"))]
use fuser::{MountOption};
#[cfg(not(target_os = "windows"))]
use rfs_fuse::RemoteFS;
#[cfg(target_os = "windows")]
use rfs_winfsp::RemoteFS;
#[cfg(target_os = "windows")]
use winfsp::{self, winfsp_init};
#[cfg(target_os = "windows")]
use signal_hook::flag;
use std::{fs::{create_dir_all, File}, sync::{Arc, Condvar, Mutex}};
use rfs_api::HttpBackend;
//use rfs_cache::Cache;

#[cfg(not(target_os = "windows"))]
use signal_hook::{consts::signal::*, iterator::Signals};
#[cfg(target_os = "windows")]
use signal_hook::consts::signal::{SIGINT, SIGTERM};

use std::thread;
use tokio::runtime::Builder;

#[cfg(target_os = "windows")]
const DEFAULT_MOUNT: &str = "C:\\mnt\\remote";
#[cfg(not(target_os = "windows"))]
const DEFAULT_MOUNT: &str = "/home/matteo/mnt/remote";

#[derive(Parser, Debug)]
#[command(name = "Remote-FS", version = "0.1.0")]
struct Cli {
    /// Directory di mount del filesystem remoto in locale
    #[arg(short, long, default_value = DEFAULT_MOUNT)]
    mount_point: String,

    /// Indirizzo del backend remoto
    #[arg(short, long, default_value = "http://fzucca.com:25570")]
    remote_address: String,

    /// Abilita la modalità speed testing (solo Linux e windows)
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

    let mut file_speed: Option<File> = None;
    #[cfg(target_os = "linux")]
    {
        let stdout = File::create("/tmp/remote-fs.log").expect("Failed to create log file");
        let stderr = File::create("/tmp/remote-fs.err").expect("Failed to create error log file");
        if cli.speed_testing {
            println!("Speed testing mode enabled.");
            file_speed = Some(File::create("/tmp/remote-fs.speed-test.out").expect("Failed to create speed test log file"));
        }
        let daemonize = Daemonize::new()
            .pid_file("/tmp/remote-fs.pid") // saves PID
            .stdout(stdout) // log stdout
            .stderr(stderr) // log stderr
            .working_directory("/")
            .umask(0o027); // file's default permission

        daemonize.start().expect("Error, daemonization failed");
    }

    #[cfg(not(target_os = "windows"))]
    let options = vec![MountOption::FSName("Remote-FS".to_string()), MountOption::RW];
    #[cfg(target_os = "windows")]
    let options = vec!["Remote-FS".to_string(), "rw".to_string()];

    let runtime= Arc::new(Builder::new_multi_thread().enable_all().thread_name("rfs-runtime").build().expect("Unable to build a Runtime object"));

    let http_backend;
    match HttpBackend::new(cli.remote_address.clone(), credentials, sessionid, runtime.clone()) {
        Ok(be) => http_backend = be,
        Err(_) => {
            std::process::exit(1);
        }
    }
    //let cache = Cache::new(http_backend, 256, 16, 64, 16); // 256 attr, 16 dir, 64 blocchi per file (da 16 Kb), 16 file
    let fs = RemoteFS::new(http_backend, runtime.clone(), cli.speed_testing, file_speed);
    

    create_dir_all(&cli.mount_point).expect("mount point does not exist and cannot be created");
    
    #[cfg(not(target_os = "windows"))] // linux or macOS
    let session = fuser::spawn_mount2(fs, &cli.mount_point, &options).expect("failed to mount");
    #[cfg(target_os = "windows")]
    let session = {
        use winfsp::host::VolumeParams;
        let mut host = winfsp::host::FileSystemHost::new(VolumeParams::new(), fs).expect("Unable o create a FileSystemHost");
        host.mount(&cli.mount_point).expect("Unable to mount the filesystem");
        host.start()
    };

    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let pair_clone = pair.clone();
    

    // signal handling

    #[cfg(not(target_os = "windows"))]
    {
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
    }

    #[cfg(target_os = "windows")]
    {
        let term = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _th = {
            let term_clone = term.clone();
            flag::register(SIGINT, term_clone).expect("Unable to register SIGINT handler");
            let term_clone2 = term.clone();
            flag::register(SIGTERM, term_clone2).expect("Unable to register SIGTERM handler");
            
            thread::spawn(move || {
                while !term.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                let (lock, cvar) = &*pair_clone;
                let mut stop = lock.lock().unwrap();
                *stop = true;
                cvar.notify_one();
                println!("\nSignal received");
            })
        };
    }

    println!("Remote-FS mounted on {}", cli.mount_point);
    println!("Remote address: {}", cli.remote_address);

    // waits for the signal
    let (lock, cvar) = &*pair;
    let _stop = cvar.wait_while(lock.lock().unwrap(), |s|{!*s}).expect("Mutex poisoned");
    println!("Unmounting Remote-FS...");
    drop(session);
    println!("Remote-FS unmounted correctly");
}
