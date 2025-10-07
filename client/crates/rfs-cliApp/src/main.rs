use clap::{Parser,ArgAction};
use rfs_api::{HttpBackend,Credentials};
use std::sync::Arc;
use tokio::runtime::{Builder,Runtime};
use std::thread;

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
    #[arg(short, long, default_value = "http://localhost:3000")]  //"http://fzucca.com:25570"
    remote_address: String,

    /// Abilita la modalità speed testing (solo Linux)
    #[arg(long, action = ArgAction::SetTrue)]
    speed_testing: bool,
}

// su windows settare:
// $env:PATH += ";C:\Program Files (x86)\WinFsp\bin"

fn main(){
    let cli = Cli::parse();

    // first authentication
    let (credentials, sessionid) = match Credentials::first_authentication(cli.remote_address.clone()) {
        Ok(creds) =>{
            println!("Authentication successful.");
            creds
        } ,
        Err(e) => {
            eprintln!("Error reading credentials: {}", e);
            panic!("Cannot continue without credentials");
        }
    };

    #[cfg(target_os = "linux")] // only on linux
    daemonize();


    let runtime= Arc::new(Builder::new_multi_thread().enable_all().thread_name("rfs-runtime").build().expect("Unable to build a Runtime object"));
    let http_backend= HttpBackend::new(cli.remote_address.clone(), credentials, sessionid, runtime.clone()).expect("Cannot create the HTTP backend");

    #[cfg(unix)]
    run_unix(cli, http_backend, runtime);
    #[cfg(target_os = "windows")]
    run_windows(cli, http_backend, runtime);
}

#[cfg(target_os = "linux")]
fn daemonize() -> () {
    use std::fs::File;
    use daemonize::Daemonize;
    
    let stdout = File::create("/tmp/remote-fs.log").expect("Failed to create log file");
    let stderr = File::create("/tmp/remote-fs.err").expect("Failed to create error log file");
    
    const PID_FILE :&str = "/tmp/remote-fs.pid";
    if std::path::Path::new(PID_FILE).exists() {
        if let Ok(pid_content) = std::fs::read_to_string(PID_FILE) {
            if let Ok(pid) = pid_content.trim().parse::<u32>() {
                let proc_path = format!("/proc/{}", pid);
                if std::path::Path::new(&proc_path).exists() {
                    eprintln!("Remote-FS daemon is already running with PID: {}", pid);
                    eprintln!("To stop it, run: kill {}", pid);
                    panic!("Daemon already running");
                } else {
                    let _ = std::fs::remove_file(PID_FILE);
                }
            }
        }
    }

    let daemonize = Daemonize::new()
        .pid_file(PID_FILE) // saves PID
        .stdout(stdout) // log stdout
        .stderr(stderr) // log stderr
        .working_directory("/")
        .umask(0o027); // file's default permission

    daemonize.start().expect("Error, daemonization failed");
}

#[cfg(unix)]
fn run_unix(cli: Cli, http_backend: HttpBackend, runtime: Arc<Runtime>){
    use fuser::{MountOption,Session};
    use std::fs::File;
    use rfs_fuse::RemoteFS;
    use signal_hook::consts::*;
    use signal_hook::iterator::Signals;
    //use rfs_cache::Cache;

    let file_speed= if cfg!(target_os = "linux") && cli.speed_testing {
        println!("Speed testing mode enabled.");
        Some(File::create("/tmp/remote-fs.speed-test.out").expect("Failed to create speed test log file"))
    }else{
        None
    };

    //let cache = Cache::new(http_backend, 256, 16, 64, 16); // 256 attr, 16 dir, 64 blocchi per file (da 16 Kb), 16 file
    let fs = RemoteFS::new(http_backend, runtime.clone(), cli.speed_testing, file_speed);
    let options = vec![MountOption::FSName("Remote-FS".to_string()), MountOption::RW];
    let mut session= Session::new(fs, &cli.mount_point, &options).expect("failed to mount");

    println!("Remote-FS mounted on {}", cli.mount_point);
    println!("Remote address: {}", cli.remote_address);

    let mut signals = Signals::new(&[SIGINT, SIGTERM, SIGQUIT, SIGHUP]).expect("signals");
    let mut unmounter = session.unmount_callable();
    let sig_handle = signals.handle();
    let sig_thread = thread::spawn(move || {
        for sig in &mut signals {
            println!("Segnale {} ricevuto: smonto…", sig);
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
        Ok(()) => println!("Remote-FS chiuso correttamente."),
        Err(e) => eprintln!("Remote-FS terminato con errore: {e}")
    }
}

#[cfg(target_os = "windows")]
fn run_windows(cli: Cli, http_backend: HttpBackend, runtime: Arc<Runtime>){
    use rfs_winfsp::RemoteFS;
    use winfsp::host::{FileSystemHost, VolumeParams};
    use std::sync::atomic::{AtomicBool, Ordering};
    use signal_hook::flag;

    let fs = RemoteFS::new(http_backend, runtime.clone());
    let mut vp = VolumeParams::default();
    vp.case_preserved_names(true);
    vp.case_sensitive_search(true);
    vp.unicode_on_disk(true);
    vp.reparse_points(true);

    let mut host = FileSystemHost::new(vp, fs).expect("Unable o create a FileSystemHost");
    host.mount(&cli.mount_point).expect("Unable to mount the filesystem");

    println!("Remote-FS mounted on {}", cli.mount_point);
    println!("Remote address: {}", cli.remote_address);
    host.start().expect("Unable to start the filesystem host");

    let term = Arc::new(AtomicBool::new(false));
    flag::register(SIGINT, term.clone()).expect("register SIGINT");
    flag::register(SIGTERM, term.clone()).expect("register SIGTERM");

    // Polling leggero dell’AtomicBool
    while !term.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    println!("\nSignal received, unmounting Remote-FS...");
    if let Err(e) = host.stop() {
        eprintln!("Error stopping Remote-FS: {}", e);
    } 
    if let Err(e) = host.unmount() {
        eprintln!("Error unmounting Remote-FS: {}", e);
    }
    println!("Remote-FS unmounted correctly");
}