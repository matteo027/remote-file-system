#![cfg(FALSE)] // per evitare che questo file venga compilato, al momento non serve

use lru::LruCache;
use rfs_models::{RemoteBackend, FileEntry, BackendError, SetAttrRequest, BLOCK_SIZE};
use std::num::NonZeroUsize;
use std::path::Path;
use rfs_models::ByteStream;

#[inline]
fn block_index(offset: u64) -> u64 {
    offset / BLOCK_SIZE as u64
}

#[inline]
fn block_start(idx: u64) -> u64 {
    idx * BLOCK_SIZE as u64
}

#[inline]
fn blocks_span(offset:u64, len:u64) -> (u64, u64) {
    let first = block_index(offset);
    let last = block_index(offset + len.saturating_sub(1));
    (first, last)
}

#[inline]
fn get_parent_str(path: &str) -> Option<&str> {
    Path::new(path).parent().and_then(|p| p.to_str())
}

// Serve per verificare se un path è sotto un certo prefisso, così possiamo invalidare la cache del prefisso
#[inline]
fn is_under_prefix(path: &str, prefix: &str) -> bool {
    Path::new(path).starts_with(Path::new(prefix))
}

type FileBlockIdx = u64;

pub struct Cache <B:RemoteBackend>{
    // chiamata al backend remoto
    http_backend: B,
    // cache tra path e FileEntry, serve per get_attr e set_attr
    attr_cache: LruCache<String, FileEntry>,
    // cache tra path e lista dei figli (solo path). Gli attributi dei figli sono in attr_cache
    dir_cache: LruCache<String, Vec<String>>,
    // mappa tra path e cache dei blocchi del file, lru su idx del blocco e i dati
    block_cache: LruCache<String,LruCache<FileBlockIdx,Vec<u8>>>,
    file_block_cap: NonZeroUsize // capacità massima della lru cache per ciascun file
}

impl <B:RemoteBackend> Cache<B> {
    pub fn new(http_backend: B, attr_cap: usize, dir_cap: usize, file_block_cap: usize, file_num: usize) -> Self {
        Cache {
            http_backend,
            attr_cache: LruCache::new(NonZeroUsize::new(attr_cap).expect("attr_cap must be non-zero")),
            dir_cache: LruCache::new(NonZeroUsize::new(dir_cap).expect("dir_cap must be non-zero")),
            block_cache: LruCache::new(NonZeroUsize::new(file_num).expect("block_cache must be non-zero")),
            file_block_cap: NonZeroUsize::new(file_block_cap).expect("file_block_cap must be non-zero"),
        }
    }

    fn invalidate_attr_and_parent(&mut self, path: &str){
        self.attr_cache.pop(path);
        if let Some(parent)=get_parent_str(path){
            self.attr_cache.pop(parent);
            self.dir_cache.pop(parent);
        }
    }

    fn invalidate_all_under_prefix(&mut self, prefix: &str){
        let to_remove: Vec<String> = self.attr_cache.iter().filter_map(|(k,_)| (is_under_prefix(k, prefix)).then(|| k.clone())).collect();
        for k in to_remove{
            self.attr_cache.pop(&k);
        }
        let to_remove: Vec<String> = self.dir_cache.iter().filter_map(|(k,_)| (is_under_prefix(k, prefix)).then(|| k.clone())).collect();
        for k in to_remove{
            self.dir_cache.pop(&k);
        }
        let to_remove: Vec<String> = self.block_cache.iter().filter_map(|(k,_)| (is_under_prefix(k, prefix)).then(|| k.clone())).collect();
        for k in to_remove{
            self.block_cache.pop(&k);
        }
    }

    fn invalidate_blocks_for(&mut self, path:&str, from_idx:Option<u64>,to_idx:Option<u64>){
        if let Some(file_cache)=self.block_cache.get_mut(path){
            let to_remove: Vec<u64> = file_cache.iter()
                .map(|(idx, _)| *idx)
                .filter(|idx| match (from_idx, to_idx) {
                    (Some(from), Some(to)) => *idx >= from && *idx <= to,
                    (Some(from), None)     => *idx >= from,
                    (None,   Some(to))     => *idx <= to,
                    (None,   None)         => true,
                })
                .collect();
            for idx in to_remove {
                file_cache.pop(&idx);
            }
            if file_cache.len() == 0 {
                // se non ci sono più blocchi, rimuovo tutta la cache del file
                self.block_cache.pop(path);
            }
        }
    }

