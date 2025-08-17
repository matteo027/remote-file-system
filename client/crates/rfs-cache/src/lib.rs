use lru::LruCache;
use rfs_models::{RemoteBackend, FileEntry, BackendError, SetAttrRequest};
use std::sync::{Arc,Mutex};
use std::num::NonZeroUsize;
use std::path::Path;

type FilePageKey = (String, u64); // (path, page_idx)
const PAGE_SIZE: usize = 4096; // Dimensione della pagina in byte

fn page_index(offset: u64) -> u64 {
    offset / PAGE_SIZE as u64
}

fn page_start(idx: u64) -> u64 {
    idx * PAGE_SIZE as u64
}

fn pages_span(offset:u64, len:u64) -> (u64, u64) {
    let start = page_index(offset);
    let end = page_index(offset + len - 1);
    (start, end)
}


pub struct Cache <B:RemoteBackend>{
    // chiamata al backend remoto
    http_backend: B,
    // cache tra path e FileEntry, serve per get_attr e set_attr
    attr_cache: Arc<Mutex<LruCache<String, FileEntry>>>,
    // cache tra path e lista di entry del fs, serve per list_dir
    dir_cache: Arc<Mutex<LruCache<String, Vec<FileEntry>>>>,
    // cache tra fileChunk e i dati contenuti
    page_cache: Arc<Mutex<LruCache<FilePageKey, Vec<u8>>>>
}

impl <B:RemoteBackend> Cache<B> {
    pub fn new(http_backend: B, attr_cap: usize, dir_cap: usize, file_cap: usize) -> Self {
        Cache {
            http_backend,
            attr_cache: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(attr_cap).expect("attr_cap must be non-zero")))),
            dir_cache: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(dir_cap).expect("dir_cap must be non-zero")))),
            page_cache: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(file_cap).expect("file_cap must be non-zero")))),
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
        let mut page_cache = self.page_cache.lock().unwrap();
        let keys:Vec<FilePageKey>=page_cache.iter().filter_map(|(k,_)| (k.0 == path).then(|| (k.0.clone(), k.1))).collect();
        for key in keys {
            page_cache.pop(&key);
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
        let mut page_cache=self.page_cache.lock().unwrap();
        let to_remove: Vec<FilePageKey> = page_cache.iter().filter_map(|(k,_)| (k.0==path).then(|| (k.0.clone(), k.1))).collect();
        for k in to_remove{ 
            page_cache.pop(&k);
        }
        Ok(())
    }

    fn read_chunk(&self, path: &str, offset: u64, size: u64)-> Result<Vec<u8>, BackendError> {
        if size==0 { return Ok(Vec::new())}
        let mut out=Vec::with_capacity(size as usize);
        let (first, last) = pages_span(offset, size);
        let mut remaning = size as usize;
        for idx in first..=last {
            let page_off=page_start(idx);
            let maybe=self.page_cache.lock().unwrap().get(&(path.to_string(), idx)).cloned();
            let page=if let Some(data) = maybe {
                data
            }else{
                // Se il chunk non è in cache, lo leggo dal backend e lo metto
                let data= self.http_backend.read_chunk(path, page_off, PAGE_SIZE as u64)?;
                self.page_cache.lock().unwrap().put((path.to_string(), idx), data.clone());
                data
            };

            let start_in_page = if idx==first { 
                (offset - page_off) as usize 
            } else { 
                0 
            };
            if start_in_page >= page.len() {break};
            let avail = page.len() - start_in_page;
            let take = remaning.min(avail);
            out.extend_from_slice(&page[start_in_page..start_in_page + take]);
            remaning -= take;
            if remaning == 0 { break; }
        }
        Ok(out)
    }

    //write invalida solo la cache, sarà la read ssuccessiva a caricarla in cache se necessario
    fn write_chunk(&self, path: &str, offset: u64, data: Vec<u8>) -> Result<u64, BackendError> {
        let n = self.http_backend.write_chunk(path, offset, data.clone())?;
        println!("writing chunk {} at offset {} with size {}", path, offset, n);
        self.attr_cache.lock().unwrap().pop(path); // invalido la cache degli attributi
        if let Some(parent) = Path::new(path).parent().and_then(|p| p.to_str()) {
            // invalido la cache della directory padre
            self.dir_cache.lock().unwrap().pop(parent);
        }
        // invalida tutte le pagine con idx > offset
        let start_idx=page_index(offset);
        let mut pc = self.page_cache.lock().unwrap();
        let to_remove: Vec<FilePageKey>= pc.iter().filter_map(|(k,_)| (k.0==path && k.1 >=start_idx).then(|| (k.0.clone(),k.1))).collect();
        for k in to_remove{
            pc.pop(&k);
        }
        Ok(n)
    }

    fn rename(&self, old_path: &str, new_path: &str) -> Result<FileEntry, BackendError> {
        let entry = self.http_backend.rename(old_path, new_path)?;
        // Invalido la cache degli attributi per il vecchio e nuovo path
        self.attr_cache.lock().unwrap().pop(old_path);
        self.attr_cache.lock().unwrap().put(new_path.to_string(), entry.clone());

        for p in [old_path, new_path] {
            if let Some(parent) = Path::new(p).parent().and_then(|p| p.to_str()) {
                // invalido la cache della directory padre dei due path
                self.dir_cache.lock().unwrap().pop(parent);
            }
        }

        // Invalido la cache dei chunk di file
        let mut file_cache = self.page_cache.lock().unwrap();
        let to_remove: Vec<FilePageKey> = file_cache.iter().map(|(k, _)| k.clone()).filter(|(p, _)| p == old_path).collect();
        for key in to_remove {
            file_cache.pop(&key);
        }
        Ok(entry)
    }

    fn set_attr(&self, path: &str, attrs: SetAttrRequest) -> Result<FileEntry, BackendError> {
        let entry = self.http_backend.set_attr(path, attrs.clone())?;
        if let Some(parent) = Path::new(path).parent().and_then(|p| p.to_str()) {
            // invalido la cache della directory padre
            self.dir_cache.lock().unwrap().pop(parent);
        }
        // put fa già override sulla cache
        self.attr_cache.lock().unwrap().put(path.to_string(), entry.clone());
        if let Some(new_size) = attrs.size{
            let first_drop=page_index(new_size as u64);
            let mut pc=self.page_cache.lock().unwrap();
            let to_remove: Vec<FilePageKey>=pc.iter().filter_map(|(k,_)| (k.0==path && k.1 >=first_drop).then(|| (k.0.clone(),k.1))).collect();
            for k in to_remove{
                pc.pop(&k);
            }
        }
        Ok(entry)
    }
}