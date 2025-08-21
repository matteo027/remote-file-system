use reqwest::cookie::Jar;
use reqwest::{Client, Method, StatusCode, Url};
use rfs_models::{BackendError, FileEntry, RemoteBackend, SetAttrRequest};
use rpassword::read_password;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::PathBuf;
use std::str::{ FromStr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;
use tokio_stream::StreamExt;


#[derive(Deserialize, Debug)]
struct ErrorResponse {
    error: String,
}

#[derive(Serialize, Clone)]
pub struct Credentials {
    username: String, // it's the uid
    password: String,
}

#[derive(Deserialize,Debug)]
struct FileServerResponse {
    path: PathBuf,
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
    btime: SystemTime, 
}

pub struct HttpBackend {
    runtime: Arc<Runtime>, // from tokio, used to manage async calls
    base_url: Url,
    client: Client,
    credentials: Credentials
}

impl Credentials {

    pub fn first_authentication(address: String) -> Result<(Credentials, String), BackendError> {
        let client = Client::new();
        let mut sid = String::new();
        let base_url = Url::from_str(&address).expect("Invalid url");

        // Spawn a new OS thread to handle the async login workflow
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Unable to generate tokio Runtime");

            rt.block_on(async move {
                loop {
                    let mut username = String::new();
                    let password;
                    eprint!("username: ");
                    io::stdout().flush().unwrap();
                    io::stdin()
                        .read_line(&mut username)
                        .expect("Failed to read the username");
                    username = username.trim().to_string(); // removing the final endl
                    eprint!("password: ");
                    io::stdout().flush().unwrap();
                    password = read_password().unwrap_or_else(|_| {
                        eprintln!("\n[auth] Failed to read password");
                        String::new()
                    });
                    let login_url = base_url.join("api/login").unwrap();
                    let credentials = Self {
                        username: username,
                        password: password,
                    };
                    let resp_login = client
                        .post(login_url.clone())
                        .json(&credentials)
                        .send()
                        .await
                        .map_err(|e| BackendError::Other(e.to_string()))?;

                    eprintln!("[auth] login status: {:?}", resp_login.status());
                    for cookie in resp_login.cookies() {
                        sid = cookie.value().to_string();
                    }

                    if resp_login.status() == StatusCode::OK {
                        return Ok((credentials, sid));
                    } 
                    
                }
            })
        });

        // Wait for authentication thread to finish before proceeding
        handle
            .join()
            .unwrap_or_else(|e| Err(BackendError::Other(format!("Thread join failure: {:?}", e))))
    }
}

fn deserialize_systemtime_from_millis<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
where
    D: Deserializer<'de>,
{
    let millis: u64 = Deserialize::deserialize(deserializer)?;
    Ok(UNIX_EPOCH + Duration::from_millis(millis))
}

impl HttpBackend {
    pub fn new(address: String, credentials: Credentials, sid: String, rt: Arc<Runtime>) -> Result<Self, BackendError> {
        let base_url = Url::from_str(&address).expect("Invalid url");
        let cookie_jar = Arc::new(Jar::default());
        let cookie_str = format!("connect.sid={}", sid.trim());
        cookie_jar.add_cookie_str(&cookie_str, &base_url);
        let client = reqwest::Client::builder()
            .cookie_provider(cookie_jar)
            .build()
            .expect("Unable to build the Client object");



        let httpb = Self {
            runtime: rt,
            base_url,
            client,
            credentials
        };

        Ok(httpb)
    }

