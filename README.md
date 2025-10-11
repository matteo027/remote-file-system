# Remote File System

A remote filesystem implementation for Windows (using WinFSP), Linux (using fuser) and MacOS (using macFUSE), with a Node.js server backend and Rust client.

Un'implementazione di filesystem remoto per Windows (usando WinFSP), Linux (usando fuser) e MacOS (usando macFUSE), con server backend Node.js e client Rust.

---

# ENGLISH

## How to run it

Server: `npm run dev`
Client: `cargo run -- -r /path/to/local/dir` for Unix systems, or your disk like `-r X:`, in case of Windows system

## FileSystem API

All routes require authentication (middleware `isLoggedIn`).

### Authentication

#### POST /api/login

**Description:**  
Log in with local authentication (session).

**Request body (JSON):**
```json
{
  "uid": 5000,
  "password": "your-password"
}
```

**Returns:**
Authenticated user ID.

**Return type (JSON):**
```json
5000
```

---

#### POST /api/logout

**Description:**  
Ends the active user session.

**Body:** None

**Returns:** Status `200 OK` without content.

---

#### GET /api/me

**Description:**  
Returns the data of the currently authenticated user.

**Body:** None

**Returns:**  
Authenticated user data.

**Return type (JSON):**
```json
{
  "uid": 5000
}
```

---

### File and Directory Attributes

#### GET /api/files/{ino}/attributes

**Description:**
Returns the metadata of a file or directory by inode number.

**URL parameters:**
- `ino` (string): inode number of the file or directory

**Body:** None

**Returns:**  
File or directory metadata.

**Return type (JSON):**
```json
{
  "ino": "12345",
  "name": "example.txt",
  "kind": 0,
  "size": 1245,
  "uid": 1000,
  "gid": 1000,
  "perm": 420,
  "atime": 1690963200000,
  "mtime": 1690963200000,
  "ctime": 1690963200000,
  "btime": 1690963200000
}
```

---

#### PATCH /api/files/{ino}/attributes

**Description:**  
Updates one or more attributes of a file or directory, such as permissions or size.

**URL parameters:**
- `ino` (string): inode number of the file or directory

**Body (JSON):**
```json
{
  "perm": 644,
  "size": 1000
}
```

**Returns:**
Updated metadata for the file or directory.

---

### Directory Operations

#### GET /api/directories/{parentIno}/entries/lookup

**Description:**  
Looks up a specific entry in a directory by name.

**URL parameters:**
- `parentIno` (string): inode number of the parent directory

**Query parameters:**
- `name` (string): name of the file or directory to lookup

**Returns:**
Metadata of the found entry.

**Return type (JSON):**
```json
{
  "ino": "12345",
  "name": "example.txt",
  "kind": 0,
  "size": 1024,
  "uid": 5000,
  "gid": 5000,
  "perm": 420,
  "atime": 1699999999999,
  "mtime": 1699999999999,
  "ctime": 1699999999999,
  "btime": 1699999999999
}
```

---

#### GET /api/directories/{ino}/entries

**Description:**  
Returns the list of files and directories contained in a directory.

**URL parameters:**
- `ino` (string): inode number of the directory

**Body:** None

**Returns:**  
List of entries in the directory.

**Return type (JSON):**
```json
[
  {
    "ino": "12345",
    "name": "notes.txt",
    "kind": 0,
    "size": 1024,
    "uid": 5000,
    "gid": 5000,
    "perm": 420,
    "atime": 1699999999999,
    "mtime": 1699999999999,
    "ctime": 1699999999999,
    "btime": 1699999999999
  },
  {
    "ino": "67890",
    "name": "documents",
    "kind": 1,
    "size": 4096,
    "uid": 5000,
    "gid": 5000,
    "perm": 755,
    "atime": 1399999999999,
    "mtime": 1399999999999,
    "ctime": 1399999999999,
    "btime": 1399999999999
  }
]
```

---

### File and Directory Creation/Deletion

#### POST /api/directories/{parentIno}/dirs/{name}

**Description:**  
Creates a new directory.

**URL parameters:**
- `parentIno` (string): inode number of the parent directory
- `name` (string): name of the new directory

**Returns:**
Metadata of the created directory.

---

#### DELETE /api/directories/{parentIno}/dirs/{name}

**Description:**  
Deletes a directory.

**URL parameters:**
- `parentIno` (string): inode number of the parent directory
- `name` (string): name of the directory to delete

**Returns:**
Confirmation of deletion.

---

#### POST /api/directories/{parentIno}/files/{name}

