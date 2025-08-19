use lru::LruCache;
use rfs_models::{RemoteBackend, FileEntry, BackendError, SetAttrRequest};
use std::sync::{Arc,Mutex};
use std::num::NonZeroUsize;
use std::path::Path;
use rfs_models::ByteStream;

type FileChunkKey = (String, u64, u64); // (path, offset, length)

// implementato meccanismo di cache on write tranne sulla risoluzione di list_dir dopo una creazione/rimozione

pub struct Cache <B:RemoteBackend>{
    // chiamata al backend remoto
    http_backend: B,
    // cache tra path e FileEntry, serve per get_attr e set_attr
    attr_cache: Arc<Mutex<LruCache<String, FileEntry>>>,
    // cache tra path e lista di entry del fs, serve per list_dir
    dir_cache: Arc<Mutex<LruCache<String, Vec<FileEntry>>>>,
    // cache tra fileChunk e i dati contenuti
    file_chunk_cache: Arc<Mutex<LruCache<FileChunkKey, Vec<u8>>>>
}

impl <B:RemoteBackend> Cache<B> {
    pub fn new(http_backend: B, attr_cap: usize, dir_cap: usize, file_cap: usize) -> Self {
        Cache {
            http_backend,
            attr_cache: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(attr_cap).expect("attr_cap must be non-zero")))),
            dir_cache: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(dir_cap).expect("dir_cap must be non-zero")))),
            file_chunk_cache: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(file_cap).expect("file_cap must be non-zero")))),
        }
    }
}

impl <B:RemoteBackend> RemoteBackend for Cache<B> {
    fn list_dir(&self, path: &str) -> Result<Vec<FileEntry>, BackendError> {
        let mut cache = self.dir_cache.lock().unwrap();
        if let Some(entries) = cache.get(path) {
            return Ok(entries.clone());
        }
        drop(cache); // Rilascia il lock prima di chiamare il backend
        let entries = self.http_backend.list_dir(path)?;

        // riprendo la lock dopo aver chiamato il backend
        self.dir_cache.lock().unwrap().put(path.to_string(), entries.clone());
        let mut attr_cache = self.attr_cache.lock().unwrap();
        // Cache anche gli attributi di ogni file nella directory
        for entry in &entries {
            attr_cache.put(entry.path.clone(), entry.clone());
        }
        Ok(entries)
    }

    fn get_attr(&self, path: &str) -> Result<FileEntry, BackendError> {
        let mut cache = self.attr_cache.lock().unwrap();
        if let Some(entry) = cache.get(path) {
            return Ok(entry.clone());
        }
        drop(cache); // Rilascia il lock prima di chiamare il backend
        let entry = self.http_backend.get_attr(path)?;
        self.attr_cache.lock().unwrap().put(path.to_string(), entry.clone());
        Ok(entry)
    }

    // Invalido solo la cache del padre, non carico i nuovi attributi del file nella cache directory
    fn create_file(&self, path: &str) -> Result<FileEntry, BackendError> {
        let entry = self.http_backend.create_file(path)?;
        if let Some(parent) = Path::new(path).parent().and_then(|p| p.to_str()) {
            // Aggiorno la cache della directory padre
            self.dir_cache.lock().unwrap().pop(parent);
        }
        self.attr_cache.lock().unwrap().put(path.to_string(), entry.clone());
        Ok(entry)
    }

    fn create_dir(&self, path: &str) -> Result<FileEntry, BackendError> {
        let entry= self.http_backend.create_dir(path)?;
        if let Some(parent) = Path::new(path).parent().and_then(|p| p.to_str()) {
            // invalido la cache della directory padre
            self.dir_cache.lock().unwrap().pop(parent);
        }
        self.attr_cache.lock().unwrap().put(path.to_string(), entry.clone());
        Ok(entry)
    }

