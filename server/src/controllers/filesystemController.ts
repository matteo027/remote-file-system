import { Request, Response } from 'express';
import * as fs from 'node:fs/promises';
import * as fsSync from 'node:fs';     
import path_manipulator from 'node:path';
import { AppDataSource } from '../data-source';
import { File } from '../entities/File';
import { User } from '../entities/User';
import { Group } from '../entities/Group';
import { pipeline, Writable } from 'node:stream';
import { AuthenticationController } from './authenticationController';

const FS_ROOT = path_manipulator.join(__dirname, '..', '..', 'file-system');
const fileRepo = AppDataSource.getRepository(File);
const userRepo = AppDataSource.getRepository(User);
const groupRepo = AppDataSource.getRepository(Group);

function normalizePath(input?: string | string[]): string {
    const raw = Array.isArray(input) ? input.join('/'): (input ?? '');
    const replaced = raw.replace(/\\/g, '/');
    // 3) normalizza POSIX (rimuove ".", "..", doppi slash, ecc.)
    return path_manipulator.posix.normalize('/' + replaced);
}

function toFsPath(dbPath: string): string {
  const segments = dbPath === '/' ? [] : dbPath.slice(1).split('/');
  return path_manipulator.join(FS_ROOT, ...segments);
}

export class FileSystemController {

    // operation:  0: read, 1: write, 2: execute
    private has_permissions = (file: File, operation: number, user: User): boolean => {

        let mask = 0;

        if(user.uid == 5000) // admin!
            return true;

        const slashes = file.path.split('/').length-1;
        console.log("path:", file.path, "slashes:", slashes);
        if(operation == 0 && slashes == 1)
            return true;

        switch (operation) {
            case 0:
                mask = 0o4;
                if(file.path == '/') // can alwas read the root
                    return true;
                break;
            case 1:
                mask = 0o2;
                break;
            case 2:
                mask = 0o1;
                break;
        }

        if ((file.permissions & (mask << 6)) === (mask << 6) && user.uid === file.owner.uid)
            return true;
        if ((file.permissions & (mask << 3)) === (mask << 3) && user.groups.includes(file.group))
            return true;
        if ((file.permissions & mask) === mask)
            return true;

        return false;
    }

