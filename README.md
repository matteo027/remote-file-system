# ENGLISH

# FileSystem API

All routes require authentication (middleware "isLoggedIn").


## GET /api/directories/{*path}

Description:
Returns the list of folders and files contained in the directory indicated by the path.

Parameters:
- path (string): path relative to the remote root (example: "5000/projects").

Request body:
None

**Response:**
- List of items contained in the directory (files and folders).

**Return type:**
```json
[
  {
    "path": "/5000/notes.txt",
    "owner": 5000,
    "group": 5000,
    "permissions": 420,
    "type": 0,
    "size": 1024,
    "atime": 1699999999999,
    "mtime": 1699999999999,
    "ctime": 1699999999999,
    "btime": 1699999999999,
  },
  {
    "path": "/5000/dir",
    "owner": 5000,
    "group": 5000,
    "permissions": 420,
    "type": 1,
    "size": 1024,
    "atime": 1399999999999,
    "ctime": 1399999999999,
    "ctime": 1399999999999,
    "btime": 1399999999999,
  }
]
```

---

## GET /api/files/{*path}

Description:
Returns the metadata of the file or directory specified by the path.

Parameters:
- path (string): path relative to the remote file or directory.

Request body:
None

**Response:**
- Metadata for the requested file or directory.

**Return type:**
```json
{
  "path": "/5000/notes.txt",
  "owner": 5000,
  "group": 5000,
  "permissions": 420,
  "type": 0,
  "size": 1024,
  "atime": 1699999999999,
  "mtime": 1699999999999,
  "ctime": 1699999999999,
  "btime": 1699999999999,
}
```

---

## POST /api/files/{*path}

**Description:**  
Updates the metadata of the file or directory indicated by the path.

**Parameters:**  
- `path` (string): path relative to the remote file or directory.

**Request body:**  
```json
{
  "name": "new_name",          // optional, string
  "perm": 493                   // optional, integer (permissions)
}
```

---

## DELETE /api/files/{*path}

**Description:**  
Deletes the file or directory specified by the path.

**Parameters:**  
- `path` (string): relative path of the file or directory to be deleted.

**Response:**  
Confirmation of deletion.

**Return type:**  
```json
{
  "success": true,
  "message": "File or directory successfully deleted."
}
```

---

## PATCH /api/files/{*path}

**Description:**  
Renames a file or directory.

**URL parameters:**
- `path` (string): path of the file/directory to be renamed.

**Request body (JSON):**
```json
{
  "new_path": "/new/path/of/the/file"
}
```

**Returns:**
Updated metadata of the renamed file or directory.

**Return type (JSON):**

```json
{
  "path": "/new/path/of/the/file",
  "owner": 1000,
  "group": 1000,
  "permissions": 493,
  "type": 0,
  "size": 2048,
  "atime": 1699999999999,
  "mtime": 1699999999999,
  "ctime": 1699999999999,
  "btime": 1699999999999
}
```

---

## PUT /api/files/{*path}

**Description:**
Writes binary or text content to a file, starting from an offset.

**URL parameters**
- `path` (in path): relative path

**URL parameters**
- `path` (in the path): relative path of the file to be written.

**Supported headers**
- `X-Chunk-Offset` (optional): position (offset in bytes) from which to start writing. Default: 0.

Binary content (stream) of the file to be written.

**Returns:**  
The number of bytes written successfully.

**Return type**
```json
{
  "bytes": 1024
}
```

---

## GET /api/files/attributes/{*path}

**Description:**
Returns the metadata of a file or directory.

**URL parameters:**
- `path` (in the path): the relative path of the file or directory (e.g., /5000/documents/text.txt)

**Body:**  
(none)

**Returns:**  
File or directory metadata.

**Return type (JSON):**
```json
{
  "path": "/documents/text.txt",
  "type": 0,
  "permissions": 420,
  "owner": 1000,
  "group": 1000,
  "atime": 1690963200000,
  "mtime": 1690963200000,
  "ctime": 1690963200000,
  "btime": 1690963200000,
  "size": 1245
}
```

---

## PATCH /api/files/attributes/{*path}

**Description:**  
Updates one or more attributes of a file or directory, such as permissions (`perm`) or size (`size`).  
Properties such as `uid` or `gid` cannot be modified.

**URL parameters:**
- `path` (in path): the relative path of the file or directory (e.g., /5000/documents/text.txt)

**Body (JSON):**
- `perm`: (optional) new permissions in octal format (e.g., 644)
- `size`: (optional) new file size (for files only)
- `uid` / `gid`: ignored, cannot be modified

