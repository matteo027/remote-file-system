use reqwest::cookie::Jar;
use rfs_models::{RemoteBackend,DirectoryEntry, BackendError};
use std::str::FromStr;
use std::sync::Arc;
use std::time::SystemTime;
use std::collections::HashMap;
use std::path::{Path};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;

#[derive(Deserialize, Debug)]
struct ErrorResponse {
    error: String,
}

pub struct StubBackend{
    //test purposes
    dirs: HashMap<String,Vec<DirectoryEntry>>,
}

pub struct Server{
    runtime: Runtime, // from tokio, used to manage async calls
    address: Url,
    client: Client
}

#[derive(Serialize)]
struct DirApisPayload {
    path: String
}
#[derive(Serialize)]
struct LoginPayload {
    username: String,
    password: String
}

fn now() -> SystemTime {
    SystemTime::now()
}

impl RemoteBackend for StubBackend {
    fn new() -> Self {
        let mut dirs = HashMap::new();
        // la root ("" o "/") contiene file1, file2 e dir1
        dirs.insert("".into(), vec![
        DirectoryEntry::new(2, "file1.txt".into(), false, 1024, 0o644, 1, 0, 0, now(), now(), now()),
        DirectoryEntry::new(3, "file2.txt".into(), false, 2048, 0o644, 1, 0, 0, now(), now(), now()),
        DirectoryEntry::new(4, "dir1".into(), true, 0, 0o755, 1, 0, 0, now(), now(), now()),
        ]);
        // dentro /dir1 c'è dir2
        dirs.insert("/dir1".into(), vec![
        DirectoryEntry::new(5, "dir2".into(), true, 0, 0o755, 1, 0, 0, now(), now(), now()),
        ]);
        // /dir1/dir2 è vuota
        dirs.insert("/dir1/dir2".into(), vec![]);

        StubBackend { dirs }
    }

    fn create_dir(&mut self, mut entry:DirectoryEntry) -> Result<(), BackendError> {
        let full=entry.name.clone();

        let parent=match full.rfind('/') {
            Some(idx) => full[..idx].to_string(),
            _ => "".to_string(),
        };
        let name=full.split('/').last().unwrap().to_string();

        entry.name = name.clone();

        self.dirs.entry(parent).or_default().push(entry);
        self.dirs.entry(full).or_insert_with(Vec::new);
        Ok(())
    }

    fn delete_dir(&mut self, path: &str) -> Result<(), BackendError> {
        if self.dirs.remove(path).is_some() {
            let parent_path = match path.rfind('/') {
                Some(idx) => &path[..idx],
                _ => "",
            };
            let dir_name = Path::new(path).file_name().unwrap().to_str().unwrap().to_string();

            if let Some(children) = self.dirs.get_mut(parent_path) {
                children.retain(|entry| entry.name != dir_name);
            }
            Ok(())
        } else {
            Err(BackendError::NotFound(path.to_string()))
        }
    }

    fn list_dir(&mut self, path: &str) -> Result<Vec<DirectoryEntry>, BackendError> {
        let key = if path == "/" { "".to_string() } else { path.to_string() };
        match self.dirs.get(&key) {
        Some(v) => Ok(v.clone()),
        None    => Err(BackendError::NotFound(path.to_string())),
        }
    }
    
    fn check_and_authenticate(&mut self) -> Result<(), BackendError> {
        todo!()
    }
}

impl RemoteBackend for Server {
    fn new() -> Self
    where
        Self: Sized
    {
        Self {
            runtime: Runtime::new().unwrap(),
            address: Url::from_str("http://localhost:3000/").unwrap(), // meglio passarlo come parametro la metodo (?)
            client: {
                let cookie_jar = Arc::new(Jar::default());
                // Build client with the cookie jar
                reqwest::Client::builder()
                    .cookie_provider(Arc::clone(&cookie_jar))
                    .build().expect("Unable to build the Client object")
            }
        }
    }

    fn list_dir(&mut self, path: &str) -> Result<Vec<DirectoryEntry>, BackendError> {

        self.check_and_authenticate()?;

        let api_result = self.runtime.block_on(async {
            let request_url = self.address.clone()
                .join("api/directories").unwrap();
            let body = DirApisPayload {
                path: String::from(path.strip_prefix('/').unwrap_or(path))
            };
            
            let resp = self.client
                .get(String::from(request_url))
                .json(&body)
                .send()
                .await;

            match resp {
                Ok(resp) => { 
                    match resp.status() {
                        StatusCode::OK => {
                            match resp.json::<Vec<DirectoryEntry>>().await {
                                Ok(files) => return Ok(files),
                                Err(e) => Err(BackendError::BadAnswerFormat)
                            }
                        },
                        StatusCode::UNAUTHORIZED => Err(BackendError::Unauthorized),
                        StatusCode::CONFLICT => Err(BackendError::Conflict(resp.json::<ErrorResponse>().await.unwrap().error)),
                        StatusCode::INTERNAL_SERVER_ERROR => Err(BackendError::InternalServerError),
                        _ => Err(BackendError::Other(String::from("Unknown error")))
                    }
                }
                Err(err) => Err(BackendError::Other(err.to_string()))
            }
        });
        
        return api_result;
    }