    public readdir = async (req: Request, res: Response) => {
        const dbPath    = normalizePath(req.params.path);
        if(!(dbPath === '/' || this.has_permissions(await fileRepo.findOne({ where: { path: dbPath}, relations: ['owner', 'group'] }) as File, 1, req.user as User)))
            return res.status(403).json({ error: 'EACCES', message: 'You have not the permission to remove the directory ' + dbPath });
        const fullFsPath = toFsPath(dbPath);
        try {
            const names = await fs.readdir(fullFsPath);
            const content = await Promise.all(
                names.map(async (name) => {
                    const childPath = dbPath === '/' ? `/${name}` : `${dbPath}/${name}`;
                    const file: File = await fileRepo.findOne({ where: { path: childPath }, relations: ['owner', 'group'] }) as File;
                    if (!file) {
                        console.log("not good for", childPath)
                        throw new Error(`Mismatch between the file system and the database for file: ${childPath}`);
                    }
                    const size: number = (await fs.stat(toFsPath(childPath))).size;
                    return { ...file, owner: file.owner.uid, group: file.group?.gid, size: size };
                })
            );
            return res.json(content);
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                return res.status(404).json({ error: 'Directory not found' });
            }
            return res.status(500).json({ error: 'Not possible to read from the folder ' + dbPath, details: err });
        }
    }

    public mkdir = async (req: Request, res: Response) => {
        const dbPath    = normalizePath(req.params.path);
        const fullFsPath = toFsPath(dbPath);

        const now = Date.now();
        const user: User = req.user as User;
        if (user == null) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        const user_group: Group = await groupRepo.findOne({ where: { users: user } }) as Group;

        try {
            await fs.mkdir(fullFsPath);
            const directory = {
                path: dbPath,
                owner: user,
                type: 1,
                permissions: 0o755,
                group: user_group,
                size: 0,
                atime: now,
                mtime: now,
                ctime: now,
                btime: now
            } as File;
            await fileRepo.save(directory);

            // retreiving file size dinamically
            const size: number = (await fs.stat(fullFsPath)).size;

            return res.status(200).json({...directory, owner: user.uid, group: user_group?.gid, size: size});
        } catch (err: any) {
            if (err.code === 'EEXIST') { // Error Exists
                return res.status(409).json({ error: 'Folder already exists' });
            } else {
                return res.status(500).json({ error: 'Not possible to create the folder ' + dbPath, details: err });
            }
        }
    }

    public rmdir = async (req: Request, res: Response) => {
        const dbPath    = normalizePath(req.params.path);
        const fullFsPath = toFsPath(dbPath);

        try {
            const dir: File = await fileRepo.findOne({ where: { path: dbPath }, relations: ['owner'] }) as File;
            if (!this.has_permissions(dir, 1, req.user as User))
                return res.status(403).json({ error: 'EACCES', message: 'You have not the permission to remove the directory ' + dbPath });

            if (dir.type != 1) {
                return res.status(400).json({ error: 'ENOTDIR', message: 'The specified path is not a directory' });
            }

            await fs.rm(fullFsPath, { recursive: true }); // fs.rmdirwill be depreacted
            await fileRepo.remove(dir);
            res.status(200).end();
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'Directory not found' });
            } else {
                res.status(500).json({ error: 'Not possible to remove the directory ' + dbPath, details: err });
            }
        }
    }

    public create = async (req: Request, res: Response) => {

        const dbPath    = normalizePath(req.params.path);
        const fullFsPath = toFsPath(dbPath);
        const now = Date.now();
        const user: User = req.user as User;
        if (user === null) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        const user_group: Group = await groupRepo.findOne({ where: { users: user } }) as Group;

        try {
            await fs.writeFile(fullFsPath, "", { flag: "wx" });

            const file: File = {
                path: dbPath,
                owner: user,
                type: 0,
                permissions: 0o644,
                group: user_group,
                size: 0,
                atime: now,
                mtime: now,
                ctime: now,
                btime: now
            } as File;
            await fileRepo.save(file);

            // retreiving file size dinamically
            const size: number = (await fs.stat(fullFsPath)).size;

            res.status(200).json({ ...file, owner: user.uid, group: user_group?.gid, size: size });
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'Directory not found' });
            } else if (err.code === 'EEXIST') {
                res.status(409).json({ error: 'File already exists' });
            } else {
                res.status(500).json({ error: 'Not possible to create the file ' + dbPath, details: err });
            }
        }
    }

    public write = async (req: Request, res: Response) => {
        const dbPath = normalizePath(req.params.path);
        const fullFsPath = toFsPath(dbPath);

        //const text: Buffer = Buffer.from(req.body.data);
        const offset: number = Number(req.headers['x-chunk-offset'] ?? 0); // offset passed by the client, default 0      
        const user: User = req.user as User;
        if (user === null) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        const user_group: Group = await groupRepo.findOne({ where: { users: user } }) as Group;
        const now = Date.now();

        try {

            const file: File = await fileRepo.findOne({
                where: { path:dbPath },
                relations: ['owner', 'group']
            }) as File;
            if (!this.has_permissions(file, 1, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to write on the file ' + dbPath });

            
            file.mtime = now;
            file.ctime = now;

            await fileRepo.save(file); // update metadata before writing to the file system


            //const fh = await fs.open(fullFsPath, "r+");
            //await fh.write(text, 0, text.length, offset);
            //await fh.close();

            const fd = fsSync.openSync(fullFsPath, 'r+'); // or 'w+' to truncate, 'a+' to append
            const writeStream = fsSync.createWriteStream('', { fd, start: offset, autoClose: true });
            let bytesWritten = 0;
            req.on('data', (chunk) => {
                bytesWritten += chunk.length;
            });

            req.pipe(writeStream);

            let responded = false; // serve perché potrebbe inviare un 500 error dopo un 200 finish
            writeStream.on('finish', async () => {
                if (!responded) {
                    responded = true;

                    if (dbPath === '/create-user.txt') {
                        try {
                            const content = await fs.readFile(fullFsPath, 'utf8');
                            const fields = content.trim().split(/\s+/);
                            const uid = Number(fields[0]);
                            const password = fields[1];

                            if (!uid || !password || !Number.isInteger(uid)) {
                                await fs.writeFile(fullFsPath, `Bad format. Write like this:\n<userid> <password>`);
                                return res.status(400).json({ error: "Bad format" });
                            }

                            // POST /api/signup
                            const fetchRes = await fetch('http://localhost:3000/api/signup', {
                                method: 'POST',
                                headers: { 'Content-Type': 'application/json' },
                                body: JSON.stringify({ uid, password })
                            });
                            const result = await fetchRes.json()

                            if (fetchRes.ok) {
                                await fs.writeFile(fullFsPath, `User ${uid} created successfully.`);
                            } else {
                                await fs.writeFile(fullFsPath, `Failed to create user ${uid}: ${result.message || 'Unknown error'}`);
                            }

                        } catch (err: any) {
                            console.error("Signup error:", err);
                            await fs.writeFile(fullFsPath, `Error: ${err.message}`);
                            return res.status(500).json({ error: "Internal server error" });
                        }
                    }


                    res.status(200).json({ bytes: bytesWritten });
                }
            });

            writeStream.on('error', (err) => {
                if (!responded) {
                    responded = true;
                    console.error('Stream error:', err);
                    res.status(500).json({ error: 'Write error' });
                }
            });
        } catch (err: any) {
            console.error('Error writing file:', err);
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                res.status(403).json({ error: 'Access denied' });
            } else {
                res.status(500).json({ error: 'Not possible to write on file ' + dbPath, details: err });
            }
        }
    }

    // returns an object containing the field "data", associated to the file content
    public open = async (req: Request, res: Response) => {
        const dbPath = normalizePath(req.params.path);
        const fullFsPath = toFsPath(dbPath);

        const offset = Number(req.query.offset) || 0;
        const size = Number(req.query.size) || 4096;
        const now = Date.now();

        try {
            const file: File = await fileRepo.findOne({
                where: { path: dbPath },
                relations: ['owner', 'group']
            }) as File;
            if (file === null)
                return res.status(404).json({ error: 'File not found' });
            if (!this.has_permissions(file, 0, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to read the content the file ' + dbPath });
            file.atime = now;
            await fileRepo.save(file); // update access time before reading

            //const fh = await fs.open(fullFsPath, "r");
            //const buffer = Buffer.alloc(Number(size));
            //await fh.read(buffer, 0, Number(size), Number(offset));
            //await fh.close();

            const readStream = fsSync.createReadStream(fullFsPath, { start: offset , end: offset+size-1 }); // il -1 server perchè end è incluso
            readStream.pipe(res);

            //res.json({ data: buffer.toString("utf-8")}); //offset non serve al ritorno
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                res.status(403).json({ error: 'Access denied' });
            } else {
                res.status(500).json({ error: 'Not possible to read the file ' + dbPath, details: err });
            }
        }
    }

    public unlink = async (req: Request, res: Response) => {
        const dbPath    = normalizePath(req.params.path);
        const fullFsPath = toFsPath(dbPath);

        const user: User = req.user as User;
        if (user === null) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        const user_group: Group = await groupRepo.findOne({ where: { users: user } }) as Group;
        try {
            const file: File = await fileRepo.findOne({
                where: { path: dbPath },
                relations: ['owner', 'group']
            }) as File;

            if (!file) {
                return res.status(404).json({ error: 'File metadata not found in database' });
            }
            if (!this.has_permissions(file, 1, req.user as User))
                return res.status(403).json({ err: 'You have not the permission to delete the file ' + dbPath });

            await fs.rm(fullFsPath, { force: true }); // force is used to ignore errors if the file does not exist
            await fileRepo.remove(file);
            res.status(200).end();
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else {
                res.status(500).json({ error: 'Not possible to remove the file ' + dbPath, details: err });
            }
        }
    }

    public rename = async (req: Request, res: Response) => {

        if (req.body.new_path === undefined)
            return res.status(400).json({ error: 'Bad format: new file name parameter is missing' });

        const dbOldPath=normalizePath(req.params.path);
        const dbNewPath=normalizePath(req.body.new_path);

        if (dbOldPath === '/') {
            return res
                .status(400)
                .json({ error: 'Cannot rename the root directory' });
        }

        const fullOldFsPath = toFsPath(dbOldPath);
        const fullNewFsPath = toFsPath(dbNewPath);

        try {
            const file: File = await fileRepo.findOne({
                where: { path: dbOldPath },
                relations: ['owner', 'group']
            }) as File;
            if (!file) {
                return res.status(404).json({ error: 'File metadata not found' });
            }

            if (!this.has_permissions(file, 1, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to rename on the file ' + dbOldPath });

            await fs.rename(fullOldFsPath, fullNewFsPath);
            await fileRepo.remove(file);
            const new_file = fileRepo.create({
                ...file,
                ctime: Date.now(),
                path: dbNewPath
            });
            await fileRepo.save(new_file);

            // retreiving file size dinamically
            const size: number = (await fs.stat(fullNewFsPath)).size;

            res.status(200).json({ ...new_file, owner: new_file.owner.uid, group: new_file.group?.gid, size: size });
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                res.status(403).json({ error: 'Access denied' });
            } else {
                res.status(500).json({ error: 'Not possible to rename ' + dbOldPath, details: err });
            }
        }
    }

    public setattr = async (req: Request, res: Response) => {
        const dbPath = normalizePath(req.params.path);
        if (!dbPath) {
            return res.status(400).json({ error: 'Path parameter is missing' });
        }

        const fullFsPath = toFsPath(dbPath);

        const {
            perm: rawPerm,
            uid: rawUid,
            gid: rawGid,
            size: rawSize,
            // flags: rawFlags
        } = req.body;

        // Da implementare vedendo se il group è esistente e se l'utente ha i permessi per cambiarlo
        if (rawUid !== undefined && rawUid !== null || rawGid !== undefined && rawGid !== null) {
            return res.status(401).json({ error: 'Changing ownership is not allowed' });
        }

        let newPerm: number | undefined;
        if (rawPerm !== undefined && rawPerm !== null) {
            newPerm = parseInt(rawPerm, 10);
            if (isNaN(newPerm) || newPerm < 0 || newPerm > 0o777) {
                return res.status(400).json({ error: 'Invalid mode (0–0o777)' });
            }
        }

        let newSize: number | undefined;
        if (rawSize !== undefined && rawSize !== null) {
            newSize = parseInt(rawSize, 10);
            if (isNaN(newSize) || newSize < 0) {
                return res.status(400).json({ error: 'Invalid size' });
            }
        }

        const now = Date.now();

        try{
            const file = await fileRepo.findOneOrFail({
                where: { path: dbPath },
                relations: ['owner', 'group']
            });

            if (!this.has_permissions(file, /* bit= */1, req.user as User)) {
                return res.status(403).json({ error: `No permission on ${dbPath}` });
            }

            if (newPerm !== undefined) {
                // await fs.chmod(fullFsPath, newPerm); // non c'è bisogno di cambiare i metadati effettivi del file
                file.permissions = newPerm;
                file.ctime = now; // update ctime to reflect the change
            }

            if (newSize !== undefined) {
                await fs.truncate(fullFsPath, newSize);
                file.mtime = now; // update mtime to reflect the change
                file.ctime = now; // update ctime to reflect the change
            }
            // retreiving file size dinamically
            const size: number = (await fs.stat(fullFsPath)).size;

            await fileRepo.save(file);
            return res.status(200).json({...file, owner: file.owner.uid, group: file.group?.gid, size: size });
        } catch (err: any) {
            if (err.name === 'EntityNotFound') {
                return res.status(404).json({ error: 'File not found' });
            }
            if (err.code === 'ENOENT') {
                return res.status(404).json({ error: 'Filesystem path not found' });
            }
            if (err.code === 'EACCES') {
                return res.status(403).json({ error: 'Access denied on FS' });
            }
            console.error(err);
            return res.status(500).json({ error: 'Unable to update attributes', details: err.message });
        }
    }

    public getattr = async (req: Request, res: Response) => {
        const dbPath    = normalizePath(req.params.path);

        if (dbPath == undefined)
            return res.status(400).json({ error: 'Bad format: path parameter is missing' });

        try {
            let file: File = await fileRepo.findOne({
                where: { path: dbPath },
                relations: ['owner', 'group']
            }) as File;
            if (!this.has_permissions(file, 0, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to visualize the file ' + dbPath });
            // retreiving file size dinamically
            const size: number = (await fs.stat(toFsPath(dbPath))).size;

            res.status(200).json({ ...file, owner: file.owner.uid, group: file.group?.gid, size: size });
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                res.status(403).json({ error: 'Access denied' });
            } else {
                console.error(err);
                res.status(500).json({ error: 'Not possible to perform the operation ' + dbPath, details: err });
            }
        }
    }

}