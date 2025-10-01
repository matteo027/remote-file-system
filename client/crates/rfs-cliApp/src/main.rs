use clap::Parser;
use rfs_api::HttpBackend;
use std::sync::{Arc, Mutex, Condvar};
use tokio::runtime::Builder;
use signal_hook::{consts::*};
use std::thread;
use std::fs::File;

#[cfg(target_os = "linux")]
use daemonize::Daemonize;
#[cfg(unix)]
use fuser::MountOption;
#[cfg(unix)]
use rfs_fuse::RemoteFS;
//#[cfg(unix)]
//se rfs_cache::Cache;
#[cfg(unix)]
use signal_hook::iterator::Signals;

#[cfg(target_os = "windows")]
use rfs_winfsp::RemoteFS;
#[cfg(target_os = "windows")]
use winfsp::host::{FileSystemHost, VolumeParams};

// ---------- Costanti OS-specifiche ----------
#[cfg(target_os = "windows")]
const DEFAULT_MOUNT: &str = "X:";
#[cfg(unix)]
const DEFAULT_MOUNT: &str = "/home/matteo/mnt/remote";

#[derive(Parser, Debug)]
#[command(name = "Remote-FS", version = "0.1.0")]
struct Cli {
    /// Directory di mount del filesystem remoto in locale
    #[arg(short, long, default_value = DEFAULT_MOUNT)]
    mount_point: String,

    /// Indirizzo del backend remoto
    #[arg(short, long, default_value = "http://localhost:3000")]
    remote_address: String,

    /// Abilita la modalità speed testing (solo Linux e windows)
    #[arg(long, action = clap::ArgAction::SetTrue)]
    speed_testing: bool,
}

fn main() {

    // su windows settare:
    // $env:PATH += ";C:\Program Files (x86)\WinFsp\bin"

    let cli = Cli::parse();

    // authentication
    let (credentials, sessionid) = match rfs_api::Credentials::first_authentication(cli.remote_address.clone()) {
        Ok(creds) =>{
            println!("Authentication successful.");
            creds
        } ,
        Err(e) => {
            eprintln!("Error reading credentials: {}", e);
            panic!("Cannot continue without credentials");
        }
    };
    
    // --- Logging + daemonize (solo Linux) ---
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

    let runtime= Arc::new(Builder::new_multi_thread().enable_all().thread_name("rfs-runtime").build().expect("Unable to build a Runtime object"));

    let http_backend= match HttpBackend::new(cli.remote_address.clone(), credentials, sessionid, runtime.clone()) {
        Ok(be) => be,
        Err(_) => panic!("Cannot create the HTTP backend"),
    };
    
    #[cfg(unix)]{
        //let cache = Cache::new(http_backend, 256, 16, 64, 16); // 256 attr, 16 dir, 64 blocchi per file (da 16 Kb), 16 file
        let fs = RemoteFS::new(http_backend, runtime.clone(), cli.speed_testing, file_speed);
        let options = vec![MountOption::FSName("Remote-FS".to_string()), MountOption::RW];
        fuser::spawn_mount2(fs, &cli.mount_point, &options).expect("failed to mount");
    }

    #[cfg(target_os = "windows")]{
        let fs = RemoteFS::new(http_backend, runtime.clone(), cli.speed_testing, file_speed);
        let mut host = FileSystemHost::new(VolumeParams::new(), fs).expect("Unable o create a FileSystemHost");
        host.mount(&cli.mount_point).expect("Unable to mount the filesystem");
        host.start().expect("Unable to start the filesystem host");
    }

    println!("Remote-FS mounted on {}", cli.mount_point);
    println!("Remote address: {}", cli.remote_address);
    
    // signal handling
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let pair_clone = pair.clone();

    #[cfg(unix)]
    {
        thread::spawn(move || {
            let mut signals = Signals::new(&[SIGINT, SIGTERM, SIGQUIT, SIGHUP]).expect("Unable to create signals to listen to");
            for signal in signals.forever() {
                match signal {
                    SIGINT | SIGTERM | SIGQUIT | SIGHUP => {
                        let (lock, cvar) = &*pair_clone;
                        if let Ok(mut stop) = lock.lock() {
                            *stop = true;
                            cvar.notify_one();
                        }
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
        use std::sync::atomic::{AtomicBool, Ordering};

        use signal_hook::flag;
        let term = Arc::new(AtomicBool::new(false));

        flag::register(SIGINT, term.clone()).expect("register SIGINT");
        flag::register(SIGTERM, term.clone()).expect("register SIGTERM");

        let term_clone = term.clone();
        thread::spawn(move || {
            // Polling leggero dell’AtomicBool
            while !term_clone.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let (lock, cvar) = &*pair_clone;
            if let Ok(mut stop) = lock.lock() {
                *stop = true;
                cvar.notify_one();
            }
            eprintln!("\nSignal received (Windows)");
        });
    }

    println!("Remote-FS mounted on {}", cli.mount_point);
    println!("Remote address: {}", cli.remote_address);

    // waits for the signal
    let (lock, cvar) = &*pair;
    let _stop = cvar.wait_while(lock.lock().unwrap(), |s|{!*s}).expect("Mutex poisoned");
    println!("Unmounting Remote-FS...");
    println!("Remote-FS unmounted correctly");
}
