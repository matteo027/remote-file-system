use reqwest::cookie::Jar;
use reqwest::{Client, Method, Response, StatusCode, Url};
use rfs_models::{BackendError, FileEntry, RemoteBackend, SetAttrRequest};
use rpassword::read_password;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::ffi::OsStr;
use std::io::{stdin,stdout, Write};
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
                    print!("username: ");  
                    stdout().flush().unwrap();                  
                    stdin().read_line(&mut username).expect("Failed to read the username");
                    username = username.trim().to_string(); // removing the final endl
                    print!("password: ");
                    stdout().flush().unwrap();
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

                    println!("[auth] login status: {:?}", resp_login.status());
                    let sid = resp_login.cookies().find(|c| c.name() == "connect.sid").map(|c| c.value().to_string()).ok_or_else(|| BackendError::Other("Missing session cookie 'connect.sid'".into()))?;

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

impl HttpBackend {
    pub fn new(address: String, credentials: Credentials, sid: String, rt: Arc<Runtime>) -> Result<Self, BackendError> {
        let base_url = Url::from_str(&address).expect("Invalid url");
        let cookie_jar = Arc::new(Jar::default());
        let cookie_str = format!("connect.sid={}", sid.trim());
        cookie_jar.add_cookie_str(&cookie_str, &base_url);
        let client = reqwest::Client::builder()
            .cookie_provider(cookie_jar.clone())
            .timeout(Duration::from_secs(30))
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
        let login_url= self.base_url.join("api/login").map_err(|e| BackendError::Other(e.to_string()))?;
        let client = self.client.clone();
        let credentials = self.credentials.clone();

        self.runtime.block_on(async move{
            let resp = client.post(login_url).json(&credentials).send().await
                .map_err(|e| BackendError::Other(e.to_string()))?;
            match resp.status(){
                StatusCode::OK => Ok(()),
                StatusCode::UNAUTHORIZED => Err(BackendError::Unauthorized),
                s => Err(BackendError::Other(format!("HTTP {}", s))),
            }
        })
    }

    fn raw_request<B: Serialize>(&self, method: Method, endpoint: &str, body: Option<&B>) -> Result<Response, BackendError> {
        let mut retried = false;
        loop {
            let url = self.base_url.join(endpoint).map_err(|e| BackendError::Other(e.to_string()))?;
            let mut req = self.client.request(method.clone(), url);
            if let Some(b) = body { req = req.json(b); }
            let resp= self.runtime.block_on(async {req.send().await}).map_err(|e| BackendError::Other(e.to_string()))?;
            if resp.status() == StatusCode::UNAUTHORIZED && !retried{
                self.authenticate()?;
                retried=true;
                continue;
            }
            return Ok(resp);
        }
    }

    fn request_response<R: DeserializeOwned, B: Serialize>(&self,method: Method,endpoint: &str,body: Option<&B>) -> Result<R, BackendError> {
        let resp=self.raw_request(method, endpoint, body)?;
        match resp.status(){
            StatusCode::OK | StatusCode::CREATED =>{
                self.runtime.block_on(async{resp.json().await}).map_err(|_| BackendError::BadAnswerFormat)
            }
            _ => Err(self.decode_error(resp, endpoint)),
        }
    }

    fn decode_error(&self, resp:Response, endpoint: &str) -> BackendError {
        match resp.status() {
            StatusCode::UNAUTHORIZED => BackendError::Unauthorized,
            StatusCode::FORBIDDEN => BackendError::Forbidden,
            StatusCode::NOT_FOUND => BackendError::NotFound(endpoint.to_string()),
            StatusCode::CONFLICT => {
                let msg = self.runtime.block_on(async { resp.json::<ErrorResponse>().await.ok().map(|e| e.error) }).unwrap_or_else(|| "Conflict".to_string());
                BackendError::Conflict(msg)
            }
            StatusCode::INTERNAL_SERVER_ERROR => BackendError::InternalServerError,
            StatusCode::BAD_REQUEST => BackendError::BadAnswerFormat,
            StatusCode::SERVICE_UNAVAILABLE => BackendError::ServerUnreachable,
            other => BackendError::Other(format!("HTTP {}", other)),
        }
    }
}

impl RemoteBackend for HttpBackend {
    fn list_dir(&self, path: &str) -> Result<Vec<FileEntry>, BackendError> {
        let endpoint = format!("api/directories/{}", path.trim_start_matches('/'));
        let files: Vec<FileServerResponse> = self.request_response::<Vec<FileServerResponse>, ()>(Method::GET, &endpoint, None)?;
        Ok(files.into_iter().map(response_to_entry).collect())
    }