    fn create_dir(&mut self, entry: DirectoryEntry) -> Result<(), BackendError> {

        let body = DirApisPayload {
            path: String::from(Path::new(&entry.name).parent().unwrap_or(Path::new("")).to_str().unwrap_or(""))
        };

        let api_result = self.runtime.block_on(async {
            let request_url = self.address.clone()
                .join("api/directories").unwrap()
                .join(Path::new(&entry.name).file_name().unwrap_or_default().to_str().unwrap_or("")).unwrap();
            
            let resp =self.client
                .post(request_url)
                .json(&body)
                .send()
                .await;
            match resp {
                Ok(resp) => {
                    match resp.status() {
                        StatusCode::OK => Ok(()),
                        StatusCode::UNAUTHORIZED => Err(BackendError::Unauthorized),
                        StatusCode::CONFLICT => Err(BackendError::Conflict(resp.json::<ErrorResponse>().await.unwrap().error)),
                        StatusCode::INTERNAL_SERVER_ERROR => Err(BackendError::InternalServerError),
                        _ => Err(BackendError::Other(String::from("Unknown error")))
                    }
                }
                Err(err) => Err(BackendError::Other(err.to_string()))
            }
        });
        
        return api_result
    }

    fn delete_dir(&mut self, path: &str) -> Result<(), BackendError> {
        
        let body = DirApisPayload {
            path: String::from(Path::new(path).parent().unwrap_or(Path::new("")).to_str().unwrap_or(""))
        };

        let api_result = self.runtime.block_on(async {
            let request_url = self.address.clone()
                .join("api/directories").unwrap()
                .join(Path::new(path).file_name().unwrap_or_default().to_str().unwrap_or("")).unwrap();
            
            let resp =self.client
                .delete(request_url)
                .json(&body)
                .send()
                .await;
            match resp {
                Ok(resp) => {
                    match resp.status() {
                        StatusCode::OK => Ok(()),
                        StatusCode::UNAUTHORIZED => Err(BackendError::Unauthorized),
                        StatusCode::CONFLICT => Err(BackendError::Conflict(resp.json::<ErrorResponse>().await.unwrap().error)),
                        StatusCode::INTERNAL_SERVER_ERROR => Err(BackendError::InternalServerError),
                        _ => Err(BackendError::Other(String::from("Unknown error")))
                    }
                }
                Err(err) => Err(BackendError::Other(err.to_string()))
            }
        });
        
        return api_result
    }
    
    fn check_and_authenticate(&mut self) -> Result<(), BackendError> {
    let client = self.client.clone();
    let address = self.address.clone();

    // Spawn a new OS thread to handle the async login workflow
    let handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async move {
            // Step 1: check /api/me
            let me_url = address.join("api/me").unwrap();
            let resp = client.get(me_url.clone()).send().await.map_err(|e| BackendError::Other(e.to_string()))?;

            if resp.status() == StatusCode::OK {
                return Ok(());
            }

            if resp.status() == StatusCode::UNAUTHORIZED {
                // Step 2: login
                let login_url = address.join("api/login").unwrap();
                let body = LoginPayload {
                    username: "admin".into(),
                    password: "admin".into(),
                };
                let resp_login = client.post(login_url.clone())
                    .json(&body)
                    .send()
                    .await.map_err(|e| BackendError::Other(e.to_string()))?;

                println!("[auth] login status: {:?}", resp_login.status());
                for cookie in resp_login.cookies() {
                    println!("[auth] cookie: {}={}", cookie.name(), cookie.value());
                }

                if resp_login.status() == StatusCode::OK {
                    // Step 3: optionally verify with /api/me again
                    let verify = client.get(me_url).send().await.map_err(|e| BackendError::Other(e.to_string()))?;
                    if verify.status() == StatusCode::OK {
                        return Ok(());
                    } else {
                        return Err(BackendError::Unauthorized);
                    }
                } else {
                    return Err(BackendError::Unauthorized);
                }
            }

            Err(BackendError::Other(format!("Unexpected status: {}", resp.status())))
        })
    });

    // Wait for authentication thread to finish before proceeding
    handle.join().unwrap_or_else(|e| {
        Err(BackendError::Other(format!("Thread join failure: {:?}", e)))
    }) 
}

}