**Description:**  
Creates a new file.

**URL parameters:**
- `parentIno` (string): inode number of the parent directory
- `name` (string): name of the new file

**Returns:**
Metadata of the created file.

---

#### DELETE /api/directories/{parentIno}/files/{name}

**Description:**  
Deletes a file.

**URL parameters:**
- `parentIno` (string): inode number of the parent directory
- `name` (string): name of the file to delete

**Returns:**
Confirmation of deletion.

---

### Rename Operations

#### PATCH /api/directories/{oldParentIno}/entries/{oldName}

**Description:**  
Renames or moves a file or directory.

**URL parameters:**
- `oldParentIno` (string): inode number of the old parent directory
- `oldName` (string): current name of the file/directory

**Body (JSON):**
```json
{
  "newParentIno": "67890",
  "newName": "new_filename.txt"
}
```

**Returns:**
Updated metadata of the renamed/moved entry.

---

### File Read/Write Operations

#### GET /api/files/{ino}

**Description:**
Reads the contents of a file.

**URL parameters:**
- `ino` (string): inode number of the file

**Query parameters:**
- `offset` (optional): byte offset to start reading from
- `size` (optional): number of bytes to read

**Returns:**
File contents as binary data.

---

#### PUT /api/files/{ino}

**Description:**
Writes binary content to a file.

**URL parameters:**
- `ino` (string): inode number of the file

**Headers:**
- `Content-Type: application/octet-stream`
- `X-Chunk-Offset` (optional): byte offset to start writing from

**Body:**
Binary content to write.

**Returns:**
Number of bytes written.

**Return type (JSON):**
```json
{
  "bytes": 1024
}
```

---

#### GET /api/files/stream/{ino}

**Description:**
Streams file contents for large files.

**URL parameters:**
- `ino` (string): inode number of the file

**Returns:**
File contents as a stream.

---

#### PUT /api/files/stream/{ino}

**Description:**
Streams binary content to a file for large uploads.

**URL parameters:**
- `ino` (string): inode number of the file

**Body:**
Binary stream to write.

**Returns:**
Upload confirmation.

---

### Link Operations

#### POST /api/links/{targetIno}

**Description:**  
Creates a hard link to an existing file.

**URL parameters:**
- `targetIno` (string): inode number of the target file

**Body (JSON):**
```json
{
  "linkParentIno": "12345",
  "linkName": "link_name"
}
```

**Returns:**
Metadata of the created hard link.

---

#### POST /api/symlinks

**Description:**  
Creates a symbolic link.

**Body (JSON):**
```json
{
  "linkParenIno": "12345",
  "linkName": "symlink_name",
  "targetPath": "/path/to/target"
}
```

**Returns:**
Metadata of the created symbolic link.

---

#### GET /api/symlinks/{ino}

**Description:**  
Reads the target path of a symbolic link.

**URL parameters:**
- `ino` (string): inode number of the symbolic link

**Returns:**
Target path of the symbolic link.

**Return type (JSON):**
```json
{
  "target": "/path/to/target"
}
```

---

### System Information

#### GET /api/size

**Description:**  
Returns filesystem size information (used by Windows systems only!).

**Body:** None

**Returns:**
Total and available space information.

**Return type (JSON):**
```json
{
  "total": 107374182400,
  "available": 85899345920
}
```

---

### Data Types

#### File Entry Types
- `0`: Regular file
- `1`: Directory  
- `2`: Symbolic link

#### Permissions
Unix-style octal permissions (e.g., `755`, `644`, `420`)

#### Timestamps
All timestamps are in milliseconds since Unix epoch (JavaScript `Date.now()` format)

---

### Error Responses

All endpoints may return the following error responses:

**401 Unauthorized:**
```json
{
  "error": "Authentication required"
}
```

**403 Forbidden:**
```json
{
  "error": "Permission denied"
}
```

**404 Not Found:**
```json
{
  "error": "File or directory not found"
}
```

**500 Internal Server Error:**
```json
{
  "error": "Internal server error",
  "details": "Error message"
}
```

---

# ITALIANO

## Come eseguirlo

Server: `npm run dev`
Client: `cargo run -- -r /path/to/local/dir` per sistemi Unix, oppure il tuo disco come `-r X:`, nel caso di sistemi Windows

## API FileSystem

Tutte le route richiedono autenticazione (middleware `isLoggedIn`).

### Autenticazione

#### POST /api/login

**Descrizione:**  
Accedi con autenticazione locale (sessione).

