import e, { Request, Response } from 'express';
import * as fs from 'node:fs/promises';
import * as fsSync from 'node:fs';
import type { Stats } from 'node:fs';
import path_manipulator from 'node:path';
import { AppDataSource } from '../data-source';
import { File } from '../entities/File';
import { User } from '../entities/User';
import { Group } from '../entities/Group';
import { pipeline, Writable } from 'node:stream';
import { permission } from 'node:process';

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
  return path_manipulator.join(FS_ROOT, dbPath);
}

export class FileSystemController {

    // operation:  0: read, 1: write, 2: execute
    private has_permissions = (file: File, operation: number, user: User): boolean => {

        let mask = 0;

        if(user.uid == 5000) // admin!
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
        if ((file.permissions & (mask << 3)) === (mask << 3) && user.group === file.group)
            return true;
        if ((file.permissions & mask) === mask)
            return true;

        return false;
    }

    public readdir = async (req: Request, res: Response) => {
        const inoRec = BigInt(req.params.ino);
        if(!inoRec) 
            return res.status(400).json({message:"Missing ino"});

        try{
            const dir=await fileRepo.findOne({where: {ino:inoRec}, relations: ['owner', 'group'] }) as File | null;
            if (!dir) 
                return res.status(404).json({error: "ENOENT", message: `Directory with ino=${inoRec} not found` });
            if (dir.type !==1 )
                return res.status(400).json({error: "ENOTDIR", message:`${dir.path} is not a directory`});

            if (!(dir.path === "/" || this.has_permissions(dir, 0, req.user as User))) {
                return res.status(403).json({ error: "EACCES", message: `You have not the permission to list ${dir.path}` });
            }

            const fullFsPath=toFsPath(dir.path);
            const names=await fs.readdir(fullFsPath);

            const rows= await Promise.all(
                names.map(async (name) => {
                    const childDbPath = dir.path === "/" ? `/${name}` : `${dir.path}/${name}`;
                    const childFsPath = toFsPath(childDbPath);
                    let stats;
                    try {
                        stats = await fs.lstat(childFsPath, { bigint: true });
                    } catch (e: any) {
                        throw e;
                    }
                    const file = await fileRepo.findOne({
                        where: { ino: stats.ino },
                        relations: ["owner", "group"],
                    }) as File | null;

                    if (!file) {
                        // mismatch FS↔DB
                        throw new Error(`Mismatch FS/DB for ${childDbPath} (ino=${stats.ino})`);
                    }

                    if (!this.has_permissions(file, 0, req.user as User)) return undefined;

                    return {
                        ino: file.ino.toString(),                    
                        path: file.path,
                        type: file.type,
                        permissions: file.permissions,
                        owner: file.owner.uid,
                        group: file.group?.gid,
                        size: stats.size.toString(),
                        atime: stats.atime.getTime(),
                        mtime: stats.mtime.getTime(),
                        ctime: stats.ctime.getTime(),
                        btime: stats.birthtime.getTime()
                    };
                })
            );
            const content=rows.filter(Boolean);
            return res.json(content);
        }catch (err:any){
            if (err?.code === "ENOENT") {
                return res.status(404).json({ error: "ENOENT", message: "Directory not found on filesystem" });
            }
            return res.status(500).json({ error: "EIO", message: `Not possible to read the folder (ino=${inoRec})`, details: String(err?.message ?? err) });
        }
    }

