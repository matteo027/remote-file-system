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
    "name": "nome_file_o_cartella",
    "type": "file" | "directory",
    "size": 1234,
    "perm": 493
  },
  ...
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
---
