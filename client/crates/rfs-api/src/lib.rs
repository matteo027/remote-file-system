use rfs_models::{RemoteBackend,DirectoryEntry, BackendError};
use std::str::FromStr;
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
    client: Client,
    token: String
}

#[derive(Serialize)]
struct DirApisPayload {
    path: String
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

    fn list_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>, BackendError> {
        let key = if path == "/" { "".to_string() } else { path.to_string() };
        match self.dirs.get(&key) {
        Some(v) => Ok(v.clone()),
        None    => Err(BackendError::NotFound(path.to_string())),
        }
    }
}

impl RemoteBackend for Server {
    fn new() -> Self
    where
        Self: Sized
    {
        Self {
            runtime: Runtime::new().unwrap(),
            address: Url::from_str("http://127.0.0.1:3000/").unwrap(), // meglio passarlo come parametro la metodo (?)
            client: reqwest::Client::new(),
            token: String::from("")
        }
    }

    fn list_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>, BackendError> {

        let api_result = self.runtime.block_on(async {
            let request_url = self.address.clone()
                .join("api/directories").unwrap()
                .join(path.strip_prefix('/').unwrap_or(path)).unwrap();
            println!("url built: {}", request_url);
            
            let resp = self.client
                .get(String::from(request_url))
                .bearer_auth(&self.token)
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
                .join("/api/directories").unwrap()
                .join(Path::new(&entry.name).file_name().unwrap_or_default().to_str().unwrap_or("")).unwrap();
            
            let resp =self.client
                .post(request_url)
                .bearer_auth(&self.token)
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
                .join("/api/directories").unwrap()
                .join(Path::new(path).file_name().unwrap_or_default().to_str().unwrap_or("")).unwrap();
            
            let resp =self.client
                .delete(request_url)
                .bearer_auth(&self.token)
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
}