    fn authenticate(&self) -> Result<(), BackendError> {
        let client = self.client.clone();
        let address = self.base_url.clone();
        let credentials = self.credentials.clone();

        // Spawn a new OS thread to handle the async login workflow
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Unable to generate tokio Runtime");

            rt.block_on(async move {
                let login_url = address.join("api/login").unwrap();
                let resp_login = client
                    .post(login_url.clone())
                    .json(&credentials)
                    .send()
                    .await
                    .map_err(|e| BackendError::Other(e.to_string()))?;

                if resp_login.status() == StatusCode::OK {
                    return Ok(());
                } else if resp_login.status() == StatusCode::UNAUTHORIZED {
                    return Err(BackendError::Unauthorized);
                }
                return Err(BackendError::Other(String::from(resp_login.status().as_str())));
            })
        });

        // Wait for authentication thread to finish before proceeding
        handle
            .join()
            .unwrap_or_else(|e| Err(BackendError::Other(format!("Thread join failure: {:?}", e))))
    }

    fn response_to_entry(file: FileServerResponse) -> FileEntry {
        let name = file.path.file_name()
            .unwrap_or_else(|| OsStr::new("/"))
            .to_string_lossy()
            .to_string();
        let gid = file.group.unwrap_or(file.owner);
        FileEntry {
            ino: 0, // Inode number is not used in this context, set to 0, check if needed in cache layer
            path: file.path.to_string_lossy().to_string(),
            name,
            is_dir: (file.ty == 1) as bool,
            size: file.size,
            perms: file.permissions,
            nlinks: if file.ty == 1 { 2 } else { 1 },
            atime: file.atime,
            mtime: file.mtime,
            ctime: file.ctime,
            btime: file.btime,
            uid: file.owner,
            gid,
        }
    }

    fn request_no_response(&self, method: Method, endpoint: &str) -> Result<(), BackendError> {
        let url = self.base_url.join(endpoint).map_err(|e| BackendError::Other(e.to_string()))?;

        let mut token_expired = false;
        loop {
            let req = self.client.request(method.clone(), url.clone());
            let resp = self.runtime.block_on(async { req.send().await.map_err(|e| BackendError::Other(e.to_string())) }).expect("Unable to send request");
            match resp.status() {
                StatusCode::OK => return Ok(()),
                StatusCode::UNAUTHORIZED => {
                    if !token_expired {
                        self.authenticate()?;
                        token_expired = true;
                        continue; // retry the request after re-authentication
                    }
                    return Err(BackendError::Unauthorized);
                },
                StatusCode::FORBIDDEN => {
                    return Err(BackendError::Forbidden);
                },
                StatusCode::CONFLICT => {
                    let err = self.runtime.block_on(async { resp.json::<ErrorResponse>().await.unwrap().error });
                    return Err(BackendError::Conflict(err));
                }
                _ => return Err(BackendError::Other("Unexpected error".into())),
            }
        }
        
    }

    fn request<R: DeserializeOwned + 'static, B: Serialize>(&self,method: Method,endpoint: &str,body: Option<&B>) -> Result<R, BackendError> {
        let mut token_expired = false;
        loop {
            let url = self.base_url.join(endpoint).map_err(|e| BackendError::Other(e.to_string()))?;
            let mut req = self.client.request(method.clone(), url);
            if let Some(b) = body {
                req = req.json(b);
            }
            let resp = self.runtime.block_on(async { req.send().await.map_err(|e| BackendError::Other(e.to_string())) }).expect("Unable to send request");
            match resp.status() {
                StatusCode::OK => return self.runtime.block_on(async { resp.json().await.map_err(|_| BackendError::BadAnswerFormat) }),
                StatusCode::UNAUTHORIZED => {
                    if !token_expired {
                        self.authenticate()?;
                        token_expired = true;
                        continue; // retry the request after re-authentication
                    }
                    return Err(BackendError::Unauthorized);
                },
                StatusCode::FORBIDDEN => {
                    return Err(BackendError::Forbidden);
                },
                StatusCode::CONFLICT => {
                    let err = self.runtime.block_on(async { resp.json::<ErrorResponse>().await.unwrap().error });
                    return Err(BackendError::Conflict(err));
                }
                _ => return Err(BackendError::Other("Unexpected error".into())),
            };
        }
    }

    // fn request_stream_get(&self, method: Method, endpoint: &str) -> Result<Pin<Box<dyn Stream<Item = Result<bytes::Bytes, BackendError>> + Send>>, BackendError> {
    //     let mut token_expired = false;
    //     loop {
    //         let url = self.base_url.join(endpoint).map_err(|e| BackendError::Other(e.to_string()))?;
    //         let req = self.client.request(method.clone(), url);
    //         let resp = self.runtime.block_on(async { req.send().await.map_err(|e| BackendError::Other(e.to_string())) })?;
    //         match resp.status() {
    //             StatusCode::OK => return Ok(Box::pin(resp.bytes_stream().map_err(|e| BackendError::Other(e.to_string())))),
    //             StatusCode::UNAUTHORIZED => {
    //                 if !token_expired {
    //                     self.authenticate()?;
    //                     token_expired = true;
    //                     continue; // retry the request after re-authentication
    //                 }
    //                 return Err(BackendError::Unauthorized);
    //             },
    //             StatusCode::CONFLICT => {
    //                 let err = self.runtime.block_on(async { resp.json::<ErrorResponse>().await.unwrap().error });
    //                 return Err(BackendError::Conflict(err));
    //             }
    //             _ => return Err(BackendError::Other("Unexpected error".into())),
    //         };
    //     }
    // }
}

impl RemoteBackend for HttpBackend {
    fn list_dir(&self, path: &str) -> Result<Vec<FileEntry>, BackendError> {
        
        let endpoint = format!("api/directories/{}", path.trim_start_matches('/'));
        let files: Vec<FileServerResponse> = self.request::<Vec<FileServerResponse>, ()>(Method::GET, &endpoint, None)?;
        Ok(files.into_iter().map(Self::response_to_entry).collect())
    }

