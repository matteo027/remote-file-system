import { Request, Response } from 'express';
import { fileRepo,groupRepo,toFsPath,has_permissions} from '../utilities';
import { File } from '../entities/File';
import { User } from '../entities/User';
import { Group } from '../entities/Group';
import * as fs from 'node:fs/promises';
import * as fsSync from 'node:fs'; // da rimuovere, meglio async
import path_manipulator from 'node:path'; 
import { pipeline, Writable } from 'node:stream';
import { permission } from 'node:process';

export function normalizePath(input?: string | string[]): string {
    const raw = Array.isArray(input) ? input.join('/'): (input ?? '');
    const replaced = raw.replace(/\\/g, '/');
    // 3) normalizza POSIX (rimuove ".", "..", doppi slash, ecc.)
    return path_manipulator.posix.normalize('/' + replaced);
}


export class ReadWriteController{
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
            if (!has_permissions(file, 1, req.user as User))
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
            if (!has_permissions(file, 0, req.user as User)) {
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
            if (!has_permissions(file, 1, user))
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
            if (!has_permissions(file, 0, user))
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
}