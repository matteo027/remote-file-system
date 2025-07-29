use reqwest::cookie::Jar;
use rfs_models::{RemoteBackend,FileEntry, BackendError};
use std::fs::OpenOptions;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::collections::HashMap;
use std::path::{Path};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Deserializer, Serialize};
use tokio::runtime::Runtime;
use std::io::Write;

#[derive(Deserialize, Debug)]
struct ErrorResponse {
    error: String,
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
    username: String, // it's the uid
    password: String
}
#[derive(Deserialize)]
struct FileServerResponse {
    path: Box<Path>,
    owner: u32,
    group: Option<u32>,
    #[serde(rename = "type")]
    ty: usize,
    permissions: u16,
    size: u64,
    #[serde(deserialize_with = "deserialize_systemtime_from_millis")]
    atime: SystemTime,
    #[serde(deserialize_with = "deserialize_systemtime_from_millis")]
    mtime: SystemTime,
    #[serde(deserialize_with = "deserialize_systemtime_from_millis")]
    ctime: SystemTime,
    #[serde(deserialize_with = "deserialize_systemtime_from_millis")]
    btime: SystemTime
}

fn deserialize_systemtime_from_millis<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
where
    D: Deserializer<'de>,
{
    let millis: u64 = Deserialize::deserialize(deserializer)?;
    Ok(UNIX_EPOCH + Duration::from_millis(millis))
}

impl RemoteBackend for Server {
    fn new() -> Self
    where
        Self: Sized
    {
        Self {
            runtime: Runtime::new().expect("Unable to built a Runtime object"),
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

    fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>, BackendError> {

        self.check_and_authenticate()?;

        let api_result = self.runtime.block_on(async {
            let request_url = self.address.clone()
                .join("api/directories/").unwrap()
                .join(path.strip_prefix('/').unwrap_or(path)).unwrap();
            
            let resp = self.client
                .get(request_url)
                .send()
                .await;
            
            match resp {
                Ok(resp) => { 
                    match resp.status() {
                        StatusCode::OK => {
                            match resp.json::<Vec<FileServerResponse>>().await {
                                Ok(files) => return Ok(files.into_iter().map(|f|{

                                    FileEntry {
                                        ino: 0,
                                        name: f.path.file_name().expect("Unable to get the file name").to_str().unwrap().to_string(),
                                        is_dir: f.ty == 1,
                                        size: f.size,
                                        perms: f.permissions,
                                        nlinks: 0,
                                        atime: f.atime,
                                        mtime: f.mtime,
                                        ctime: f.ctime,
                                        uid: f.owner,
                                        gid: match f.group {
                                            Some(g) => g,
                                            None => f.owner
                                        }
                                    }
                                }).collect()),
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

    fn create_dir(&mut self, entry: FileEntry) -> Result<(), BackendError> {

        self.check_and_authenticate()?;

        let api_result = self.runtime.block_on(async {
            let request_url = self.address.clone()
                .join("api/directories/").unwrap()
                .join(&entry.name.strip_prefix('/').unwrap_or(&entry.name)).unwrap();
            
            let resp =self.client
                .post(request_url)
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
        
        self.check_and_authenticate()?;

        let api_result = self.runtime.block_on(async {
            let request_url = self.address.clone()
                .join("api/directories/").unwrap()
                .join(path.strip_prefix('/').unwrap_or(path)).unwrap();
            
            let resp =self.client
                .delete(request_url)
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
        
        return api_result;
    }
    
    fn check_and_authenticate(&mut self) -> Result<(), BackendError> {
        let client = self.client.clone();
        let address = self.address.clone();

        // Spawn a new OS thread to handle the async login workflow
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Unable to geenrate e tokio Runtime");

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
                        username: "5000".into(),
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