    fn put_attr_and_invalidate_parent(&mut self, path: &str, entry: &FileEntry){
        self.attr_cache.put(path.to_string(), entry.clone());
        if let Some(parent)=get_parent_str(path){
            self.attr_cache.pop(parent);
            self.dir_cache.pop(parent);
        }
    }

    fn put_dir_listing(&mut self,dir:&str, entries: &[FileEntry]){
        let children: Vec<String> = entries.iter().map(|e| e.path.clone()).collect();
        self.dir_cache.put(dir.to_string(), children);
        // mettiamo o aggiorniamo anche gli attributi dei figli
        for entry in entries {
            self.attr_cache.put(entry.path.clone(), entry.clone());
        }
    }

    // se manca qualche attributo dei figli invalida la cache e ritorna None
    fn list_dir_from_cache(&mut self, dir: &str) -> Option<Vec<FileEntry>> {
        let children = self.dir_cache.get(dir)?.clone();
        let mut entries = Vec::with_capacity(children.len());
        let mut all_present = true;
        for child in children {
            if let Some(entry) = self.attr_cache.get(&child) {
                entries.push(entry.clone());
            } else {
                all_present = false;
                break;
            }
        }

        if all_present {
            Some(entries)
        } else {
            self.dir_cache.pop(dir);
            None
        }
    }

    fn fetch_block_if_missing(&mut self, path: &str, idx: u64) -> Result<(), BackendError> {
        if let Some(file_cache) = self.block_cache.get_mut(path) {
            if file_cache.peek(&idx).is_some() {
                return Ok(());
            }
        };


        let offset = block_start(idx);
        let data = self.http_backend.read_chunk(path, offset, BLOCK_SIZE as u64)?;
        let len = data.len();

        if self.block_cache.get_mut(path).is_none() {
            self.block_cache.put(path.to_string(), LruCache::new(self.file_block_cap));
        }
        let file_cache = self.block_cache.get_mut(path).unwrap();
        file_cache.put(idx, data);

        // Se il server ha restituito meno di BLOCK_SIZE, significa EOF entro questo blocco:
        // segna preventivamente il blocco successivo come vuoto (evita una futura chiamata inutile)
        if len < BLOCK_SIZE {
            if file_cache.peek(&(idx+1)).is_none() {
                file_cache.put(idx + 1, Vec::new());
            }
        }
        Ok(())
    }
}

impl <B:RemoteBackend> RemoteBackend for Cache<B> {
    fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>, BackendError> {
        if let Some(entries) = self.list_dir_from_cache(path) {
            return Ok(entries);
        }
        let fresh=self.http_backend.list_dir(path)?;
        self.put_dir_listing(path, &fresh);
        Ok(fresh)
    }

    fn get_attr(&mut self, path: &str) -> Result<FileEntry, BackendError> {
        if let Some(cached_entry) = self.attr_cache.get(path) {
            match self.http_backend.get_attr_if_modified_since(path, cached_entry.mtime) {
                Ok(Some(updated_entry)) => {
                    self.invalidate_blocks_for(path, None, None);
                    self.put_attr_and_invalidate_parent(path, &updated_entry);
                    return Ok(updated_entry);
                },
                Ok(None) => return Ok(cached_entry.clone()),
                Err(e) => return Err(e),
            }
        }
        else{
            let fresh_entry = self.http_backend.get_attr(path)?;
            self.put_attr_and_invalidate_parent(path, &fresh_entry);
            return Ok(fresh_entry);
        }
    }

    // Invalido solo la cache del padre, non carico i nuovi attributi del file nella cache directory
    fn create_file(&mut self, path: &str) -> Result<FileEntry, BackendError> {
        let entry = self.http_backend.create_file(path)?;
        self.put_attr_and_invalidate_parent(path, &entry);
        Ok(entry)
    }

