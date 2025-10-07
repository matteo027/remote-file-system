use clap::{Parser,ArgAction};
use rfs_api::{HttpBackend,Credentials};
use std::sync::Arc;
use tokio::runtime::{Builder,Runtime};

// ---------- Costanti OS-specifiche ----------
#[cfg(target_os = "linux")]
const DEFAULT_MOUNT: &str = "/home/andrea/mnt/remote";
#[cfg(target_os = "macos")]
const DEFAULT_MOUNT: &str = "/Volumes/Remote-FS"; //?DA CONTROLLARE
#[cfg(target_os = "windows")]
const DEFAULT_MOUNT: &str = "X:";

#[derive(Parser, Debug)]
#[command(name = "Remote-FS", version = "0.1.0")]
struct Cli {
    /// Directory di mount del filesystem remoto in locale
    #[arg(short, long, default_value = DEFAULT_MOUNT)]
    mount_point: String,

    /// Indirizzo del backend remoto
    #[arg(short, long, default_value = "http://fzucca.com:25570")]  //"http://fzucca.com:25570"
    remote_address: String,

    /// Abilita la modalità speed testing (solo Unix)
    #[arg(long, action = ArgAction::SetTrue)]
    speed_testing: bool,
}

// su windows settare:
// $env:PATH += ";C:\Program Files (x86)\WinFsp\bin"

fn main(){
    let cli = Cli::parse();

    // first authentication
    let (credentials, sessionid) = match Credentials::first_authentication(&cli.remote_address) {
        Ok(creds) =>{
            println!("Authentication successful. Welcome!");
            creds
        } ,
        Err(e) => {
            eprintln!("Error authenticating: {}", e);
            eprintln!("Exiting...");
            return;
        }
    };

    #[cfg(target_os = "linux")]
    {
        if let Err(e) = demonize() {
            eprintln!("{}", e);
            eprintln!("Exiting...");
            return;
        }
    }

    let runtime= Arc::new(Builder::new_multi_thread().enable_all().thread_name("rfs-runtime").build().expect("Unable to build a Runtime object"));
    let http_backend= HttpBackend::new(cli.remote_address.clone(), credentials, sessionid, runtime.clone()).expect("Cannot create the HTTP backend");

    #[cfg(unix)]
    run_unix(cli, http_backend, runtime);
    #[cfg(target_os = "windows")]
    run_windows(cli, http_backend, runtime);
}

#[cfg(target_os = "linux")]
fn demonize() -> Result<(), String>{
    use std::fs::File;
    use daemonize::Daemonize;

    const PID_FILE :&str = "/tmp/remote-fs.pid";
    if std::path::Path::new(PID_FILE).exists() {
        if let Ok(pid_content) = std::fs::read_to_string(PID_FILE) {
            if let Ok(pid) = pid_content.trim().parse::<u32>() {
                let proc_path = format!("/proc/{}", pid);
                if std::path::Path::new(&proc_path).exists() {
                    return Err(format!("Remote-FS daemon is already running with PID: {}\nTo stop it, run: kill {}", pid, pid));
                } else {
                    let _ = std::fs::remove_file(PID_FILE);
                }
            }
        }
    }

    let stdout = File::create("/tmp/remote-fs.log").expect("Failed to create log file");
    let stderr = File::create("/tmp/remote-fs.err").expect("Failed to create error log file");
    let daemonize = Daemonize::new()
        .pid_file(PID_FILE) // saves PID
        .stdout(stdout) // log stdout
        .stderr(stderr) // log stderr
        .working_directory("/")
        .umask(0o027); // file's default permission
    println!("Starting Remote-FS daemon... Check /tmp/remote-fs.log and /tmp/remote-fs.err for output.");
    daemonize.start().expect("Failed to daemonize the process");
    Ok(())
}

#[cfg(unix)]
fn run_unix(cli: Cli, http_backend: HttpBackend, runtime: Arc<Runtime>){
    use fuser::{MountOption,Session};
    use std::fs::File;
    use rfs_fuse::RemoteFS;
    use signal_hook::consts::*;
    use signal_hook::iterator::Signals;
    use std::thread;
    use rfs_cache::Cache;

    let file_speed= if cli.speed_testing {
        println!("Speed testing mode enabled.");
        Some(File::create("/tmp/remote-fs.speed-test.out").expect("Failed to create speed test log file"))
    }else{
        None
    };

    let cache = Cache::new(http_backend, 256, 16, 64, 16); // 256 attr, 16 dir, 64 blocchi per file (da 16 Kb), 16 file
    let fs = RemoteFS::new(cache, runtime.clone(), cli.speed_testing, file_speed);
    let options = vec![MountOption::FSName("Remote-FS".to_string()), MountOption::RW];
    let mut session= Session::new(fs, &cli.mount_point, &options).expect("failed to mount");

    println!("Remote-FS mounted on {}", cli.mount_point);
    println!("Remote address: {}", cli.remote_address);
    println!("All set! Refer to /tmp/remote-fs.pid for killing the daemon.");

    let mut signals = Signals::new(&[SIGINT, SIGTERM, SIGQUIT, SIGHUP]).expect("signals");
    let mut unmounter = session.unmount_callable();
    let sig_handle = signals.handle();
    let sig_thread = thread::spawn(move || {
        for sig in &mut signals {
            println!("Signal {} received: unmounting...", sig);
            let _ = unmounter.unmount();
            break;
        }
    });

    let run_res = session.run(); // blocca finché non viene smontato o c’è un errore

    // Sveglia/chiudi il listener segnali e attendi che termini
    if !sig_handle.is_closed() {
        sig_handle.close();
    }
    sig_thread.join().expect("error joining signal thread");

    match run_res {
        Ok(()) => println!("Remote-FS closed successfully."),
        Err(e) => eprintln!("Remote-FS terminated with error: {e}")
    }
}

#[cfg(target_os = "windows")]
fn run_windows(cli: Cli, http_backend: HttpBackend, runtime: Arc<Runtime>) {
    use rfs_winfsp::RemoteFS;
    use std::sync::{Arc, Condvar, Mutex};
    use winfsp::host::{FileSystemHost, VolumeParams};

    let fs = RemoteFS::new(http_backend, runtime.clone());

    let mut vp = VolumeParams::default();
    vp.case_preserved_names(true);
    vp.case_sensitive_search(true);
    vp.unicode_on_disk(true);
    vp.reparse_points(true);

    let mut host = FileSystemHost::new(vp, fs).expect("Unable to create a FileSystemHost");

    host.mount(&cli.mount_point).expect("Unable to mount the filesystem");

    println!("Remote-FS mounted on {}", cli.mount_point);
    println!("Remote address: {}", cli.remote_address);
    println!("All set! Press Ctrl+C to unmount and exit.");

    // Coordinazione della terminazione senza busy-wait
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let pair_for_handler = pair.clone();

    ctrlc::set_handler(move || {
        let (lock, cvar) = &*pair_for_handler;
        let mut done = lock.lock().expect("lock poisoned");
        *done = true;
        cvar.notify_all(); // Sveglia il thread principale
    }).expect("failed to install Ctrl+C handler");

    host.start().expect("Unable to start the filesystem host");

    let (lock, cvar) = &*pair;
    let mut done = lock.lock().expect("lock poisoned");
    while !*done {
        done = cvar.wait(done).expect("condvar wait failed");
    }

    println!("\nSignal received, unmounting Remote-FS...");
    host.stop();
    host.unmount();
    println!("Remote-FS unmounted correctly");
}