    public mkdir = async (req: Request, res: Response) => {
        const parentIno=BigInt(req.params.parentIno);
        const name: string | undefined = req.body?.name;

        if(!parentIno)
            return res.status(400).json({ error: "EINVAL", message: "Parent inode missing" });
        if (typeof name !== "string" || name.length === 0 || name==="." || name==="..")
            return res.status(400).json({ error: "EINVAL", message: "Invalid directory name" });

        const user: User = req.user as User;
        if (user == null)
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        const userGroup = (await groupRepo.findOne({ where: { users: user } })) as Group | null;
        
        try{
            const parent = (await fileRepo.findOne({
                where: { ino: parentIno },              
                relations: ["owner", "group"],
            })) as File | null;

            if (!parent) {
                return res.status(404).json({ error: "ENOENT", message: `Parent inode ${parentIno} not found` });
            }

            if(!this.has_permissions(parent,2,user)){
                return res.status(403).json({ error: "EACCES", message: `No permission to create in ${parent.path}` });
            }

            const childDbPath = parent.path === "/" ? `/${name}` : `${parent.path}/${name}`;
            const childFsPath = toFsPath(childDbPath);

            await fs.mkdir(childFsPath);
            const stats=await fs.lstat(childFsPath,{bigint:true});
            const directory = {
                ino:stats.ino,
                path:childDbPath,
                owner:user,
                group: userGroup,
                type: 1,
                permissions: 0o755,
            } as File;
            await fileRepo.save(directory);
            return res.status(201).json({
                ino:stats.ino.toString(),
                path:directory.path,
                type:directory.type,
                permission:directory.permissions,
                owner: user.uid,
                group: userGroup?.gid ?? null,
                size: stats.size.toString(),
                atime: stats.atime.getTime(),
                mtime: stats.mtime.getTime(),
                ctime: stats.ctime.getTime(),
                btime: stats.birthtime.getTime(),
            });
        }catch(err:any){
            if (err?.code === "EEXIST") {
                return res.status(409).json({ error: "EEXIST", message: "Folder already exists" });
            }
            if (err?.code === "ENOENT") {
                return res.status(404).json({ error: "ENOENT", message: "Parent path not found on filesystem" });
            }
            return res.status(500).json({ error: "EIO", message: "Not possible to create the folder", details: String(err?.message ?? err) });
        }
    }