    fn create_dir(&self, path: &str) -> Result<FileEntry, BackendError> {
        
        let endpoint = format!("api/directories/{}", path.trim_start_matches('/'));
        let f: FileServerResponse = self.request::<FileServerResponse, ()>(Method::POST, &endpoint, None)?;
        Ok(Self::response_to_entry(f))
    }

    fn delete_dir(&self, path: &str) -> Result<(), BackendError> {
        
        let endpoint = format!("api/directories/{}", path.trim_start_matches('/'));
        self.request_no_response(Method::DELETE, &endpoint)?;
        Ok(())
    }

    fn get_attr(&self, path: &str) -> Result<FileEntry, BackendError> {
        
        let endpoint = format!("api/files/attributes/{}", path.trim_start_matches('/'));
        let f: FileServerResponse = self.request::<FileServerResponse, ()>(Method::GET, &endpoint, None)?;
        Ok(Self::response_to_entry(f))
    }

    fn create_file(&self, path: &str) -> Result<FileEntry, BackendError> {
        
        let endpoint = format!("api/files/{}", path.trim_start_matches('/'));
        let f: FileServerResponse = self.request::<FileServerResponse, ()>(Method::POST, &endpoint, None)?;
        Ok(Self::response_to_entry(f))
    }

    fn delete_file(&self, path: &str) -> Result<(), BackendError> {
        
        let endpoint = format!("api/files/{}", path.trim_start_matches('/'));
        self.request_no_response(Method::DELETE, &endpoint)?;
        Ok(())
    }

    fn read_chunk(&self,path: &str, offset: u64, size: u64) -> Result<Vec<u8>, BackendError> {
        println!("Reading chunk from path: {}, offset: {}, size: {}", path, offset, size);
        let endpoint = format!("api/files/{}?offset={}&size={}", path.trim_start_matches('/'), offset, size);
        let resp: serde_json::Value = self.request::<serde_json::Value, ()>(Method::GET, &endpoint, None)?;
        Ok(resp["data"].as_str().map(|s| s.as_bytes().to_vec()).unwrap_or_default())
    }

    fn write_chunk(&self, path: &str, offset: u64, data: Vec<u8>) -> Result<u64, BackendError> {
        let text = String::from_utf8_lossy(&data).to_string();
        let endpoint = format!("api/files/{}", path.trim_start_matches('/'));
        let body = serde_json::json!({ "offset": offset, "data": text });
        let resp: serde_json::Value = self.request(Method::PUT, &endpoint, Some(&body))?;
        Ok(resp["bytes"].as_u64().unwrap_or(0))
    }

    fn rename(&self, old_path: &str, new_path: &str) -> Result<FileEntry, BackendError> {
        
        let endpoint = format!("api/files/{}", old_path.trim_start_matches('/'));
        let body = serde_json::json!({ "new_path": new_path.trim_start_matches('/') });
        let f: FileServerResponse = self.request::<FileServerResponse, Value>(Method::PATCH, &endpoint, Some(&body))?;
        
        Ok(Self::response_to_entry(f))
    }

    fn set_attr(&self,path: &str,attrs: SetAttrRequest) -> Result<FileEntry, BackendError> {
        
        let endpoint = format!("api/files/attributes/{}", path.trim_start_matches('/'));
        let body = serde_json::to_value(attrs).map_err(|e| BackendError::Other(e.to_string()))?;
        let f: FileServerResponse = self.request::<FileServerResponse, Value>(Method::PATCH, &endpoint, Some(&body))?;
        
        Ok(Self::response_to_entry(f))
    }

    fn read_stream(&self, path: &str, offset: u64) -> Result<rfs_models::ByteStream, BackendError> {
        let mut token_expired=false;
        loop{
            let endpoint = format!("api/stream/files/{}?offset={}", path.trim_start_matches('/'), offset);
            let url = self.base_url.join(&endpoint).map_err(|e| BackendError::Other(e.to_string()))?;
            let req = self.client.request(Method::GET, url);
            let resp = self.runtime.block_on(async { req.send().await}).map_err(|e| BackendError::Other(e.to_string()))?;
            match resp.status() {
                StatusCode::OK => {
                    let s=resp.bytes_stream().map(|r| r.map_err(|e| BackendError::Other(e.to_string())));
                    return Ok(Box::pin(s));
                },
                StatusCode::UNAUTHORIZED => {
                    if !token_expired {
                        self.authenticate()?;
                        token_expired = true;
                        continue; // retry the request after re-authentication
                    }
                    return Err(BackendError::Unauthorized);
                },
                StatusCode::FORBIDDEN => {
                    return Err(BackendError::Forbidden);
                },
                StatusCode::CONFLICT => {
                    let err = self.runtime.block_on(async { resp.json::<ErrorResponse>().await}).map(|e| e.error).unwrap_or_else(|_| "Conflict".to_string());
                    return Err(BackendError::Conflict(err));
                },
                _ => return Err(BackendError::Other("Unexpected error".into())),
            }
        }
    }
}
