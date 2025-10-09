import { Request, Response } from 'express';
import { fileRepo,groupRepo,toFsPath,has_permissions, parseIno} from '../utilities';
import { File } from '../entities/File';
import { User } from '../entities/User';
import { Group } from '../entities/Group';
import * as fsNode from 'node:fs/promises';
import * as fs from 'fs'
import path_manipulator from 'node:path'; 
import { pipeline, Writable } from 'node:stream';
import { permission } from 'node:process';

export function normalizePath(input?: string | string[]): string {
    const raw = Array.isArray(input) ? input.join('/'): (input ?? '');
    const replaced = raw.replace(/\\/g, '/');
    // 3) normalizza POSIX (rimuove ".", "..", doppi slash, ecc.)
    return path_manipulator.posix.normalize('/' + replaced);
}


export class ReadWriteController {
    public writeStream = async (req: Request, res: Response) => {
        console.log("[writeStream] called with ino:", req.params.ino, "offset:", req.query.offset, "user:", (req.user as User)?.uid);
        const ino = parseIno(req.params.ino);
        const offset = Number(req.query.offset) || 0;
        const user: User = req.user as User;

        if (!ino) {
            console.log("[writeStream] status 400: Inode missing");
            return res.status(400).json({ error: "EINVAL", message: "Inode missing" });
        }
        if (user === null) {
            console.log("[writeStream] status 500: User not found");
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }
        if (offset < 0) {
            console.log("[writeStream] status 400: Invalid offset");
            return res.status(400).json({ error: 'Bad request: invalid offset' });
        }

        try {
            const file: File = await fileRepo.findOne({
                where: { ino },
                relations: ['owner', 'group', 'paths']
            }) as File;
            if (!has_permissions(file, 1, req.user as User)) {
                console.log("[writeStream] status 403: No permission");
                return res.status(403).json({ error: 'You have not the permission to write into the inode ' + ino });
            }

            const dbPath = file.paths[0].path;
            if (!dbPath) {
                console.log("[writeStream] status 500: File path not found");
                return res.status(500).json({ error: 'File path not found for inode ' + ino });
            }
            const fullFsPath = toFsPath(dbPath);
            const writeStream = fs.createWriteStream(fullFsPath, { flags: 'r+', start: offset, autoClose: true });
            let bytesWritten = 0;
            req.on('data', (chunk) => {
                bytesWritten += chunk.length;
            });

            req.pipe(writeStream);

            let responded = false;
            writeStream.on('finish', async () => {
                if (!responded) {
                    responded = true;

                    if (dbPath === '/create-user.txt') { // new user
                        try {
                            const content = await fsNode.readFile(fullFsPath, 'utf8');
                            const fields = content.trim().split(/\s+/);
                            const uid = Number(fields[0]);
                            const password = fields[1];

                            if (!uid || !password || !Number.isInteger(uid)) {
                                await fsNode.writeFile(fullFsPath, `Bad format. Write like this:\n<userid> <password>`);
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
                                await fsNode.writeFile(fullFsPath, `User ${uid} created successfully.`);
                            } else {
                                await fsNode.writeFile(fullFsPath, `Failed to create user ${uid}: ${result.message || 'Unknown error'}`);
                            }

                        } catch (err: any) {
                            console.error("Signup error:", err);
                            await fsNode.writeFile(fullFsPath, `Error: ${err.message}`);
                            return res.status(500).json({ error: "Internal server error" });
                        }
                    }
                    else if (dbPath === '/create-group.txt') { // new group
                        try {
                            const content = await fsNode.readFile(fullFsPath, 'utf8');
                            const fields = content.trim().split(/\s+/);
                            const uid = Number(fields[0]);
                            const gid = Number(fields[1]);

                            if (!uid || !gid || !Number.isInteger(uid) || !Number.isInteger(gid)) {
                                await fsNode.writeFile(fullFsPath, `Bad format. Write like this:\n<userid> <groupid>`);
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
                                await fsNode.writeFile(fullFsPath, `Group ${gid} associated successfully to the user ${uid}.`);
                            } else {
                                await fsNode.writeFile(fullFsPath, `Correctly associated the group ${gid} to the user ${uid}: ${fetchRes.text || 'Unknown error'}`);
                            }

                        } catch (err: any) {
                            console.error("New group error:", err);
                            await fsNode.writeFile(fullFsPath, `Error: ${err.message}`);
                            return res.status(500).json({ error: "Internal server error" });
                        }
                    }


                    console.log("[writeStream] status 200: Write finished, bytesWritten:", bytesWritten);
                    res.status(200).json({ bytes: bytesWritten });
                }
            });

            writeStream.on('error', (err) => {
                if (!responded) {
                    responded = true;
                    console.error('[writeStream] Stream error:', err);
                    res.status(500).json({ error: 'Write error' });
                }
            });
        } catch (err: any) {
            console.error('[writeStream] Error writing file:', err);
            if (err.code === 'ENOENT') {
                console.log("[writeStream] status 404: File not found");
                res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                console.log("[writeStream] status 403: Access denied");
                res.status(403).json({ error: 'Access denied' });
            } else {
                console.log("[writeStream] status 500: Internal error");
                res.status(500).json({ error: 'Not possible to write into inode ' + ino, details: err });
            }
        }
    }