```json
{
  "perm": 644,
  "size": 1000
}
```

**Returns:**
Updated metadata for the file or directory.

**Return type (JSON):**
```json
{
  "path": "/documents/text.txt",
  "type": 0,
  "permissions": 420,
  "owner": 1000,
  "group": 1000,
  "atime": 1690963200000,
  "mtime": 1691505600000,
  "ctime": 1691505600000,
  "btime": 1690963200000,
  "size": 1000
}
```

---

## POST /api/login

**Description:**  
Log in with local authentication (session).

**Request body (form-urlencoded or JSON):**
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

## POST /api/logout

**Description:**  
Ends the active user session.

**Body:**  
(none)

**Returns:**  
Status `200 OK` without content.

---

## GET /api/me

**Description:**  
Returns the data of the currently authenticated user.

**Body:**  
(none)

**Returns:**  
Authenticated user data.

**Return type (JSON):**
```json
{
  "uid": 5000,
}
```

## GET /api/signup

**Description:**  
Allows the admin (uid: 5000) to create a new user. The user writes the uid and password of the user to be registered in the `/create-user.txt` file, separated by a space.
**Request body:**
```json
{
  "uid": 5001,
  "password": "password"
}
```

**Returns:**  
(none)

## GET /api/group

**Description:**  
Allows the admin (uid: 5000) to associate a user with a (new) group. The user writes the user's uid and the gid of the group to be associated in the `/create-group.txt` file, separated by a space.

**Request body:**
```json
{
  "uid": 5000,
  "gid": 6000
}
```

**Returns:**  
(none)

Translated with DeepL.com (free version)

---

# ITALIANO

# API del FileSystem

Tutte le rotte richiedono autenticazione (middleware `isLoggedIn`).


## GET /api/directories/{*path}

**Descrizione:**  
Restituisce la lista di cartelle e file contenuti nella directory indicata dal path.

**Parametri:**  
- `path` (string): percorso relativo alla root remota (esempio: `5000/progetti`)

**Corpo della richiesta:**  
Nessuno

**Risposta:**  
Lista degli elementi contenuti nella directory (file e cartelle).

**Tipo di ritorno:**  
```json
[
  {
    "path": "/5000/notes.txt",
    "owner": 5000,
    "group": 5000,
    "permissions": 420,
    "type": 0,
    "size": 1024,
    "atime": 1699999999999,
    "mtime": 1699999999999,
    "ctime": 1699999999999,
    "btime": 1699999999999
  },
  {
    "path": "/5000/dir",
    "owner": 5000,
    "group": 5000,
    "permissions": 420,
    "type": 1,
    "size": 1024,
    "atime": 1399999999999,
    "mtime": 1399999999999,
    "ctime": 1399999999999,
    "btime": 1399999999999
  },
]
```

---

## GET /api/files/{*path}

**Descrizione:**  
Restituisce i metadati del file o directory indicata dal path.

**Parametri:**  
- `path` (string): percorso relativo al file o directory remoto.

**Corpo della richiesta:**  
Nessuno

**Risposta:**  
Metadati del file o directory richiesto.

**Tipo di ritorno:**  
```json
{
  "path": "/5000/appunti.txt",
  "owner": 5000,
  "group": 5000,
  "permissions": 420,
  "type": 0,
  "size": 1024,
  "atime": 1699999999999,
  "mtime": 1699999999999,
  "ctime": 1699999999999,
  "btime": 1699999999999
}
```

---

## POST /api/files/{*path}

**Descrizione:**  
Aggiorna i metadati del file o directory indicata dal path.

**Parametri:**  
- `path` (string): percorso relativo al file o directory remoto.

**Corpo della richiesta:**  
```json
{
  "name": "nuovo_nome",          // opzionale, string
  "perm": 493                   // opzionale, intero (permessi)
}
```

---

## DELETE /api/files/{*path}

**Descrizione:**  
Elimina il file o la directory specificata dal percorso.

**Parametri:**  
- `path` (string): percorso relativo del file o directory da eliminare.

**Risposta:**  
Conferma dell'eliminazione.

**Tipo di ritorno:**  
```json
{
  "success": true,
  "message": "File o directory eliminata con successo."
}
```

---

## PATCH /api/files/{*path}

**Descrizione:**  
Rinomina un file o una directory.

**Parametri URL:**
- `path` (string): percorso del file/directory da rinominare.