**Corpo della richiesta (JSON):**
```json
{
  "uid": 5000,
  "password": "tua-password"
}
```

**Restituisce:**
ID utente autenticato.

**Tipo di ritorno (JSON):**
```json
5000
```

---

#### POST /api/logout

**Descrizione:**  
Termina la sessione utente attiva.

**Corpo:** Nessuno

**Restituisce:** Status `200 OK` senza contenuto.

---

#### GET /api/me

**Descrizione:**  
Restituisce i dati dell'utente attualmente autenticato.

**Corpo:** Nessuno

**Restituisce:**  
Dati dell'utente autenticato.

**Tipo di ritorno (JSON):**
```json
{
  "uid": 5000
}
```

---

### Attributi di File e Directory

#### GET /api/files/{ino}/attributes

**Descrizione:**
Restituisce i metadati di un file o directory tramite numero inode.

**Parametri URL:**
- `ino` (string): numero inode del file o directory

**Corpo:** Nessuno

**Restituisce:**  
Metadati del file o directory.

**Tipo di ritorno (JSON):**
```json
{
  "ino": "12345",
  "name": "esempio.txt",
  "kind": 0,
  "size": 1245,
  "uid": 1000,
  "gid": 1000,
  "perm": 420,
  "atime": 1690963200000,
  "mtime": 1690963200000,
  "ctime": 1690963200000,
  "btime": 1690963200000
}
```

---

#### PATCH /api/files/{ino}/attributes

**Descrizione:**  
Aggiorna uno o più attributi di un file o directory, come permessi o dimensione.

**Parametri URL:**
- `ino` (string): numero inode del file o directory

**Corpo (JSON):**
```json
{
  "perm": 644,
  "size": 1000
}
```

**Restituisce:**
Metadati aggiornati per il file o directory.

---

### Operazioni su Directory

#### GET /api/directories/{parentIno}/entries/lookup

**Descrizione:**  
Cerca una voce specifica in una directory per nome.

**Parametri URL:**
- `parentIno` (string): numero inode della directory padre

**Parametri Query:**
- `name` (string): nome del file o directory da cercare

**Restituisce:**
Metadati della voce trovata.

**Tipo di ritorno (JSON):**
```json
{
  "ino": "12345",
  "name": "esempio.txt",
  "kind": 0,
  "size": 1024,
  "uid": 5000,
  "gid": 5000,
  "perm": 420,
  "atime": 1699999999999,
  "mtime": 1699999999999,
  "ctime": 1699999999999,
  "btime": 1699999999999
}
```

---

#### GET /api/directories/{ino}/entries

**Descrizione:**  
Restituisce l'elenco di file e directory contenuti in una directory.

**Parametri URL:**
- `ino` (string): numero inode della directory

**Corpo:** Nessuno

**Restituisce:**  
Elenco delle voci nella directory.

**Tipo di ritorno (JSON):**
```json
[
  {
    "ino": "12345",
    "name": "note.txt",
    "kind": 0,
    "size": 1024,
    "uid": 5000,
    "gid": 5000,
    "perm": 420,
    "atime": 1699999999999,
    "mtime": 1699999999999,
    "ctime": 1699999999999,
    "btime": 1699999999999
  },
  {
    "ino": "67890",
    "name": "documenti",
    "kind": 1,
    "size": 4096,
    "uid": 5000,
    "gid": 5000,
    "perm": 755,
    "atime": 1399999999999,
    "mtime": 1399999999999,
    "ctime": 1399999999999,
    "btime": 1399999999999
  }
]
```

---

### Creazione/Eliminazione di File e Directory

#### POST /api/directories/{parentIno}/dirs/{name}

**Descrizione:**  
Crea una nuova directory.

**Parametri URL:**
- `parentIno` (string): numero inode della directory padre
- `name` (string): nome della nuova directory

**Restituisce:**
Metadati della directory creata.

---

#### DELETE /api/directories/{parentIno}/dirs/{name}

**Descrizione:**  
Elimina una directory.

**Parametri URL:**
- `parentIno` (string): numero inode della directory padre
- `name` (string): nome della directory da eliminare

**Restituisce:**
Conferma dell'eliminazione.

---

#### POST /api/directories/{parentIno}/files/{name}

**Descrizione:**  
Crea un nuovo file.

**Parametri URL:**
- `parentIno` (string): numero inode della directory padre
- `name` (string): nome del nuovo file

**Restituisce:**
Metadati del file creato.

---

#### DELETE /api/directories/{parentIno}/files/{name}

**Descrizione:**  
Elimina un file.

