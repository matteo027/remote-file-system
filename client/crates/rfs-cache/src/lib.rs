use lru::LruCache;
use rfs_models::{RemoteBackend, FileEntry, BackendError, SetAttrRequest, BLOCK_SIZE};
use std::num::NonZeroUsize;
use std::time::SystemTime;
use rfs_models::ByteStream;
use std::sync::Arc;

type FileIno = u64;

pub struct Cache <B:RemoteBackend>{
    // chiamata al backend remoto
    http_backend: B,
    // cache tra ino e FileEntry, serve per get_attr e set_attr
    meta: LruCache<FileIno, Arc<FileEntry>>,
    // cache tra ino e lista dei figli (solo ino). Gli attributi dei figli sono in attr_cache
    dir_child: LruCache<FileIno, Arc<Vec<FileIno>>>,
    // mappa tra ino e cache dei blocchi del file, lru su idx del blocco e i dati
    file_blocks: LruCache<FileIno,LruCache<u64,Arc<Vec<u8>>>>,
    file_block_cap: NonZeroUsize // capacità massima della lru cache per ciascun file
}

#[inline]
fn block_span(offset:u64, len:u64) -> (u64,u64){
    let start = offset / BLOCK_SIZE as u64;
    let end = (offset + len.saturating_sub(1)) / BLOCK_SIZE as u64;
    (start, end)
}

impl <B:RemoteBackend> Cache<B> {
    pub fn new(http_backend: B, attr_cap: usize, dir_cap: usize, file_block_cap: usize, file_num: usize) -> Self {
        Cache {
            http_backend,
            meta: LruCache::new(NonZeroUsize::new(attr_cap).expect("attr_cap must be non-zero")),
            dir_child: LruCache::new(NonZeroUsize::new(dir_cap).expect("dir_cap must be non-zero")),
            file_blocks: LruCache::new(NonZeroUsize::new(file_num).expect("file_num must be non-zero")),
            file_block_cap: NonZeroUsize::new(file_block_cap).expect("file_block_cap must be non-zero"),
        }
    }

    #[inline]
    fn remember_meta(&mut self, entry: &FileEntry) {
        self.meta.put(entry.ino, Arc::new(entry.clone()));
    }

    #[inline]
    fn get_cached_mtime(&mut self, ino: u64) -> Option<SystemTime> {
        self.meta.get(&ino).map(|e| e.mtime)
    }

    #[inline]
    fn invalidate_blocks(&mut self, ino: u64) {
        self.file_blocks.pop(&ino);
    }

    #[inline]
    fn invalidate_dir_listing(&mut self, dir_ino: u64) {
        self.dir_child.pop(&dir_ino);
    }

    fn revalidate_meta(&mut self, ino:u64) -> Result<FileEntry, BackendError> {
        let since= self.get_cached_mtime(ino).unwrap_or(SystemTime::UNIX_EPOCH);
        match self.http_backend.get_attr_if_modified_since(ino, since)? {
            Some(entry) => {
                if let Some(prev) = self.get_cached_mtime(ino) {
                    if entry.mtime > prev {
                        self.invalidate_blocks(ino);
                    }
                }
                self.remember_meta(&entry);
                return Ok(entry);
            },
            None => {
                if let Some(cached) = self.meta.get(&ino) {
                    return Ok((**cached).clone());
                }
                else{
                    let entry = self.http_backend.get_attr(ino)?;
                    self.remember_meta(&entry);
                    return Ok(entry);
                }
            }
        }
    }

    fn get_or_create_file_lru(&mut self, ino: u64) -> &mut LruCache<u64, Arc<Vec<u8>>> {
        if !self.file_blocks.contains(&ino) {
            self.file_blocks.put(ino, LruCache::new(self.file_block_cap));
        }
        self.file_blocks.get_mut(&ino).unwrap()
    }

    fn read_block_aligned(&mut self, ino: u64, block_idx: u64) -> Result<Arc<Vec<u8>>, BackendError> {
        let off = block_idx * BLOCK_SIZE as u64;
        let buf = self.http_backend.read_chunk(ino, off, BLOCK_SIZE as u64)?;
        Ok(Arc::new(buf))
    }
}

impl <B:RemoteBackend> RemoteBackend for Cache<B> {
    fn list_dir(&mut self, ino: u64) -> Result<Vec<FileEntry>, BackendError> {
        // se abbiamo la lista in cache, usiamola

        if let Some(cached) = self.dir_child.get(&ino).cloned() {
            let mtime=self.get_cached_mtime(ino).unwrap_or(SystemTime::UNIX_EPOCH);
            match self.http_backend.get_attr_if_modified_since(ino, mtime)? {
                None => {
                    // proviamo a ricostruire la cache dai dati esistenti
                    let mut result = Vec::with_capacity(cached.len());
                    let mut missing=false;
                    for child_ino in cached.iter() {
                        if let Some(child_entry) = self.meta.get(child_ino).cloned() {
                            result.push((*child_entry).clone());
                        } else {
                            // se manca qualche metadato, dobbiamo rifare la lista
                            missing = true;
                            break;
                        }
                    }
                    if !missing {
                        return Ok(result);
                    }
                    self.dir_child.pop(&ino);
                }
                Some(_) => {
                    // la directory è cambiata, invalidiamo la cache
                    self.invalidate_dir_listing(ino);
                }
            }
        }
        // gestiamo il miss o il caso di cache invalida, richiamiamo il backend
        let entries = self.http_backend.list_dir(ino)?;
        for e in &entries {
            // facciamo un meccanismo di cache on write
            self.remember_meta(e);
        }

        let child_inos: Vec<u64> = entries.iter().map(|e| e.ino).collect();
        self.dir_child.put(ino, Arc::new(child_inos));
        Ok(entries)
    }