    public readStream = async (req: Request, res: Response) => {
        console.log("[readStream] called with ino:", req.params.ino, "offset:", req.query.offset, "user:", (req.user as User)?.uid);
        const ino = parseIno(req.params.ino);
        const offset = Number(req.query.offset) || 0;
        const user: User = req.user as User;

        if (!ino) {
            console.log("[readStream] status 400: Inode missing");
            return res.status(400).setHeader('Content-Type', 'application/octet-stream').end();
        }
        if (offset < 0) {
            console.log("[readStream] status 400: Invalid offset");
            return res.status(400).setHeader('Content-Type', 'application/octet-stream').end();
        }
        if (user === null) {
            console.log("[readStream] status 500: User not found");
            return res.status(500).setHeader('Content-Type', 'application/octet-stream').end();
        }

        try {
            const file: File = await fileRepo.findOne({
                where: { ino },
                relations: ['owner', 'group', 'paths']
            }) as File;
            if (file === null) {
                console.log("[readStream] status 404: File not found");
                return res.status(404)
                    .setHeader('Content-Type', 'application/octet-stream')
                    .setHeader('Content-Length', '0')
                    .end();
            }
            if (!has_permissions(file, 0, req.user as User)) {
                console.log("[readStream] status 403: No permission");
                return res.status(403)
                    .setHeader('Content-Type', 'application/octet-stream')
                    .setHeader('Content-Length', '0')
                    .end();
            }
            const dbPath = file.paths[0].path;
            if (!dbPath) {
                console.log("[readStream] status 500: File path not found");
                return res.status(500)
                    .setHeader('Content-Type', 'application/octet-stream')
                    .setHeader('Content-Length', '0')
                    .end();
            }
            const fullFsPath = toFsPath(dbPath);

            const readStream = fs.createReadStream(fullFsPath, { start: offset });

            readStream.on('error', (err) => {
                console.error('[readStream] Stream error:', err);
                if (!res.headersSent) {
                    console.log("[readStream] status 500: Stream error");
                    res.status(500)
                        .setHeader('Content-Type', 'application/octet-stream')
                        .setHeader('Content-Length', '0')
                        .end();
                } else {
                    res.destroy();
                }
            });

            readStream.pipe(res);
            console.log("[readStream] status 200: Streaming file");

        } catch (err: any) {
            console.error('[readStream] Error:', err);
            console.log("[readStream] status 500: Internal error");
            res.status(500)
                .setHeader('Content-Type', 'application/octet-stream')
                .setHeader('Content-Length', '0')
                .end();
        }
    }