**Parametri URL:**
- `parentIno` (string): numero inode della directory padre
- `name` (string): nome del file da eliminare

**Restituisce:**
Conferma dell'eliminazione.

---

### Operazioni di Rinomina

#### PATCH /api/directories/{oldParentIno}/entries/{oldName}

**Descrizione:**  
Rinomina o sposta un file o directory.

**Parametri URL:**
- `oldParentIno` (string): numero inode della vecchia directory padre
- `oldName` (string): nome attuale del file/directory

**Corpo (JSON):**
```json
{
  "newParentIno": "67890",
  "newName": "nuovo_nome_file.txt"
}
```

**Restituisce:**
Metadati aggiornati della voce rinominata/spostata.

---

### Operazioni di Lettura/Scrittura File

#### GET /api/files/{ino}

**Descrizione:**
Legge il contenuto di un file.

**Parametri URL:**
- `ino` (string): numero inode del file

**Parametri Query:**
- `offset` (opzionale): offset in byte da cui iniziare la lettura
- `size` (opzionale): numero di byte da leggere

**Restituisce:**
Contenuto del file come dati binari.

---

#### PUT /api/files/{ino}

**Descrizione:**
Scrive contenuto binario in un file.

**Parametri URL:**
- `ino` (string): numero inode del file

**Headers:**
- `Content-Type: application/octet-stream`
- `X-Chunk-Offset` (opzionale): offset in byte da cui iniziare la scrittura

**Corpo:**
Contenuto binario da scrivere.

**Restituisce:**
Numero di byte scritti.

**Tipo di ritorno (JSON):**
```json
{
  "bytes": 1024
}
```

---

#### GET /api/files/stream/{ino}

**Descrizione:**
Trasmette il contenuto di file per file di grandi dimensioni.

**Parametri URL:**
- `ino` (string): numero inode del file

**Restituisce:**
Contenuto del file come stream.

---

#### PUT /api/files/stream/{ino}

**Descrizione:**
Trasmette contenuto binario a un file per upload di grandi dimensioni.

**Parametri URL:**
- `ino` (string): numero inode del file

**Corpo:**
Stream binario da scrivere.

**Restituisce:**
Conferma dell'upload.

---

### Operazioni sui Link

#### POST /api/links/{targetIno}

**Descrizione:**  
Crea un hard link a un file esistente.

**Parametri URL:**
- `targetIno` (string): numero inode del file target

**Corpo (JSON):**
```json
{
  "linkParentIno": "12345",
  "linkName": "nome_link"
}
```

**Restituisce:**
Metadati dell'hard link creato.

---

#### POST /api/symlinks

**Descrizione:**  
Crea un link simbolico.

**Corpo (JSON):**
```json
{
  "linkParenIno": "12345",
  "linkName": "nome_symlink",
  "targetPath": "/percorso/al/target"
}
```

**Restituisce:**
Metadati del link simbolico creato.

---

#### GET /api/symlinks/{ino}

**Descrizione:**  
Legge il percorso target di un link simbolico.

**Parametri URL:**
- `ino` (string): numero inode del link simbolico

**Restituisce:**
Percorso target del link simbolico.

**Tipo di ritorno (JSON):**
```json
{
  "target": "/percorso/al/target"
}
```

---

### Informazioni di Sistema

#### GET /api/size

**Descrizione:**  
Restituisce informazioni sulla dimensione del filesystem (usato solo dai sistemi Windows!).

**Corpo:** Nessuno

**Restituisce:**
Informazioni sullo spazio totale e disponibile.

**Tipo di ritorno (JSON):**
```json
{
  "total": 107374182400,
  "available": 85899345920
}
```

---

### Tipi di Dati

#### Tipi di Voce File
- `0`: File regolare
- `1`: Directory  
- `2`: Link simbolico

#### Permessi
Permessi ottali in stile Unix (es. `755`, `644`, `420`)

#### Timestamp
Tutti i timestamp sono in millisecondi dall'epoca Unix (formato `Date.now()` di JavaScript)

---

### Risposte di Errore

Tutti gli endpoint possono restituire le seguenti risposte di errore:

**401 Unauthorized:**
```json
{
  "error": "Autenticazione richiesta"
}
```

**403 Forbidden:**
```json
{
  "error": "Permesso negato"
}
```

**404 Not Found:**
```json
{
  "error": "File o directory non trovato"
}
```

**500 Internal Server Error:**
```json
{
  "error": "Errore interno del server",
  "details": "Messaggio di errore"
}
```