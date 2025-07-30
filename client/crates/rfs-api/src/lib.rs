use reqwest::cookie::Jar;
use reqwest::{Client, Method, StatusCode, Url};
use rfs_models::{BackendError, FileChunk, FileEntry, RemoteBackend, SetAttrRequest};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;

#[derive(Deserialize, Debug)]
struct ErrorResponse {
    error: String,
}

#[derive(Serialize)]
struct LoginPayload {
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

pub struct Server {
    runtime: Runtime, // from tokio, used to manage async calls
    base_url: Url,
    client: Client,
}

fn deserialize_systemtime_from_millis<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
where
    D: Deserializer<'de>,
{
    let millis: u64 = Deserialize::deserialize(deserializer)?;
    Ok(UNIX_EPOCH + Duration::from_millis(millis))
}

impl Server {
    pub fn new() -> Self {
        Self {
            runtime: Runtime::new().expect("Unable to built a Runtime object"),
            base_url: Url::from_str("http://localhost:3000/").unwrap(), // meglio passarlo come parametro la metodo (?)
            client: {
                let cookie_jar = Arc::new(Jar::default());
                // Build client with the cookie jar
                reqwest::Client::builder()
                    .cookie_provider(Arc::clone(&cookie_jar))
                    .build()
                    .expect("Unable to build the Client object")
            },
        }
    }

    fn check_and_authenticate(&mut self) -> Result<(), BackendError> {
        let client = self.client.clone();
        let address = self.base_url.clone();

        // Spawn a new OS thread to handle the async login workflow
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Unable to geenrate e tokio Runtime");

            rt.block_on(async move {
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
                    let login_url = address.join("api/login").unwrap();
                    let body = LoginPayload {
                        username: "5000".into(),
                        password: "admin".into(),
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
                    }

                    if resp_login.status() == StatusCode::OK {
                        // Step 3: optionally verify with /api/me again
                        let verify = client
                            .get(me_url)
                            .send()
                            .await
                            .map_err(|e| BackendError::Other(e.to_string()))?;
                        if verify.status() == StatusCode::OK {
                            return Ok(());
                        } else {
                            return Err(BackendError::Unauthorized);
                        }
                    } else {
                        return Err(BackendError::Unauthorized);
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
            is_dir: file.ty == 1, //TO DO: implement type conversion
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

    fn request<R: DeserializeOwned + 'static>(&mut self, method: Method, endpoint: &str) -> Result<R, BackendError> {

        let url = self.base_url.join(endpoint).map_err(|e| BackendError::Other(e.to_string()))?;
        let req = self.client.request(method, url);
        let resp = self.runtime.block_on(async {req.send().await.map_err(|e| BackendError::Other(e.to_string()))})?;
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

    fn request_with_body<R: DeserializeOwned + 'static, B: Serialize>(&mut self,method: Method,endpoint: &str,body: &B) -> Result<R, BackendError> {
        let url = self.base_url.join(endpoint).map_err(|e| BackendError::Other(e.to_string()))?;
        let req = self.client.request(method, url).json(body);
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
}

impl RemoteBackend for Server {
    fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>, BackendError> {
        self.check_and_authenticate()?;
        let endpoint = format!("api/directories/{}", path.trim_start_matches('/'));
        let files: Vec<FileServerResponse> = self.request(Method::GET, &endpoint)?;
        Ok(files.into_iter().map(Self::response_to_entry).collect())
    }

    fn create_dir(&mut self, path: &str) -> Result<FileEntry, BackendError> {
        self.check_and_authenticate()?;
        let endpoint = format!("api/directories/{}", path.trim_start_matches('/'));
        let f: FileServerResponse = self.request(Method::POST, &endpoint)?;
        Ok(Self::response_to_entry(f))
    }

    fn delete_dir(&mut self, path: &str) -> Result<(), BackendError> {
        self.check_and_authenticate()?;
        let endpoint = format!("api/directories/{}", path.trim_start_matches('/'));
        let _: serde_json::Value = self.request(Method::DELETE, &endpoint)?;
        Ok(())
    }

    fn get_attr(&mut self, path: &str) -> Result<FileEntry, BackendError> {
        self.check_and_authenticate()?;
        let endpoint = format!("api/files/attributes/{}", path.trim_start_matches('/'));
        let f: FileServerResponse = self.request(Method::GET, &endpoint)?;
        Ok(Self::response_to_entry(f))
    }

    fn create_file(&mut self, path: &str) -> Result<FileEntry, BackendError> {
        self.check_and_authenticate()?;
        let endpoint = format!("api/files/{}", path.trim_start_matches('/'));
        let f: FileServerResponse = self.request(Method::POST, &endpoint)?;
        Ok(Self::response_to_entry(f))
    }

    fn delete_file(&mut self, path: &str) -> Result<(), BackendError> {
        self.check_and_authenticate()?;
        let endpoint = format!("api/files/{}", path.trim_start_matches('/'));
        let _: serde_json::Value = self.request(Method::DELETE, &endpoint)?;
        Ok(())
    }

    #[allow(unused_variables)]
    fn read_chunk(&mut self,path: &str,offset: u64,size: u64) -> Result<FileChunk, BackendError> {
        todo!()
    }

    #[allow(unused_variables)]
    fn write_chunk(&mut self, path: &str, offset: u64, data: Vec<u8>) -> Result<u64, BackendError> {
        todo!()
    }

    #[allow(unused_variables)]
    fn rename(&mut self, old_path: &str, new_path: &str) -> Result<FileEntry, BackendError> {
        todo!()
    }

    fn set_attr(&mut self,path: &str,attrs: SetAttrRequest) -> Result<FileEntry, BackendError> {
        self.check_and_authenticate()?;
        let endpoint = format!("api/files/attributes/{}", path.trim_start_matches('/'));
        let body = serde_json::to_value(attrs).map_err(|e| BackendError::Other(e.to_string()))?;
        let f: FileServerResponse = self.request_with_body(Method::PATCH, &endpoint, &body)?;
        
        Ok(Self::response_to_entry(f))
    }
}