    public write = async (req: Request, res: Response) => {
        console.log("[write] called with ino:", req.params.ino, "offset:", req.query.offset, "user:", (req.user as User)?.uid);
        const ino = parseIno(req.params.ino);
        const offset = Number(req.query.offset) || 0;
        const user: User = req.user as User;

        if (!ino) {
            console.log("[write] status 400: Inode missing");
            return res.status(400).json({ error: "EINVAL", message: "Inode missing" });
        }
        if (offset < 0) {
            console.log("[write] status 400: Invalid offset");
            return res.status(400).json({ error: 'Bad request: invalid offset' });
        }

        let buffer: Buffer;

        if (Buffer.isBuffer(req.body)) {
            buffer = req.body;
        } else {
            console.log("[write] status 400: Invalid body");
            return res.status(400).json({ error: 'Bad request: invalid body' });    
        }
        try {
            const file = await fileRepo.findOne({
                where:{ino},
                relations:["owner", "group", "paths"]
            }) as File;
            if(!file) {
                console.log("[write] status 404: File not found");
                return res.status(404).json({ error: 'File not found' });
            }
            const fullFsPath = toFsPath(file.paths[0].path);
            if (!fullFsPath) {
                console.log("[write] status 500: File path not found");
                return res.status(500).json({ error: 'File path not found for inode ' + ino });
            }
            if (!has_permissions(file, 1, user)) {
                console.log("[write] status 403: No permission");
                return res.status(403).json({ error: 'You have not the permission to write the content the file ' + ino });
            }
            const fh=await fsNode.open(fullFsPath, 'r+');
            try {
                await fh.write(buffer, 0, buffer.length, offset);
            } finally {
                await fh.close();
            }
            const dbPath = file.paths[0].path;
            if (!dbPath) {
                console.log("[write] status 500: File path not found");
                return res.status(500).json({ error: 'File path not found for inode ' + ino });
            }
            if (dbPath === '/create-user.txt') {
                try {
                    const text = buffer.toString('utf8');
                    const fields = text.trim().split(/\s+/);
                    const uid = Number(fields[0]);
                    const password = fields[1];

                    if (!uid || !password || !Number.isInteger(uid)) {
                    await fsNode.writeFile(fullFsPath, `Bad format. Write like this:\n<userid> <password>`);
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
                    await fsNode.writeFile(fullFsPath, `User ${uid} created successfully.`);
                    } else {
                    const result = await fetchRes.json().catch(() => ({}));
                    await fsNode.writeFile(fullFsPath, `Failed to create user ${uid}: ${result.message || 'Unknown error'}`);
                    return res.status(502).json({ error: 'Signup failed' });
                    }
                } catch (err: any) {
                    console.error('Signup error:', err);
                    await fsNode.writeFile(fullFsPath, `Error: ${err.message || String(err)}`);
                    return res.status(500).json({ error: 'Internal server error' });
                }
            } else if (dbPath === '/create-group.txt') {
                try {
                    const text = buffer.toString('utf8');
                    const fields = text.trim().split(/\s+/);
                    const uid = Number(fields[0]);
                    const gid = Number(fields[1]);

                    if (!uid || !gid || !Number.isInteger(uid) || !Number.isInteger(gid)) {
                    await fsNode.writeFile(fullFsPath, `Bad format. Write like this:\n<userid> <groupid>`);
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
                    await fsNode.writeFile(fullFsPath, `Group ${gid} associated successfully to the user ${uid}.`);
                    } else {
                    const textRes = await fetchRes.text().catch(() => '');
                    await fsNode.writeFile(fullFsPath, `Failed to associate group ${gid} to user ${uid}: ${textRes || 'Unknown error'}`);
                    return res.status(502).json({ error: 'Group association failed' });
                    }
                } catch (err: any) {
                    console.error('New group error:', err);
                    await fsNode.writeFile(fullFsPath, `Error: ${err.message || String(err)}`);
                    return res.status(500).json({ error: 'Internal server error' });
                }
            }

            console.log("[write] status 200: Write finished, bytes:", buffer.length);
            return res.status(200).json({ bytes: buffer.length });

        } catch (err: any) {
            console.error('[write] Error:', err);
            if (err.code === 'ENOENT') {
                console.log("[write] status 404: File not found");
                return res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                console.log("[write] status 403: Access denied");
                return res.status(403).json({ error: 'Access denied' });
            } else if (err.code === 'EISDIR') {
                console.log("[write] status 400: Is a directory");
                return res.status(400).json({ error: 'Is a directory' });
            } else {
                console.log("[write] status 500: Internal error");
                return res.status(500).json({ error: 'Not possible to write into the inode ' + ino, details: String(err) });
            }
        }
    }

    public read = async (req: Request, res: Response) => {
        console.log("[read] called with ino:", req.params.ino, "offset:", req.params.offset, "size:", req.query.size, "user:", (req.user as User)?.uid);
        const ino = parseIno(req.params.ino);
        const offset = Number(req.params.offset) || 0;
        const MAX_READ_SIZE = 1024 * 1024; // 1MB
        const size = Math.min(Number(req.query.size) || 4096, MAX_READ_SIZE);
        const user: User = req.user as User;

        if (!ino) {
            console.log("[read] status 400: Inode missing");
            return res.status(400).json({ error: "EINVAL", message: "Inode missing" });
        }
        if (offset < 0 || size <= 0) {
            console.log("[read] status 400: Invalid offset or size");
            return res.status(400).json({ error: 'Bad request: invalid offset or size' });
        }

        try {
            const file: File = await fileRepo.findOne({
                where: { ino },
                relations: ['owner', 'group', 'paths']
            }) as File;
            if (file === null) {
                console.log("[read] status 404: File not found");
                return res.status(404).json({ error: 'File not found' });
            }
            if (!has_permissions(file, 0, user)) {
                console.log("[read] status 403: No permission");
                return res.status(403).json({ error: 'You have not the permission to read the content the file ' + ino });
            }

            const fullFsPath = toFsPath(file.paths[0].path);
            if (!fullFsPath) {
                console.log("[read] status 500: File path not found");
                return res.status(500).json({ error: 'File path not found for inode ' + ino });
            }
            const fd = await fsNode.open(fullFsPath, 'r');
            try {
                const buffer = Buffer.alloc(size);
                const { bytesRead } = await fd.read(buffer, 0, size, offset);
                console.log("[read] status 200: Read finished, bytesRead:", bytesRead);
                res.status(200);
                res.setHeader('Content-Type', 'application/octet-stream');
                res.setHeader('Content-Length', String(bytesRead));
                res.end(buffer.subarray(0,bytesRead)); 
            } finally {
                await fd.close();
            }

        } catch (err: any) {
            console.error('[read] Error:', err);
            if (err.code === 'ENOENT') {
                console.log("[read] status 404: File not found");
                res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                console.log("[read] status 403: Access denied");
                res.status(403).json({ error: 'Access denied' });
            } else {
                res.status(500).json({ error: 'Not possible to read the inode ' + ino, details: err });
            }
        }

    }
}