    fn delete_file(&self, path: &str) -> Result<(), BackendError> {
        self.http_backend.delete_file(path)?;
        self.attr_cache.lock().unwrap().pop(path);
        if let Some(parent) = Path::new(path).parent().and_then(|p| p.to_str()) {
            // invalido la cache della directory padre
            self.dir_cache.lock().unwrap().pop(parent);
        }

        // Rimuovo anche eventuali chunk di file dalla cache
        let mut file_chunk_cache = self.file_chunk_cache.lock().unwrap();
        let keys:Vec<FileChunkKey>=file_chunk_cache.iter().map(|(k,_)|k.clone()).filter(|(p,_,_)| p==path).collect();
        for key in keys {
            file_chunk_cache.pop(&key);
        }
        Ok(())
    }

    fn delete_dir(&self, path: &str) -> Result<(), BackendError> {
        self.http_backend.delete_dir(path)?;
        self.attr_cache.lock().unwrap().pop(path);
        if let Some(parent) = Path::new(path).parent().and_then(|p| p.to_str()) {
            // invalido la cache della directory padre
            self.dir_cache.lock().unwrap().pop(parent);
        }
        Ok(())
    }

    fn read_chunk(&self, path: &str, offset: u64, size: u64)-> Result<Vec<u8>, BackendError> {
        let key = (path.to_string(), offset, size);
        let mut cache = self.file_chunk_cache.lock().unwrap();
        if let Some(data) = cache.get(&key) {
            return Ok(data.clone());
        }
        drop(cache); // Rilascia il lock prima di chiamare il backend
        let data = self.http_backend.read_chunk(path, offset, size)?;
        // riprendo la lock dopo aver chiamato il backend
        self.file_chunk_cache.lock().unwrap().put(key, data.clone());
        Ok(data)
    }

    fn write_chunk(&self, path: &str, offset: u64, data: Vec<u8>) -> Result<u64, BackendError> {
        let n = self.http_backend.write_chunk(path, offset, data.clone())?;
        self.attr_cache.lock().unwrap().pop(path); // invalido la cache degli attributi

        //Invalido la cache di ciò che era stato scritto in precedenza
        let mut file_cache = self.file_chunk_cache.lock().unwrap();
        let keys: Vec<FileChunkKey> = file_cache.iter().map(|(k, _)| k.clone()).filter(|(p, off, sz)| p == path && *off < offset + n && offset < *off + *sz as u64).collect();
        for key in keys {
            file_cache.pop(&key);
        }
        // Aggiungo il nuovo chunk scritto nella cache
        file_cache.put((path.to_string(), offset, n), data);
        Ok(n)
    }

    fn rename(&self, old_path: &str, new_path: &str) -> Result<FileEntry, BackendError> {
        let entry = self.http_backend.rename(old_path, new_path)?;
        // Invalido la cache degli attributi per il vecchio e nuovo path
        self.attr_cache.lock().unwrap().pop(old_path);

        for p in [old_path, new_path] {
            if let Some(parent) = Path::new(p).parent().and_then(|p| p.to_str()) {
                // invalido la cache della directory padre dei due path
                self.dir_cache.lock().unwrap().pop(parent);
            }
        }

        // Invalido la cache dei chunk di file
        let mut file_cache = self.file_chunk_cache.lock().unwrap();
        let keys: Vec<FileChunkKey> = file_cache.iter().map(|(k, _)| k.clone()).filter(|(p, _, _)| p == old_path).collect();
        for key in keys {
            file_cache.pop(&key);
        }
        Ok(entry)
    }

    fn set_attr(&self, path: &str, attrs: SetAttrRequest) -> Result<FileEntry, BackendError> {
        let entry = self.http_backend.set_attr(path, attrs)?;
        // put fa già override sulla cache
        self.attr_cache.lock().unwrap().put(path.to_string(), entry.clone());
        Ok(entry)
    }

    fn read_stream(&self, path: &str, offset: u64) -> Result<ByteStream, BackendError> {
        self.http_backend.read_stream(path, offset)
    }
}