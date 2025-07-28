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
        const path: string = req.body.path;
        if (path == undefined)
            return res.status(400).json({ error: 'Bad format: path field is missing' });

        try {
            const files = await fs.readdir(path_manipulator.resolve(FS_PATH, `${path}`));
            const content = await Promise.all(
                files.map(async (file_name) => {
                    const fullPath = path_manipulator.join(FS_PATH, path, file_name);

                    const file: File = await fileRepo.findOne({ where: { path: fullPath }, relations: ['owner'] }) as File;
                    if (file == null) {
                        throw new Error(`Mismatch between the file system and the database for file: ${fullPath}`);
                    }
                    return { ...file, owner: file.owner.uid };
                })
            );
            return res.json(content);
        } catch (err) {
            return res.status(500).json({ error: 'Not possible to read from the folder ' + path_manipulator.resolve(FS_PATH, path), details: err });
        }
    }

    public mkdir = async (req: Request, res: Response) => {
        const path: string = req.body.path;
        const name: string = req.params.name;
        const now = Date.now();
        const user: User = req.user as User;

        if (path == undefined)
            return res.status(400).json({ error: 'Bad format: path field is missing' });

        if (user == null) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        const user_group: Group = await groupRepo.findOne({ where: { users: user } }) as Group;

        try {
            await fs.mkdir(path_manipulator.resolve(FS_PATH, path.startsWith('/') ? path.slice(1) : path, name));
            const directory = {
                path: path_manipulator.resolve(FS_PATH, path.startsWith('/') ? path.slice(1) : path, name),
                name: name,
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
            console.log("dir: ", directory);
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
        const path: string = req.body.path;
        const name: string = req.params.name;
        if (path == undefined)
            return res.status(400).json({ error: 'Bad format: path field is missing' });

        try {
            const dir: File = await fileRepo.findOne({ where: { name }, relations: ['owner'] }) as File;
            console.log("OKKK");
            if (!this.has_permissions(dir, 1, req.user as User))
                return res.status(403).json({ error: 'EACCES', message: 'You have not the permission to remove the directory ' + path_manipulator.resolve(path, name) });

            if (dir.type != 1) {
                return res.status(400).json({ error: 'ENOTDIR', message: 'The specified path is not a directory' });
            }

            await fs.rmdir(path_manipulator.resolve(FS_PATH, path.startsWith('/') ? path.slice(1) : path, name), { recursive: true });
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
        const path: string = req.body.path;
        const name: string = req.params.name;
        const now = Date.now();
        const user: User = req.user as User;
        if (path == undefined)
            return res.status(400).json({ error: 'Bad format: path field is missing' });

        if (user == null) {
            res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        const user_group: Group = await groupRepo.findOne({ where: { users: user } }) as Group;
        if (user == null) {
            res.status(500).json({ error: 'Not possible to retreive user\'s group' });
        }
        try {
            await fs.writeFile(path_manipulator.resolve(FS_PATH, `${path}/${name}`), "", { flag: "wx" });
            const file: File = {
                path: path_manipulator.resolve(FS_PATH, path, name),
                name: name,
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
            res.status(200).json({...file, owner: user.uid, group: user_group.gid}).end();
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'Directory not found' });
            } else if (err.code === 'EEXIST') {
                res.status(409).json({ error: 'File already exists' });
            } else {
                res.status(500).json({ error: 'Not possible to create the file ' + name, details: err });
            }
        }
    }

    public write = async (req: Request, res: Response) => {
        const path: string = req.body.path;
        const name: string = req.params.name;
        const text: string = req.body.text;
        if (path == undefined)
            return res.status(400).json({ error: 'Bad format: path field is missing' });

        try {

            const file: File = await fileRepo.findOne({ where: { path: path_manipulator.resolve(FS_PATH, `${path}/${name}`) } }) as File;
            if (!this.has_permissions(file, 1, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to write on the file ' + path_manipulator.resolve(path, name) });

            await fs.writeFile(path_manipulator.resolve(FS_PATH, `${path}/${name}`), text, { flag: "w" });

            res.status(200).end();
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                res.status(403).json({ error: 'Access denied' });
            } else {
                res.status(500).json({ error: 'Not possible to write on file ' + name, details: err });
            }
        }
    }

    // returns an object containing the field "data", associated to the file content
    public open = async (req: Request, res: Response) => {
        const path: string = req.body.path;
        const name: string = req.params.name;
        if (path == undefined)
            return res.status(400).json({ error: 'Bad format: path field is missing' });

        try {

            const file: File = await fileRepo.findOne({ where: { path: path_manipulator.resolve(FS_PATH, path, name) } }) as File;
            if (!this.has_permissions(file, 0, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to read the content the file ' + path_manipulator.resolve(path, name) });

            const content = await fs.readFile(path_manipulator.resolve(FS_PATH, `${path}/${name}`), { flag: "r" }); // tiene conto dei permessi!
            res.json({ data: content.toString() });
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                res.status(403).json({ error: 'Access denied' });
            } else {
                res.status(500).json({ error: 'Not possible to read the file ' + name, details: err });
            }
        }
    }

    public unlink = async (req: Request, res: Response) => {
        const path: string = req.body.path;
        const name: string = req.params.name;
        if (path == undefined)
            return res.status(400).json({ error: 'Bad format: path field is missing' });

        try {

            const file: File = await fileRepo.findOne({ where: { path: path_manipulator.resolve(FS_PATH, path, name) } }) as File;
            if (!this.has_permissions(file, 1, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to delete the file ' + path_manipulator.resolve(path, name) });

            await fs.rm(path_manipulator.resolve(FS_PATH, `${path}/${name}`));
            await fileRepo.remove(file);
            res.status(200).end();
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else {
                res.status(500).json({ error: 'Not possible to remove the file ' + name, details: err });
            }
        }
    }

    public rename = async (req: Request, res: Response) => {
        const path: string = req.body.path;
        const old_name: string = req.params.name;
        const new_name: string = req.body.new_name;
        if (path == undefined)
            return res.status(400).json({ error: 'Bad format: path field is missing' });

        const old_path = path_manipulator.resolve(FS_PATH, `${path}/${old_name}`);
        const new_path = path_manipulator.resolve(FS_PATH, `${path}/${new_name}`);

        try {

            const file: File = await fileRepo.findOne({ where: { path: old_path } }) as File;
            if (!this.has_permissions(file, 1, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to rename on the file ' + path_manipulator.resolve(path, old_name) });

            await fs.rename(old_path, new_path);
            await fileRepo.remove(file);
            const new_file = fileRepo.create({
                ...file,
                path: new_path
            });
            await fileRepo.save(new_file);
            res.status(200).end();
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                res.status(403).json({ error: 'Access denied' });
            } else {
                res.status(500).json({ error: 'Not possible to rename ' + old_name, details: err });
            }
        }
    }

    public setattr = async (req: Request, res: Response) => {
        const path: string = req.body.path;
        const name: string = req.params.name;
        const new_mod: Mode = parseInt(req.body.new_mod);
        if (path == undefined)
            return res.status(400).json({ error: 'Bad format: path field is missing' });

        if (isNaN(new_mod)) {
            return res.status(400).json({ error: "Parameter 'mod' is not a valid number" });
        }

        if (new_mod < 0 || new_mod > 0o777) {
            return res.status(400).json({ error: "Parameter 'mod' out of range (0-511)" });
        }

        try {

            const file: File = await fileRepo.findOne({ where: { path: path_manipulator.resolve(FS_PATH, path, name) } }) as File;
            if (!this.has_permissions(file, 1, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to chane mod of the file ' + path_manipulator.resolve(path, name) });


            await fileRepo.remove(file);
            const new_file = fileRepo.create({
                ...file,
                permissions: new_mod
            });
            await fileRepo.save(new_file);
            res.status(200).end();
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'File not found' });
            } else if (err.code === 'EACCES') {
                res.status(403).json({ error: 'Access denied' });
            } else {
                res.status(500).json({ error: 'Not possible to change mod of ' + name, details: err });
            }
        }
    }

}