    fn get_attr(&mut self, ino: u64) -> Result<FileEntry, BackendError> {
        self.revalidate_meta(ino)
    }

    fn lookup(&mut self, parent_ino:u64, name:&str) -> Result<FileEntry, BackendError> {
        let res = self.http_backend.lookup(parent_ino, name)?;
        self.remember_meta(&res);
        Ok(res)
    }

    fn create_file(&mut self, parent_ino:u64, name:&str) -> Result<FileEntry, BackendError> {
        let res= self.http_backend.create_file(parent_ino, name)?;
        self.remember_meta(&res);
        self.invalidate_dir_listing(parent_ino);
        Ok(res)
    }

    fn create_dir(&mut self, parent_ino:u64, name:&str) -> Result<FileEntry, BackendError> {
        let res= self.http_backend.create_dir(parent_ino, name)?;
        self.remember_meta(&res);
        self.invalidate_dir_listing(parent_ino);
        Ok(res)
    }

    fn delete_file(&mut self, parent_ino:u64, name:&str) -> Result<(), BackendError> {
        self.http_backend.delete_file(parent_ino, name)?;
        self.invalidate_dir_listing(parent_ino);
        Ok(())
    }

    fn delete_dir(&mut self, parent_ino:u64, name:&str) -> Result<(), BackendError> {
        self.http_backend.delete_dir(parent_ino, name)?;
        self.invalidate_dir_listing(parent_ino);
        Ok(())
    }

    fn read_chunk(&mut self, ino: u64, offset: u64, size: u64)-> Result<Vec<u8>, BackendError> {
        let _ = self.revalidate_meta(ino)?; // assicuriamoci che il file sia aggiornato
        let (start_block, end_block) = block_span(offset, size);
        let mut result = Vec::with_capacity(size as usize);

        for block_idx in start_block..=end_block {
            let arc= if let Some(cached_block) = self.file_blocks.get_mut(&ino).and_then(|file_lru| file_lru.get(&block_idx)).cloned() {
                cached_block
            } else {
                let buf= self.read_block_aligned(ino, block_idx)?;
                let file_lru= self.get_or_create_file_lru(ino);
                file_lru.put(block_idx, buf.clone());
                buf
            };
            if arc.is_empty() {
                break; // EOF
            }

            let block_offset = block_idx * BLOCK_SIZE as u64;
            let start = offset.saturating_sub(block_offset).min(BLOCK_SIZE as u64) as usize;
            let mut end = ((offset + size).saturating_sub(block_offset)).min(BLOCK_SIZE as u64) as usize;
            if end > arc.len() {
                end = arc.len();
            }
            if start < end {
                result.extend_from_slice(&arc[start..end]);
            }
        }
        Ok(result)
    }

    fn write_chunk(&mut self, ino: u64, offset: u64, data: Vec<u8>) -> Result<u64, BackendError> {
        let bytes_written = self.http_backend.write_chunk(ino, offset, data.clone())?;
        let (start_block, end_block) = block_span(offset, bytes_written);
        if let Some(file_lru) = self.file_blocks.get_mut(&ino){
            for block_idx in start_block..=end_block {
                file_lru.pop(&block_idx);
            }
        }
        // forzo la rivalidazione dei metadati al prossimo accesso
        self.meta.pop(&ino);
        Ok(bytes_written)
    }

    fn rename(&mut self, old_parent_ino:u64, old_name: &str, new_parent_ino: u64, new_name: &str) -> Result<FileEntry, BackendError> {
        let res= self.http_backend.rename(old_parent_ino, old_name, new_parent_ino, new_name)?;
        self.remember_meta(&res);
        self.invalidate_dir_listing(old_parent_ino);
        if old_parent_ino != new_parent_ino {
            self.invalidate_dir_listing(new_parent_ino);
        }
        Ok(res)
    }

    fn set_attr(&mut self, ino:u64, attrs: SetAttrRequest) -> Result<FileEntry, BackendError> {
        let res= self.http_backend.set_attr(ino, attrs)?;
        if let Some(prev) = self.get_cached_mtime(ino) {
            if res.mtime > prev {
                self.invalidate_blocks(ino);
            }
        }
        self.remember_meta(&res);
        Ok(res)
    }

    fn read_stream(&mut self, ino: u64, offset: u64) -> Result<ByteStream, BackendError> {
        //passthrough
        self.http_backend.read_stream(ino, offset)
    }

    fn write_stream(&mut self, ino: u64, offset: u64, data: Vec<u8>) -> Result<(), BackendError> {
        //passthrough
        self.http_backend.write_stream(ino, offset, data)
    }

    fn link(&mut self, target_ino: u64, link_parent_ino: u64, link_name: &str) -> Result<FileEntry, BackendError> {
        let res= self.http_backend.link(target_ino, link_parent_ino, link_name)?;
        self.meta.pop(&target_ino); // il numero di link è cambiato
        self.remember_meta(&res);
        self.invalidate_dir_listing(link_parent_ino);
        Ok(res)
    }

    fn symlink(&mut self, target_path: &str, link_parent_ino: u64, link_name: &str) -> Result<FileEntry, BackendError> {
        let res = self.http_backend.symlink(target_path, link_parent_ino, link_name)?;
        self.remember_meta(&res);
        self.invalidate_dir_listing(link_parent_ino);
        Ok(res)
    }

    fn readlink(&mut self, ino: u64) -> Result<String, BackendError> {
        // DA VEDERE, forse si può fare caching
        self.http_backend.readlink(ino)
    }
}