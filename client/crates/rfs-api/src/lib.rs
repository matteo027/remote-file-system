use bytes::Bytes;
use reqwest::cookie::Jar;
use reqwest::{Client, Method, StatusCode, Url};
use rfs_models::{BackendError, FileEntry, RemoteBackend, SetAttrRequest};
use rpassword::read_password;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::pin::Pin;
use std::str::{ FromStr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;
use tokio_stream::Stream;
use futures_util::stream::TryStreamExt;
use futures_util::StreamExt;
use futures_util::stream;

#[derive(Deserialize, Debug)]
struct ErrorResponse {
    error: String,
}

#[derive(Serialize)]
struct LoginPayload {
    username: String, // it's the uid
    password: String,
}

#[derive(Deserialize)]
struct WriteResponse {
    bytes: u64,
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
    runtime: Runtime, // from tokio, used to manage async calls
    base_url: Url,
    client: Client,
    cookie_jar: Arc<Jar>
}

fn deserialize_systemtime_from_millis<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
where
    D: Deserializer<'de>,
{
    let millis: u64 = Deserialize::deserialize(deserializer)?;
    Ok(UNIX_EPOCH + Duration::from_millis(millis))
}

impl HttpBackend {
    pub fn new() -> Self {
        let cookie_jar = Arc::new(Jar::default());
        let client = reqwest::Client::builder()
            .cookie_provider(cookie_jar.clone())
            .build()
            .expect("Unable to build the Client object");
        Self {
            runtime: Runtime::new().expect("Unable to built a Runtime object"),
            base_url: Url::from_str("http://localhost:3000/").unwrap(), // meglio passarlo come parametro la metodo (?)
            client,
            cookie_jar
        }
    }

    fn check_and_authenticate(&self) -> Result<(), BackendError> {
        let client = self.client.clone();
        let address = self.base_url.clone();
        let cookie_jar = self.cookie_jar.clone();

        // Spawn a new OS thread to handle the async login workflow
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Unable to generate tokio Runtime");

            rt.block_on(async move {


                // trying to read from /temp/rfs-token
                if let Ok(token) = fs::read_to_string("/tmp/rfs-token") {
                    let cookie_str = format!("connect.sid={}", token.trim());
                    cookie_jar.add_cookie_str(&cookie_str, &address);
                }

                // Step 1: check /api/me
                let me_url = address.join("api/me").unwrap();
                let resp = client
                    .get(me_url.clone())
                    .send()
                    .await

                    .map_err(|e| BackendError::Other(e.to_string()))?;

                if resp.status() == StatusCode::OK {
                    return Ok(());
                }

                if resp.status() == StatusCode::UNAUTHORIZED {
                    // Step 2: login
                    loop {

                        let mut username = String::new();
                        let password;
                        print!("username: ");
                        io::stdout().flush().unwrap();
                        io::stdin()
                            .read_line(&mut username)
                            .expect("Failed to read the username");
                        username = username.trim().to_string(); // removing the final endl
                        print!("password: ");
                        io::stdout().flush().unwrap();
                        password = read_password().unwrap_or_else(|_| {
                            println!("\n[auth] Failed to read password");
                            String::new()
                        });
                        let login_url = address.join("api/login").unwrap();
                        let body = LoginPayload {
                            username: username,
                            password: password,
                        };
                        let resp_login = client
                            .post(login_url.clone())
                            .json(&body)
                            .send()
                            .await
                            .map_err(|e| BackendError::Other(e.to_string()))?;

                        println!("[auth] login status: {:?}", resp_login.status());
                        for cookie in resp_login.cookies() {
                            println!("[auth] cookie: {}={}", cookie.name(), cookie.value());
                            fs::create_dir_all("/tmp").expect("Unbale to create /tmp");
                            fs::write("/tmp/rfs-token", cookie.value()).expect("Unbale to create /tmp/rfs-token");
                        }

                        if resp_login.status() == StatusCode::OK {
                            // Step 3: optionally verify with /api/me again
                            let verify = client
                                .get(me_url.clone())
                                .send()
                                .await
                                .map_err(|e| BackendError::Other(e.to_string()))?;
                            println!("verify status: {}", verify.status());
                            for cookie in resp_login.cookies() {
                                println!("[veri] cookie: {}={}", cookie.name(), cookie.value());
                            }
                            if verify.status() == StatusCode::OK {
                                return Ok(());
                            }
                        } 
                    }
                }

                Err(BackendError::Other(format!(
                    "Unexpected status: {}",
                    resp.status()
                )))
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
        let req = self.client.request(method, url);
        let resp = self.runtime.block_on(async { req.send().await.map_err(|e| BackendError::Other(e.to_string())) })?;
        match resp.status() {
            StatusCode::OK => Ok(()),
            StatusCode::UNAUTHORIZED => Err(BackendError::Unauthorized),
            StatusCode::CONFLICT => {
                let err = self.runtime.block_on(async { resp.json::<ErrorResponse>().await.unwrap().error });
                Err(BackendError::Conflict(err))
            }
            _ => Err(BackendError::Other("Unexpected error".into())),
        }
    }

    fn request<R: DeserializeOwned + 'static, B: Serialize>(&self,method: Method,endpoint: &str,body: Option<&B>) -> Result<R, BackendError> {
        let url = self.base_url.join(endpoint).map_err(|e| BackendError::Other(e.to_string()))?;
        let mut req = self.client.request(method, url);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = self.runtime.block_on(async { req.send().await.map_err(|e| BackendError::Other(e.to_string())) })?;
        match resp.status() {
            StatusCode::OK => self.runtime.block_on(async { resp.json().await.map_err(|_| BackendError::BadAnswerFormat) }),
            StatusCode::UNAUTHORIZED => Err(BackendError::Unauthorized),
            StatusCode::CONFLICT => {
                let err = self.runtime.block_on(async { resp.json::<ErrorResponse>().await.unwrap().error });
                Err(BackendError::Conflict(err))
            }
            _ => Err(BackendError::Other("Unexpected error".into())),
        }
    }

    fn request_stream_get(&self, method: Method, endpoint: &str) -> Result<Pin<Box<dyn Stream<Item = Result<bytes::Bytes, BackendError>> + Send>>, BackendError> {
        let url = self.base_url.join(endpoint).map_err(|e| BackendError::Other(e.to_string()))?;
        let req = self.client.request(method, url);
        let resp = self.runtime.block_on(async { req.send().await.map_err(|e| BackendError::Other(e.to_string())) })?;
        
        match resp.status() {
            StatusCode::OK => Ok(Box::pin(resp.bytes_stream().map_err(|e| BackendError::Other(e.to_string())))),
            StatusCode::UNAUTHORIZED => Err(BackendError::Unauthorized),
            StatusCode::CONFLICT => {
                let err = self.runtime.block_on(async { resp.json::<ErrorResponse>().await.unwrap().error });
                Err(BackendError::Conflict(err))
            }
            _ => Err(BackendError::Other("Unexpected error".into())),
        }
    }

     fn request_stream_send<R: DeserializeOwned + 'static, S: Stream<Item = Result<Bytes, BackendError>> + Send + 'static,>(&self, method: Method, endpoint: &str, offset: u64, stream: S) -> Result<R, BackendError> {
        let url = self.base_url.join(endpoint).map_err(|e| BackendError::Other(e.to_string()))?;
        let req = self.client.request(method, url)
            .header("X-Chunk-Offset", offset.to_string()) // non posso mettere offset nel body in quanto è occupato dallo stream di byte
            .body(reqwest::Body::wrap_stream(stream));
        let resp = self.runtime.block_on(async { req.send().await.map_err(|e| BackendError::Other(e.to_string())) })?;
        
        match resp.status() {
            StatusCode::OK => self.runtime.block_on(async {
                resp.json::<R>().await.map_err(|_| BackendError::BadAnswerFormat)
            }),
            StatusCode::UNAUTHORIZED => Err(BackendError::Unauthorized),
            StatusCode::CONFLICT => {
                let err = self.runtime.block_on(async {
                    resp.json::<ErrorResponse>().await.unwrap().error
                });
                Err(BackendError::Conflict(err))
            }
            _ => Err(BackendError::Other("Unexpected error".into())),
        }
    }


}

impl RemoteBackend for HttpBackend {
    fn list_dir(&self, path: &str) -> Result<Vec<FileEntry>, BackendError> {
        self.check_and_authenticate()?;
        let endpoint = format!("api/directories/{}", path.trim_start_matches('/'));
        let files: Vec<FileServerResponse> = self.request::<Vec<FileServerResponse>, ()>(Method::GET, &endpoint, None)?;
        Ok(files.into_iter().map(Self::response_to_entry).collect())
    }

    fn create_dir(&self, path: &str) -> Result<FileEntry, BackendError> {
        self.check_and_authenticate()?;
        let endpoint = format!("api/directories/{}", path.trim_start_matches('/'));
        let f: FileServerResponse = self.request::<FileServerResponse, ()>(Method::POST, &endpoint, None)?;
        Ok(Self::response_to_entry(f))
    }

    fn delete_dir(&self, path: &str) -> Result<(), BackendError> {
        self.check_and_authenticate()?;
        let endpoint = format!("api/directories/{}", path.trim_start_matches('/'));
        self.request_no_response(Method::DELETE, &endpoint)?;
        Ok(())
    }

    fn get_attr(&self, path: &str) -> Result<FileEntry, BackendError> {
        self.check_and_authenticate()?;
        let endpoint = format!("api/files/attributes/{}", path.trim_start_matches('/'));
        let f: FileServerResponse = self.request::<FileServerResponse, ()>(Method::GET, &endpoint, None)?;
        Ok(Self::response_to_entry(f))
    }

    fn create_file(&self, path: &str) -> Result<FileEntry, BackendError> {
        self.check_and_authenticate()?;
        let endpoint = format!("api/files/{}", path.trim_start_matches('/'));
        let f: FileServerResponse = self.request::<FileServerResponse, ()>(Method::POST, &endpoint, None)?;
        Ok(Self::response_to_entry(f))
    }

    fn delete_file(&self, path: &str) -> Result<(), BackendError> {
        self.check_and_authenticate()?;
        let endpoint = format!("api/files/{}", path.trim_start_matches('/'));
        self.request_no_response(Method::DELETE, &endpoint)?;
        Ok(())
    }

    fn read_chunk(&self,path: &str, offset: u64, size: u64) -> Result<Vec<u8>, BackendError> {
        self.check_and_authenticate()?;
        
        let endpoint = format!("api/files/{}?offset={}&size={}", path.trim_start_matches('/'), offset, size);
        let mut stream = self.request_stream_get(Method::GET, &endpoint)?;
        let mut data = Vec::<u8>::new();

        self.runtime.block_on(async {
            while let Some(chunk) = stream.next().await {
                let bytes = chunk.expect("Unable to read the chunck");
                data.extend_from_slice(&bytes);
            }
        });

        Ok(data)
    }

    fn write_chunk(&self, path: &str, offset: u64, data: Vec<u8>) -> Result<u64, BackendError> {
        self.check_and_authenticate()?;
        let endpoint = format!("api/files/{}", path.trim_start_matches('/'));
        let data_stream = stream::once(async move { Ok(Bytes::from(data)) });

        let resp: WriteResponse = self.request_stream_send(Method::PUT, &endpoint, offset, data_stream)?;

        Ok(resp.bytes)
    }

    fn rename(&self, old_path: &str, new_path: &str) -> Result<FileEntry, BackendError> {
        self.check_and_authenticate()?;
        let endpoint = format!("api/files/{}", old_path.trim_start_matches('/'));
        let body = serde_json::json!({ "new_path": new_path.trim_start_matches('/') });
        let f: FileServerResponse = self.request::<FileServerResponse, Value>(Method::PATCH, &endpoint, Some(&body))?;
        
        Ok(Self::response_to_entry(f))
    }

    fn set_attr(&self,path: &str,attrs: SetAttrRequest) -> Result<FileEntry, BackendError> {
        self.check_and_authenticate()?;
        let endpoint = format!("api/files/attributes/{}", path.trim_start_matches('/'));
        let body = serde_json::to_value(attrs).map_err(|e| BackendError::Other(e.to_string()))?;
        let f: FileServerResponse = self.request::<FileServerResponse, Value>(Method::PATCH, &endpoint, Some(&body))?;
        
        Ok(Self::response_to_entry(f))
    }
}
