import { Request, Response } from 'express';
import * as fs from 'node:fs/promises';
import type { Mode } from 'node:fs';
import path_manipulator from 'node:path';
import { AppDataSource } from '../data-source';
import { File } from '../entities/File';
import { User } from '../entities/User';
import { Group } from '../entities/Group';

const FS_ROOT = path_manipulator.join(__dirname, '..', '..', 'file-system');
const fileRepo = AppDataSource.getRepository(File);
const userRepo = AppDataSource.getRepository(User);
const groupRepo = AppDataSource.getRepository(Group);

function normalizePath(input?: string | string[]): string {
    const raw = Array.isArray(input) ? input.join('/'): (input ?? '');
    const replaced = raw.replace(/\\/g, '/');
    // 2) se è vuoto, considera subito la root "/"
    const toNormalize = replaced === '' ? '/' : replaced;
    // 3) normalizza POSIX (rimuove ".", "..", doppi slash, ecc.)
    return path_manipulator.posix.normalize(toNormalize);
}

function toFsPath(dbPath: string): string {
  const segments = dbPath === '/' ? [] : dbPath.slice(1).split('/');
  return path_manipulator.join(FS_ROOT, ...segments);
}

export class FileSystemController {

    // operation:  0: read, 1: write, 2: execute
    private has_permissions = (file: File, operation: number, user: User): boolean => {

        let mask = 0;

        switch (operation) {
            case 0:
                mask = 0o4;
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
        const fullFsPath = toFsPath(dbPath);
        try {
            const names = await fs.readdir(fullFsPath);
            const content = await Promise.all(
                names.map(async (name) => {
                    const childPath = dbPath === '/' ? `/${name}` : `${dbPath}/${name}`;
                    const file: File = await fileRepo.findOne({ where: { path: childPath }, relations: ['owner', 'group'] }) as File;
                    if (!file) {
                        throw new Error(`Mismatch between the file system and the database for file: ${childPath}`);
                    }
                    return { ...file, owner: file.owner.uid, group: file.group?.gid };
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
            return res.status(200).json({...directory, owner: user.uid, group: user_group?.gid});
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
                type: 1,
                permissions: 0o755,
                group: user_group,
                size: 0,
                atime: now,
                mtime: now,
                ctime: now,
                btime: now
            } as File;
            await fileRepo.save(file);
            res.status(200).json({ ...file, owner: user.uid, group: user_group?.gid });
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
        const dbPath    = normalizePath(req.params.path);
        const fullFsPath = toFsPath(dbPath);

        const text: string = req.body.text;
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

            await fs.writeFile(fullFsPath, text, { flag: "w" });

            res.status(200).json({ bytes: req.body.text.length });
        } catch (err: any) {
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
        const dbPath    = normalizePath(req.params.path);
        const fullFsPath = toFsPath(dbPath);

        try {
            const file: File = await fileRepo.findOne({
                where: { path: dbPath },
                relations: ['owner', 'group']
            }) as File;
            if (file === null)
                return res.status(404).json({ error: 'File not found' }); res.status(404).json({ error: 'File not found' });
            if (!this.has_permissions(file, 0, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to read the content the file ' + dbPath });

            const content = await fs.readFile(fullFsPath, { flag: "r" });
            res.json({ bytes: content.toString(), offset: 0 });
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

        if (req.body.new_name === undefined)
            return res.status(400).json({ error: 'Bad format: new file name parameter is missing' });

        const dbOldPath=normalizePath(req.params.path);
        const dbNewPath=normalizePath(req.body.new_name);

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
            res.status(200).json({ ...new_file, owner: new_file.owner.uid, group: new_file.group?.gid });
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
        const fullFsPath = toFsPath(dbPath);
        const new_mod: Mode = parseInt(req.body.new_mod);
        if (dbPath == undefined)
            return res.status(400).json({ error: 'Bad format: path parameter is missing' });

        if (isNaN(new_mod)) {
            return res.status(400).json({ error: "Parameter 'mod' is not a valid number" });
        }

        if (new_mod < 0 || new_mod > 0o777) {
            return res.status(400).json({ error: "Parameter 'mod' out of range (0-511)" });
        }

        try {

            let file: File = await fileRepo.findOne({
                where: { path: dbPath },
                relations: ['owner', 'group']
            }) as File;
            if (!this.has_permissions(file, 1, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to change mod of the file ' + dbPath });


            file.permissions = new_mod;
            await fileRepo.save(file);

            await fs.chmod(fullFsPath, new_mod);
            res.status(200).json({ ...file, owner: file.owner.uid, group: file.group?.gid });
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                res.status(403).json({ error: 'Access denied' });
            } else {
                res.status(500).json({ error: 'Not possible to change mod of ' + dbPath, details: err });
            }
        }
    }

    public getattr = async (req: Request, res: Response) => {
        const dbPath    = normalizePath(req.params.path);

        if (dbPath == undefined)
            return res.status(400).json({ error: 'Bad format: path parameter is missing' });

        try {
            console.log(`Getting attributes for: ${dbPath}`);

            let file: File = await fileRepo.findOne({
                where: { path: dbPath },
                relations: ['owner', 'group']
            }) as File;
            if (!this.has_permissions(file, 1, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to visualize the file ' + dbPath });

            res.status(200).json({ ...file, owner: file.owner.uid, group: file.group?.gid });
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