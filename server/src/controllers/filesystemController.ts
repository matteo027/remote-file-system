import { Request, Response } from 'express';
import * as fs from 'node:fs/promises';
import type { Mode } from 'node:fs';
import path_manipulator from 'path';
import { AppDataSource } from '../data-source';
import { File } from '../entities/File';
import { User } from '../entities/User';
import { Group } from '../entities/Group';
import { AuthenticationController } from './authenticationController';

const FS_PATH = path_manipulator.join(__dirname, '..', '..', 'file-system');
const fileRepo = AppDataSource.getRepository(File);
const userRepo = AppDataSource.getRepository(User);
const groupRepo = AppDataSource.getRepository(Group);

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
        if (req.params.path == undefined)
            return res.status(400).json({ error: 'Bad format: path parameter is missing' });
        const path: string = req.params.path.startsWith('/') ? req.params.path.slice(1) : req.params.path;

        try {
            const files = await fs.readdir(path_manipulator.resolve(FS_PATH, path));
            const content = await Promise.all(
                files.map(async (file_name) => {
                    const fullPath = path_manipulator.join(path, file_name);

                    const file: File = await fileRepo.findOne({ where: { path: fullPath }, relations: ['owner'] }) as File;
                    if (file == null) {
                        throw new Error(`Mismatch between the file system and the database for file: ${fullPath}`);
                    }
                    return { ...file, owner: file.owner.uid, group: file.group?.gid };
                })
            );
            return res.json(content);
        } catch (err) {
            return res.status(500).json({ error: 'Not possible to read from the folder ' + path_manipulator.resolve(FS_PATH, path), details: err });
        }
    }

    public mkdir = async (req: Request, res: Response) => {

        if (req.params.path == undefined)
            return res.status(400).json({ error: 'Bad format: path parameter is missing' });
        const path: string = req.params.path.startsWith('/') ? req.params.path.slice(1) : req.params.path;
        const now = Date.now();
        const user: User = req.user as User;
        if (user == null) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        const user_group: Group = await groupRepo.findOne({ where: { users: user } }) as Group;

        try {
            await fs.mkdir(path_manipulator.resolve(FS_PATH, path));
            const directory = {
                path: path,
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
            return res.status(200).end();
        } catch (err: any) {
            if (err.code === 'EEXIST') { // Error Exists
                return res.status(409).json({ error: 'Folder already exists' });
            } else {
                return res.status(500).json({ error: 'Not possible to create the folder ' + path, details: err });
            }
        }
    }

    public rmdir = async (req: Request, res: Response) => {
        if (req.params.path === undefined)
            return res.status(400).json({ error: 'Bad format: path parameter is missing' });
        const path: string = req.params.path.startsWith('/') ? req.params.path.slice(1) : req.params.path;

        try {
            const dir: File = await fileRepo.findOne({ where: { path }, relations: ['owner'] }) as File;
            if (!this.has_permissions(dir, 1, req.user as User))
                return res.status(403).json({ error: 'EACCES', message: 'You have not the permission to remove the directory ' + path });

            if (dir.type != 1) {
                return res.status(400).json({ error: 'ENOTDIR', message: 'The specified path is not a directory' });
            }

            await fs.rm(path_manipulator.resolve(FS_PATH, path), { recursive: true }); // fs.rmdirwill be depreacted
            await fileRepo.remove(dir);
            res.status(200).end();
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'Directory not found' });
            } else {
                res.status(500).json({ error: 'Not possible to remove the directory ' + path, details: err });
            }
        }
    }

    public create = async (req: Request, res: Response) => {

        if (req.params.path === undefined)
            return res.status(400).json({ error: 'Bad format: path parameter is missing' });
        const path: string = req.params.path.startsWith('/') ? req.params.path.slice(1) : req.params.path;
        const now = Date.now();
        const user: User = req.user as User;
        if (user === null) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        const user_group: Group = await groupRepo.findOne({ where: { users: user } }) as Group;

        try {
            await fs.writeFile(path_manipulator.resolve(FS_PATH, path), "", { flag: "wx" });

            const file: File = {
                path: path,
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
                res.status(500).json({ error: 'Not possible to create the file ' + path, details: err });
            }
        }
    }

    public write = async (req: Request, res: Response) => {
        if (req.params.path === undefined || req.body.text === undefined)
            return res.status(400).json({ error: 'Bad format: path or text parameter is missing' });
        const path: string = req.params.path.startsWith('/') ? req.params.path.slice(1) : req.params.path;
        const text: string = req.body.text;
        const user: User = req.user as User;
        if (user === null) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        const user_group: Group = await groupRepo.findOne({ where: { users: user } }) as Group;

        try {

            const file: File = await fileRepo.findOne({
                where: { path },
                relations: ['owner', 'group']
            }) as File;
            if (!this.has_permissions(file, 1, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to write on the file ' + path });

            await fs.writeFile(path_manipulator.resolve(FS_PATH, path), text, { flag: "w" });

            res.status(200).json({ ...file, owner: user.uid, group: user_group?.gid });
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                res.status(403).json({ error: 'Access denied' });
            } else {
                res.status(500).json({ error: 'Not possible to write on file ' + path, details: err });
            }
        }
    }

    // returns an object containing the field "data", associated to the file content
    public open = async (req: Request, res: Response) => {

        if (req.params.path === undefined)
            return res.status(400).json({ error: 'Bad format: path parameter is missing' });
        const path: string = req.params.path.startsWith('/') ? req.params.path.slice(1) : req.params.path;

        try {
            const file: File = await fileRepo.findOne({
                where: { path },
                relations: ['owner', 'group']
            }) as File;
            if (file === null)
                return res.status(404).json({ error: 'File not found' }); res.status(404).json({ error: 'File not found' });
            if (!this.has_permissions(file, 0, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to read the content the file ' + path });

            const content = await fs.readFile(path_manipulator.resolve(FS_PATH, path), { flag: "r" });
            res.json({ ...file, owner: file.owner.uid, group: file.group?.gid, data: content.toString() });
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                res.status(403).json({ error: 'Access denied' });
            } else {
                res.status(500).json({ error: 'Not possible to read the file ' + path, details: err });
            }
        }
    }

    public unlink = async (req: Request, res: Response) => {
        if (req.params.path == undefined)
            return res.status(400).json({ error: 'Bad format: path parameter is missing' });
        const path: string = req.params.path.startsWith('/') ? req.params.path.slice(1) : req.params.path;
        const user: User = req.user as User;
        if (user === null) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        const user_group: Group = await groupRepo.findOne({ where: { users: user } }) as Group;
        try {
            const file: File = await fileRepo.findOne({
                where: { path },
                relations: ['owner', 'group']
            }) as File;

            if (!file) {
                return res.status(404).json({ error: 'File metadata not found in database' });
            }
            if (!this.has_permissions(file, 1, req.user as User))
                return res.status(403).json({ err: 'You have not the permission to delete the file ' + path });

            await fs.rm(path_manipulator.resolve(FS_PATH, path));
            await fileRepo.remove(file);
            res.status(200).json({ ...file, owner: user.uid, group: user_group?.gid });
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else {
                res.status(500).json({ error: 'Not possible to remove the file ' + path, details: err });
            }
        }
    }

    public rename = async (req: Request, res: Response) => {

        if (req.params.path === undefined || req.body.new_name === undefined)
            return res.status(400).json({ error: 'Bad format: old path or new file name parameter is missing' });
        const old_path: string = req.params.path.startsWith('/') ? req.params.path.slice(1) : req.params.path;
        const new_name: string = req.body.new_name;
        const new_path: string = path_manipulator.join(path_manipulator.dirname(old_path), new_name);

        try {

            const file: File = await fileRepo.findOne({
                where: { path: old_path },
                relations: ['owner', 'group']
            }) as File;
            if (!this.has_permissions(file, 1, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to rename on the file ' + old_path });

            await fs.rename(path_manipulator.join(FS_PATH, old_path), path_manipulator.join(FS_PATH, new_path));
            await fileRepo.remove(file);
            const new_file = fileRepo.create({
                ...file,
                path: new_path
            });
            await fileRepo.save(new_file);
            res.status(200).json({ ...new_file, owner: new_file.owner.uid, group: new_file.group?.gid });
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                res.status(403).json({ error: 'Access denied' });
            } else {
                res.status(500).json({ error: 'Not possible to rename ' + old_path, details: err });
            }
        }
    }

    public setattr = async (req: Request, res: Response) => {
        const path: string = req.params.path.startsWith('/') ? req.params.path.slice(1) : req.params.path;
        const new_mod: Mode = parseInt(req.body.new_mod);
        if (path == undefined)
            return res.status(400).json({ error: 'Bad format: path parameter is missing' });

        if (isNaN(new_mod)) {
            return res.status(400).json({ error: "Parameter 'mod' is not a valid number" });
        }

        if (new_mod < 0 || new_mod > 0o777) {
            return res.status(400).json({ error: "Parameter 'mod' out of range (0-511)" });
        }

        try {

            let file: File = await fileRepo.findOne({
                where: { path },
                relations: ['owner', 'group']
            }) as File;
            if (!this.has_permissions(file, 1, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to chane mod of the file ' + path });


            file.permissions = new_mod;
            await fileRepo.save(file);

            await fs.chmod(path_manipulator.join(FS_PATH, path), new_mod);
            res.status(200).json({ ...file, owner: file.owner.uid, group: file.group?.gid });
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                res.status(403).json({ error: 'Access denied' });
            } else {
                res.status(500).json({ error: 'Not possible to change mod of ' + path, details: err });
            }
        }
    }

    public getattr = async (req: Request, res: Response) => {
        const path: string = req.params[0].startsWith('/') ? req.params[0].slice(1) : req.params[0];
        
        if (path == undefined)
            return res.status(400).json({ error: 'Bad format: path parameter is missing' });

        

        try {

            let file: File = await fileRepo.findOne({
                where: { path },
                relations: ['owner', 'group']
            }) as File;
            if (!this.has_permissions(file, 1, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to chane mod of the file ' + path });

            res.status(200).json({...file, owner: file.owner.uid, group: file.group?.gid});
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                res.status(403).json({ error: 'Access denied' });
            } else {
                res.status(500).json({ error: 'Not possible to change mod of ' + path, details: err });
            }
        }
    }

}