    fn create_dir(&self, path: &str) -> Result<FileEntry, BackendError> {
        let endpoint = format!("api/directories/{}", path.trim_start_matches('/'));
        let f: FileServerResponse = self.request_response::<FileServerResponse, ()>(Method::POST, &endpoint, None)?;
        Ok(response_to_entry(f))
    }

    fn delete_dir(&self, path: &str) -> Result<(), BackendError> {
        let endpoint = format!("api/directories/{}", path.trim_start_matches('/'));
        let resp=self.raw_request::<()>(Method::DELETE, &endpoint,None)?;
        match resp.status(){
            StatusCode::OK => Ok(()),
            _ => Err(self.decode_error(resp, &endpoint)),
        }
    }

    fn get_attr(&self, path: &str) -> Result<FileEntry, BackendError> {
        let endpoint = format!("api/files/attributes/{}", path.trim_start_matches('/'));
        let f: FileServerResponse = self.request_response::<FileServerResponse, ()>(Method::GET, &endpoint, None)?;
        Ok(response_to_entry(f))
    }

    fn create_file(&self, path: &str) -> Result<FileEntry, BackendError> {
        let endpoint = format!("api/files/{}", path.trim_start_matches('/'));
        let f: FileServerResponse = self.request_response::<FileServerResponse, ()>(Method::POST, &endpoint, None)?;
        Ok(response_to_entry(f))
    }

    fn delete_file(&self, path: &str) -> Result<(), BackendError> {
        let endpoint = format!("api/files/{}", path.trim_start_matches('/'));
        let resp=self.raw_request::<()>(Method::DELETE, &endpoint, None)?;
        match resp.status(){
            StatusCode::OK => Ok(()),
            _ => Err(self.decode_error(resp, &endpoint)),
        }
    }

    fn read_chunk(&self,path: &str, offset: u64, size: u64) -> Result<Vec<u8>, BackendError> {
        println!("Reading chunk from path: {}, offset: {}, size: {}", path, offset, size);
        let endpoint = format!("api/files/{}?offset={}&size={}", path.trim_start_matches('/'), offset, size);
        let resp= self.raw_request::<()>(Method::GET, &endpoint, None)?;
        match resp.status(){
            StatusCode::OK | StatusCode::PARTIAL_CONTENT => {
                let bytes = self.runtime
                    .block_on(async { resp.bytes().await })
                    .map_err(|e| BackendError::Other(e.to_string()))?;
                Ok(bytes.to_vec())
            }
            _ => Err(self.decode_error(resp, &endpoint)),
        }
    }

    fn write_chunk(&self, path: &str, offset: u64, data: Vec<u8>) -> Result<u64, BackendError> {
        let text = String::from_utf8_lossy(&data).to_string();
        let endpoint = format!("api/files/{}", path.trim_start_matches('/'));
        let body = serde_json::json!({ "offset": offset, "data": text });
        let resp: serde_json::Value = self.request_response(Method::PUT, &endpoint, Some(&body))?;
        Ok(resp["bytes"].as_u64().unwrap_or(0))
    }

    fn rename(&self, old_path: &str, new_path: &str) -> Result<FileEntry, BackendError> {
        let endpoint = format!("api/files/{}", old_path.trim_start_matches('/'));
        let body = serde_json::json!({ "new_path": new_path.trim_start_matches('/') });
        let f: FileServerResponse = self.request_response::<FileServerResponse, Value>(Method::PATCH, &endpoint, Some(&body))?;
        Ok(response_to_entry(f))
    }

    fn set_attr(&self,path: &str,attrs: SetAttrRequest) -> Result<FileEntry, BackendError> {
        let endpoint = format!("api/files/attributes/{}", path.trim_start_matches('/'));
        let body = serde_json::to_value(attrs).map_err(|e| BackendError::Other(e.to_string()))?;
        let f: FileServerResponse = self.request_response::<FileServerResponse, Value>(Method::PATCH, &endpoint, Some(&body))?;
        Ok(response_to_entry(f))
    }

    fn read_stream(&self, path: &str, offset: u64) -> Result<rfs_models::ByteStream, BackendError> {
        let endpoint = format!("api/files/stream/{}?offset={}", path.trim_start_matches('/'), offset);
        let resp= self.raw_request::<()>(Method::GET, &endpoint, None)?;
        match resp.status() {
            StatusCode::OK => {
                let stream=resp.bytes_stream().map(|r| r.map_err(|e| BackendError::Other(e.to_string())));
                return Ok(Box::pin(stream));
            },
            _ => Err(self.decode_error(resp, &endpoint)),
        }
    }
}