    fn create_dir(&mut self, path: &str) -> Result<FileEntry, BackendError> {
        let entry= self.http_backend.create_dir(path)?;
        self.put_attr_and_invalidate_parent(path, &entry);
        Ok(entry)
    }

    fn delete_file(&mut self, path: &str) -> Result<(), BackendError> {
        self.http_backend.delete_file(path)?;
        self.invalidate_blocks_for(path, None, None);
        self.invalidate_attr_and_parent(path);
        Ok(())
    }

    fn delete_dir(&mut self, path: &str) -> Result<(), BackendError> {
        self.http_backend.delete_dir(path)?;
        // invalido tutto sotto la cartella rimossa
        self.invalidate_all_under_prefix(path);
        self.invalidate_attr_and_parent(path);
        Ok(())
    }

    // DA CONTROLLARE ATTENTAMENTE
    fn read_chunk(&mut self, path: &str, offset: u64, size: u64)-> Result<Vec<u8>, BackendError> {
        if size == 0 {
            return Ok(Vec::new());
        }

        let (first, last) = blocks_span(offset, size);
        let req_end = offset + size;
        let mut out = Vec::with_capacity(size as usize);
        let mut remaining = size as usize;
        for idx in first..=last {
            self.fetch_block_if_missing(path, idx)?;
            let file_cache = match self.block_cache.get_mut(path) {
                Some(cache) => cache,
                None => return Err(BackendError::Other("cache invariant broken".to_string())),
            };
            let block = match file_cache.get(&idx){
                Some(vec) => vec,
                None => { return Err(BackendError::Other("cache invariant broken".to_string())); }
            };

            if block.is_empty() {
                // EOF raggiunto
                break;
            }

            let block_off = block_start(idx);
            //start_in_block = offset - block_start if first block, else 0
            let start_in_block=offset.saturating_sub(block_off) as usize;
            let end_in_block = (req_end.saturating_sub(block_off) as usize).min(block.len());

            // Copia solo la porzione richiesta e non oltrepassare `remaining`
            let want = end_in_block - start_in_block;
            let take = want.min(remaining);

            out.extend_from_slice(&block[start_in_block..start_in_block + take]);
            remaining -= take;

            if remaining == 0 {
                break;
            }
        }
        Ok(out)
    }

    //write invalida solo la cache, sarà la read ssuccessiva a caricarla in cache se necessario
    fn write_chunk(&mut self, path: &str, offset: u64, data: Vec<u8>) -> Result<u64, BackendError> {
        let nwritten = self.http_backend.write_chunk(path, offset, data)?;
        if nwritten > 0{
            let (a,b)=blocks_span(offset, nwritten);
            self.invalidate_blocks_for(path, Some(a), Some(b));
        }
        self.invalidate_attr_and_parent(path);
        Ok(nwritten)
    }

    fn rename(&mut self, old_path: &str, new_path: &str) -> Result<FileEntry, BackendError> {
        let entry = self.http_backend.rename(old_path, new_path)?;
        // invalido tutto per sicurezza
        self.invalidate_all_under_prefix(old_path);
        self.invalidate_attr_and_parent(old_path);
        self.put_attr_and_invalidate_parent(new_path, &entry);
        Ok(entry)
    }

    fn set_attr(&mut self, path: &str, attrs: SetAttrRequest) -> Result<FileEntry, BackendError> {
        let entry = self.http_backend.set_attr(path, attrs.clone())?;
        
        self.put_attr_and_invalidate_parent(path, &entry);
        if let Some(new_size) = attrs.size{
            let drop_from=block_index(new_size as u64);
            self.invalidate_blocks_for(path, Some(drop_from), None);
        }
        Ok(entry)
    }

    fn read_stream(&mut self, path: &str, offset: u64) -> Result<ByteStream, BackendError> {
        self.http_backend.read_stream(path, offset)
    }
    
    fn write_stream(&mut self, path: &str, offset: u64, data: Vec<u8>) -> Result<(), BackendError> {
        self.http_backend.write_stream(path, offset, data)
    }
}