    public rmdir = async (req: Request, res: Response) => {
        const parentIno=BigInt(req.params.parentIno);
        const name: string | undefined = req.body?.name;

        if(!parentIno)
            return res.status(400).json({ error: "EINVAL", message: "Parent inode missing" });
        if (typeof name !== "string" || name.length === 0 || name==="." || name==="..")
            return res.status(400).json({ error: "EINVAL", message: "Invalid directory name" });

        const user = req.user as User;
        if (user === null) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }
        try {
            const parent= await fileRepo.findOne({
                where:{ino:parentIno},
                relations:["owner","group"],
            })as File;
            if (!parent){
                return res.status(404).json({ error: "ENOENT", message: `Parent inode ${parentIno} not found` });
            }
            
            if(!this.has_permissions(parent,1,user)){
                return res.status(403).json({ error: "EACCES", message: `No permission to remove in ${parent.path}` });
            }
            const childDbPath = parent.path === "/" ? `/${name}` : `${parent.path}/${name}`;
            const childFsPath = toFsPath(childDbPath);

            const child= await fileRepo.findOne({
                where: { path: childDbPath },
                relations: ["owner", "group"],
            }) as File;

            if(!child){
                return res.status(404).json({ error: "ENOENT", message: "Directory not found" });
            }

            if (child.type !== 1) {
                return res.status(400).json({ error: "ENOTDIR", message: "The specified name is not a directory" });
            }

            try{
                await fs.rmdir(childFsPath);
            } catch (e:any){
                if (e?.code === "ENOTEMPTY") {
                    return res.status(409).json({ error: "ENOTEMPTY", message: "Directory not empty" });
                }
                if (e?.code === "ENOENT") {
                    return res.status(404).json({ error: "ENOENT", message: "Directory not found" });
                }
                throw e;
            }

            await fileRepo.remove(child);
            return res.status(200).end();
        } catch (err: any) {
            return res.status(500).json({
                error: "EIO",
                message: "Not possible to remove the directory",
                details: String(err?.message ?? err),
            });
        }
    }

    public create = async (req: Request, res: Response) => {
        const parentIno=BigInt(req.params.parentIno);
        const name: string | undefined = req.body?.name;

        if(!parentIno)
            return res.status(400).json({ error: "EINVAL", message: "Parent inode missing" });
        if (typeof name !== "string" || name.length === 0 || name==="." || name==="..")
            return res.status(400).json({ error: "EINVAL", message: "Invalid directory name" });

        const user = req.user as User;
        if (user === null) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        const userGroup= await groupRepo.findOne({where: {users:user}}) as Group;
        try{
            const parent=await fileRepo.findOne({
                where:{ino:parentIno},
                relations:["owner","group"]
            }) as File;

            if (!parent) 
                return res.status(404).json({ error: "ENOENT", message: `Parent inode ${parentIno} not found` });

            if (parent.type !== 1) 
                return res.status(400).json({ error: "ENOTDIR", message: "Parent is not a directory" });

            if(!this.has_permissions(parent,1,user)){
                return res.status(403).json({ error: "EACCES", message: `No permission to create in ${parent.path}` });
            }

            const childDbPath = parent.path === "/" ? `/${name}` : `${parent.path}/${name}`;
            const childFsPath = toFsPath(childDbPath);

            await fs.writeFile(childFsPath, "", { flag: "wx" });
            const stats=await fs.lstat(childFsPath,{bigint:true});
            const file={
                ino:stats.ino,
                path:childDbPath,
                owner:user,
                group: userGroup ?? null,
                type: 0,
                permissions: 0o644,
            }
            await fileRepo.save(file);

            return res.status(201).json({
                ino: stats.ino.toString,
                path: file.path,
                type: file.type,
                permission: file.permissions,
                owner: user.uid,
                group: userGroup?.gid ?? null,
                size: stats.size.toString(),
                atime: stats.atime.getTime(),
                mtime: stats.mtime.getTime(),
                ctime: stats.ctime.getTime(),
                btime: stats.birthtime.getTime(),
            })
        }catch(err:any){
            console.log(err);
            if (err?.code === "EEXIST") {
                return res.status(409).json({ error: "EEXIST", message: "File already exists" });
            }
            if (err?.code === "ENOENT") {
                return res.status(404).json({ error: "ENOENT", message: "Parent path not found on filesystem" });
            }
            return res.status(500).json({
                error: "EIO",
                message: "Not possible to create the file",
                details: String(err?.message ?? err),
            });
        }
    }

    public unlink = async (req: Request, res: Response) => {
        const parentIno=BigInt(req.params.parentIno);
        const name: string | undefined = req.body?.name;

        if(!parentIno)
            return res.status(400).json({ error: "EINVAL", message: "Parent inode missing" });
        if (typeof name !== "string" || name.length === 0 || name==="." || name==="..")
            return res.status(400).json({ error: "EINVAL", message: "Invalid directory name" });

        const user = req.user as User;
        if (user === null) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        try{
            const parent= await fileRepo.findOne({
                where:{ino:parentIno},
                relations:["owner", "group"],
            }) as File;
            if (!parent){
                if (!parent) return res.status(404).json({ error: "ENOENT", message: `Parent inode ${parentIno} not found` });
            }
            if(!this.has_permissions(parent,1,user)){
                return res.status(403).json({ error: "EACCES", message: `No permission to remove in ${parent.path}` });
            }
            const childDbPath = parent.path === "/" ? `/${name}` : `${parent.path}/${name}`;
            const childFsPath = toFsPath(childDbPath);
            const child=await fileRepo.findOne({
                where: {path:childDbPath},
                relations:["owner","group"],
            })as File;
            if (!child){
                return res.status(404).json({ error: "ENOENT", message: "File metadata not found in database" });
            }

            try{
                await fs.unlink(childFsPath);
            }catch(err:any){
                if (err?.code === "ENOENT")  
                    return res.status(404).json({ error: "ENOENT", message: "File not found" });
                if (err?.code === "EISDIR")  
                    return res.status(400).json({ error: "EISDIR", message: "Target is a directory" });
                throw err;
            }

            await fileRepo.remove(child);
            return res.status(200).end();
        }catch(err:any){
            return res.status(500).json({
                error: "EIO",
                message: "Not possible to remove the file",
                details: String(err?.message ?? err),
            });
        }
    }

    public writeStream = async (req: Request, res: Response) => {
        const dbPath = normalizePath(req.params.path);
        const fullFsPath = toFsPath(dbPath);

        //const text: Buffer = Buffer.from(req.body.data);
        const offset: number = Number(req.headers['x-chunk-offset'] ?? 0); // offset passed by the client, default 0      
        const user: User = req.user as User;
        if (user === null) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        const user_group: Group = await groupRepo.findOne({ where: { users: user } }) as Group;

        try {

            const file: File = await fileRepo.findOne({
                where: { path:dbPath },
                relations: ['owner', 'group']
            }) as File;
            if (!this.has_permissions(file, 1, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to write on the file ' + dbPath });
            
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

                    if (dbPath === '/create-user.txt') { // new user
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
                            const fetchRes = await fetch(`http://localhost:${process.env.PORT}/api/signup`, {
                                method: 'POST',
                                headers: {
                                    'Content-Type': 'application/json',
                                    'Cookie': req.headers['cookie'] || ''
                                },
                                body: JSON.stringify({ uid, password })
                            });
                            const result = await fetchRes.json();

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
                    else if (dbPath === '/create-group.txt') { // new group
                        try {
                            const content = await fs.readFile(fullFsPath, 'utf8');
                            const fields = content.trim().split(/\s+/);
                            const uid = Number(fields[0]);
                            const gid = Number(fields[1]);

                            if (!uid || !gid || !Number.isInteger(uid) || !Number.isInteger(gid)) {
                                await fs.writeFile(fullFsPath, `Bad format. Write like this:\n<userid> <groupid>`);
                                return res.status(400).json({ error: "Bad format" });
                            }

                            // POST /api/group
                            const fetchRes = await fetch(`http://localhost:${process.env.PORT}/api/group`, {
                                method: 'POST',
                                headers: {
                                    'Content-Type': 'application/json',
                                    'Cookie': req.headers['cookie'] || ''
                                },
                                body: JSON.stringify({ uid, gid })
                            });
                            if (fetchRes.ok) {
                                await fs.writeFile(fullFsPath, `Group ${gid} associated successfully to the user ${uid}.`);
                            } else {
                                await fs.writeFile(fullFsPath, `Correctly associated the group ${gid} to the user ${uid}: ${fetchRes.text || 'Unknown error'}`);
                            }

                        } catch (err: any) {
                            console.error("New group error:", err);
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

    public readStream = async (req: Request, res: Response) => {
        const dbPath = normalizePath(req.params.path);
        const fullFsPath = toFsPath(dbPath);
        const offset = Number(req.query.offset) || 0;

        try {
            const file: File = await fileRepo.findOne({
                where: { path: dbPath },
                relations: ['owner', 'group']
            }) as File;
            if (file === null) {
                return res.status(404)
                    .setHeader('Content-Type', 'application/octet-stream')
                    .setHeader('Content-Length', '0')
                    .end();
            }
            if (!this.has_permissions(file, 0, req.user as User)) {
                return res.status(403)
                    .setHeader('Content-Type', 'application/octet-stream')
                    .setHeader('Content-Length', '0')
                    .end();
            }

            const readStream = fsSync.createReadStream(fullFsPath, { start: offset });

            readStream.on('error', (err) => {
                console.error('[readStream] Stream error:', err);
                if (!res.headersSent) {
                    res.status(500)
                        .setHeader('Content-Type', 'application/octet-stream')
                        .setHeader('Content-Length', '0')
                        .end();
                } else {
                    res.destroy();
                }
            });

            readStream.pipe(res);

        } catch (err: any) {
            res.status(500)
                .setHeader('Content-Type', 'application/octet-stream')
                .setHeader('Content-Length', '0')
                .end();
        }
    }

    public write = async (req: Request, res: Response) => {
        const dbPath = normalizePath(req.params.path);
        const fullFsPath = toFsPath(dbPath);
        const user: User = req.user as User;
        const offset = Number(req.query.offset) || 0;
        if (offset < 0) {
            return res.status(400).json({ error: 'Bad request: invalid offset' });
        }

        let buffer: Buffer;

        if (Buffer.isBuffer(req.body)) {
            buffer = req.body;
        } else {
            return res.status(400).json({ error: 'Bad request: invalid body' });    
        }
        try {
            const file= await fileRepo.findOne({
                where: { path: dbPath },
                relations: ['owner', 'group']
            });
            if(!file)
                return res.status(404).json({ error: 'File not found' });
            if (!this.has_permissions(file, 1, user))
                return res.status(403).json({ error: 'You have not the permission to write the content the file ' + dbPath });
            const fh=await fs.open(fullFsPath, 'r+');
            try {
                await fh.write(buffer, 0, buffer.length, offset);
            } finally {
                await fh.close();
            }
            if (dbPath === '/create-user.txt') {
                try {
                    const text = buffer.toString('utf8');
                    const fields = text.trim().split(/\s+/);
                    const uid = Number(fields[0]);
                    const password = fields[1];

                    if (!uid || !password || !Number.isInteger(uid)) {
                    await fs.writeFile(fullFsPath, `Bad format. Write like this:\n<userid> <password>`);
                    return res.status(400).json({ error: 'Bad format' });
                    }

                    const fetchRes = await fetch(`http://localhost:${process.env.PORT}/api/signup`, {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'application/json',
                        'Cookie': req.headers['cookie'] || ''
                    },
                    body: JSON.stringify({ uid, password })
                    });

                    if (fetchRes.ok) {
                    await fs.writeFile(fullFsPath, `User ${uid} created successfully.`);
                    } else {
                    const result = await fetchRes.json().catch(() => ({}));
                    await fs.writeFile(fullFsPath, `Failed to create user ${uid}: ${result.message || 'Unknown error'}`);
                    return res.status(502).json({ error: 'Signup failed' });
                    }
                } catch (err: any) {
                    console.error('Signup error:', err);
                    await fs.writeFile(fullFsPath, `Error: ${err.message || String(err)}`);
                    return res.status(500).json({ error: 'Internal server error' });
                }
            } else if (dbPath === '/create-group.txt') {
                try {
                    const text = buffer.toString('utf8');
                    const fields = text.trim().split(/\s+/);
                    const uid = Number(fields[0]);
                    const gid = Number(fields[1]);

                    if (!uid || !gid || !Number.isInteger(uid) || !Number.isInteger(gid)) {
                    await fs.writeFile(fullFsPath, `Bad format. Write like this:\n<userid> <groupid>`);
                    return res.status(400).json({ error: 'Bad format' });
                    }

                    const fetchRes = await fetch(`http://localhost:${process.env.PORT}/api/group`, {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'application/json',
                        'Cookie': req.headers['cookie'] || ''
                    },
                    body: JSON.stringify({ uid, gid })
                    });

                    if (fetchRes.ok) {
                    await fs.writeFile(fullFsPath, `Group ${gid} associated successfully to the user ${uid}.`);
                    } else {
                    const textRes = await fetchRes.text().catch(() => '');
                    await fs.writeFile(fullFsPath, `Failed to associate group ${gid} to user ${uid}: ${textRes || 'Unknown error'}`);
                    return res.status(502).json({ error: 'Group association failed' });
                    }
                } catch (err: any) {
                    console.error('New group error:', err);
                    await fs.writeFile(fullFsPath, `Error: ${err.message || String(err)}`);
                    return res.status(500).json({ error: 'Internal server error' });
                }
            }

            return res.status(200).json({ bytes: buffer.length });

        } catch (err: any) {
            if (err.code === 'ENOENT') {
            return res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
            return res.status(403).json({ error: 'Access denied' });
            } else if (err.code === 'EISDIR') {
            return res.status(400).json({ error: 'Is a directory' });
            } else {
            return res.status(500).json({ error: 'Not possible to write the file ' + dbPath, details: String(err) });
            }
        }
    }

    public read = async (req: Request, res: Response) => {
        const dbPath = normalizePath(req.params.path);
        const fullFsPath = toFsPath(dbPath);

        const offset = Number(req.query.offset) || 0;
        const size = Number(req.query.size) || 4096;
        const user: User = req.user as User;

        try {
            const file: File = await fileRepo.findOne({
                where: { path: dbPath },
                relations: ['owner', 'group']
            }) as File;
            if (file === null)
                return res.status(404).json({ error: 'File not found' });
            if (!this.has_permissions(file, 0, user))
                return res.status(403).json({ error: 'You have not the permission to read the content the file ' + dbPath });

            const fd = await fs.open(fullFsPath, 'r');
            try {
                const buffer = Buffer.alloc(size);
                const { bytesRead } = await fd.read(buffer, 0, size, offset);
                res.status(200);
                res.setHeader('Content-Type', 'application/octet-stream');
                res.setHeader('Content-Length', String(bytesRead));
                res.end(buffer.subarray(0,bytesRead)); 
            } finally {
                await fd.close();
            }

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
                path: dbNewPath
            });
            await fileRepo.save(new_file);

            // retreiving file size dinamically
            const stats: Stats = await fs.stat(fullNewFsPath);

            res.status(200).json({
                ...new_file,
                owner: new_file.owner.uid,
                group: new_file.group?.gid,
                size: stats.size,
                atime: stats.atime.getTime(),
                mtime: stats.mtime.getTime(),
                ctime: stats.ctime.getTime(),
                btime: stats.birthtime.getTime()
            });
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

        let {
            perm: rawPerm,
            uid: rawUid,
            gid: rawGid,
            size: rawSize,
            // flags: rawFlags
        } = req.body;

        const user = await userRepo.findOne({ where: { uid: rawUid } });
        if (!user) { // file moved from another file system
            // substituting the uid with the authenticated user uid
            const file = await fileRepo.findOneOrFail({
                where: { path: dbPath },
                relations: ['owner', 'group']
            });
            file.owner = req.user as User;
            file.group = (req.user as User).group;
            await fileRepo.save(file);

            rawUid = undefined;
            rawGid = undefined;
        }

        // Da implementare vedendo se il group è esistente e se l'utente ha i permessi per cambiarlo
        if (rawUid !== undefined && rawUid !== null || rawGid !== undefined && rawGid !== null) {
            return res.status(403).json({ error: 'Changing ownership is not allowed' });
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


        try{
            const file = await fileRepo.findOneOrFail({
                where: { path: dbPath },
                relations: ['owner', 'group']
            });

            if (!this.has_permissions(file, /* bit= */1, req.user as User)) {
                return res.status(403).json({ error: `No permission on ${dbPath}` });
            }

            if (newPerm !== undefined && newPerm >= 0o000 && newPerm <= 0o777 && newPerm !== file.permissions) {
                file.permissions = newPerm;
            }

            if (newSize !== undefined) {
                await fs.truncate(fullFsPath, newSize);
            }
            // retreiving file size dinamically
            const stats: Stats = await fs.stat(fullFsPath);

            await fileRepo.save(file);
            
            return res.status(200).json({
                ...file,
                owner: file.owner.uid,
                group: file.group?.gid,
                size: stats.size,
                atime: stats.atime.getTime(),
                mtime: stats.mtime.getTime(),
                ctime: stats.ctime.getTime(),
                btime: stats.birthtime.getTime()
            });
        } catch (err: any) {
            if (err.name === 'EntityNotFound') {
                return res.status(404).json({ error: 'File not found' });
            }
            if (err.code === 'ENOENT') {
                return res.status(404).json({ error: 'Filesystem path not found' });
            }
            if (err.code === 'EACCES') {
                return res.status(403).json({ error: 'Access denied' });
            }
            console.error(err);
            return res.status(500).json({ error: 'Unable to update attributes', details: err.message });
        }
    }

    public getattr = async (req: Request, res: Response) => {
        const dbPath    = normalizePath(req.params.path);
        const isModifiedHead = req.header('if-modified-since');

        if (dbPath == undefined)
            return res.status(400).json({ error: 'Bad format: path parameter is missing' });

        try {
            let file: File = await fileRepo.findOne({
                where: { path: dbPath },
                relations: ['owner', 'group']
            }) as File;
            if (file == null)
                return res.status(404).json({ error: 'File not found' });
            if (!this.has_permissions(file, 0, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to visualize the file ' + dbPath });
            const stats: Stats = await fs.stat(toFsPath(dbPath));

            const lastModifiedSecond= Math.floor(stats.mtime.getTime() / 1000);

            if (isModifiedHead){
                const isModifiedMs = Date.parse(isModifiedHead);
                if (!Number.isNaN(isModifiedMs)) {
                    const isModifiedSeconds = Math.floor(isModifiedMs / 1000);
                    if (lastModifiedSecond <= isModifiedSeconds) {
                        return res.status(304).end(); // Not Modified
                    }
                }
            }

            const lastModifiedHttp= new Date(lastModifiedSecond * 1000).toUTCString();
            res.setHeader('Last-Modified', lastModifiedHttp);

            res.status(200).json({
                ...file,
                owner: file.owner.uid,
                group: file.group?.gid,
                size: stats.size,
                atime: stats.atime.getTime(),
                mtime: stats.mtime.getTime(),
                ctime: stats.ctime.getTime(),
                btime: stats.birthtime.getTime()
            });
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