**Corpo della richiesta (JSON):**
```json
{
  "new_path": "/nuovo/percorso/del/file"
}
```

**Ritorna:**
Metadati aggiornati del file o della directory rinominata.

**Tipo di ritorno (JSON):**

```json
{
  "path": "/nuovo/percorso/del/file",
  "owner": 1000,
  "group": 1000,
  "permissions": 493,
  "type": 0,
  "size": 2048,
  "atime": 1699999999999,
  "mtime": 1699999999999,
  "ctime": 1699999999999,
  "btime": 1699999999999
}
```

---

## PUT /api/files/{*path}

**Descrizione:**
Scrive contenuti binari o testuali su un file, a partire da un offset.

**Parametri URL**
- `path` (nel percorso): path relativo del file da scrivere.

**Headers supportati**
- `X-Chunk-Offset` (opzionale): posizione (offset in byte) da cui iniziare a scrivere. Default: 0.

Contenuto binario (stream) del file da scrivere.

**Ritorna:**  
Il numero di byte scritti correttamente.

**Tipo di ritorno**
```json
{
  "bytes": 1024
}
```

---

## GET /api/files/attributes/{*path}

**Descrizione:**
Restituisce i metadati di un file o directory.

**Parametri URL:**
- `path` (nel percorso): il path relativo del file o directory (es. /5000/documenti/testo.txt)

**Corpo:**  
(nessuno)

**Ritorna:**  
Metadati del file o directory.

**Tipo di ritorno (JSON):**
```json
{
  "path": "/documenti/testo.txt",
  "type": 0,
  "permissions": 420,
  "owner": 1000,
  "group": 1000,
  "atime": 1690963200000,
  "mtime": 1690963200000,
  "ctime": 1690963200000,
  "btime": 1690963200000,
  "size": 1245
}
```

---

## PATCH /api/files/attributes/{*path}

**Descrizione:**  
Aggiorna uno o più attributi di un file o directory, come permessi (`perm`) o dimensione (`size`).  
Non è consentito modificare proprietà come `uid` o `gid`.

**Parametri URL:**
- `path` (nel percorso): il path relativo del file o directory (es. /5000/documenti/testo.txt)

**Corpo (JSON):**
- `perm`: (opzionale) nuovi permessi in formato ottale (es. 644)
- `size`: (opzionale) nuova dimensione del file (solo per file)
- `uid` / `gid`: ignorati, non modificabili

```json
{
  "perm": 644,
  "size": 1000
}
```

**Ritorna:**
Metadati aggiornati del file o directory.

**Tipo di ritorno (JSON):**
```json
{
  "path": "/documenti/testo.txt",
  "type": 0,
  "permissions": 420,
  "owner": 1000,
  "group": 1000,
  "atime": 1690963200000,
  "mtime": 1691505600000,
  "ctime": 1691505600000,
  "btime": 1690963200000,
  "size": 1000
}
```

---

## POST /api/login

**Descrizione:**  
Effettua il login con autenticazione locale (sessione).

**Corpo della richiesta (form-urlencoded o JSON):**
```json
{
  "uid": 5000,
  "password": "la-tua-password"
}
```

**Ritorna:**
ID utente autenticato.

**Tipo di ritorno (JSON):**
```json
5000
```

---

## POST /api/logout

**Descrizione:**  
Termina la sessione utente attiva.

**Corpo:**  
(nessuno)

**Ritorna:**  
Status `200 OK` senza contenuto.

---

## GET /api/me

**Descrizione:**  
Ritorna i dati dell’utente attualmente autenticato.

**Corpo:**  
(nessuno)

**Ritorna:**  
Dati utente autenticato.

**Tipo di ritorno (JSON):**
```json
{
  "uid": 5000,
}
```

## GET /api/signup

**Descrizione:**  
Consente all'admin (uid: 5000) la creazione di un nuovo utente. L'utente scrive nel file `/create-user.txt` lo uid e la password dell'utente da registrare., spearati da spazio

**Corpo della richiesta:**
```json
{
  "uid": 5001,
  "password": "password"
}
```

**Restituisce:**  
(niente)

## GET /api/group

**Descrizione:**  
Consente all'admin (uid: 5000) l'associazione di un utente a un (nuovo) gruppo. L'utente scrive nel file `/create-group.txt` lo uid dell'utente e il gid del gruppo da associare, separati da spazio.

**Corpo della richiesta:**
```json
{
  "uid": 5000,
  "gid": 6000
}
```

**Restituisce:**  
(niente)

---