import { Request, Response } from 'express';
import { promises as fs, Mode } from 'fs';
import path_manipulator from 'path';
import { AppDataSource } from '../data-source';
import { File } from '../entities/File';
import { User } from '../entities/User';
import { Group } from '../entities/Group';

const FS_PATH = path_manipulator.join(__dirname, '..', '..', 'file-system');
const fileRepo = AppDataSource.getRepository(File);
const userRepo = AppDataSource.getRepository(User);
const groupRepo = AppDataSource.getRepository(Group);

export class FileSystemController {

    private has_permissions = async(full_path: string): Promise<boolean> => {

        

        return false;
    }
    
    // recursive call, to show the entire tree
    private read_dir = async (path: string, depth = 0): Promise<any[]> => {
        let content = [];
        const files_dirs = await fs.readdir(path, { withFileTypes: true });
        for (const direct of files_dirs) {
            if (direct.name === '.' || direct.name === '..') continue;
            const fullPath = path_manipulator.resolve(FS_PATH, `${path}/${direct.name}`);
            if (direct.isDirectory()) {
                let dir_content = await this.read_dir(fullPath, depth + 1);
                content.push({ [direct.name]: dir_content });
            } else {
                content.push(direct.name);
            }
        }
        return content;
    }
    
    public readdir = async (req: Request, res: Response) => {
        const path: string = req.body.path;
        try {
            const files = await fs.readdir(path_manipulator.resolve(FS_PATH, `${path}`));
            const content = await Promise.all(
                files.map(async (file_name) => {
                    const fullPath = path_manipulator.join(FS_PATH, path, file_name);
                    const file: File = (await fileRepo.findOne({ where: { path: fullPath } })) as File;
                    if(file == null) {
                        return res.status(500).json({ error: 'Mismatch bewteen the file system and the database conatining metadata for file ' + path_manipulator.resolve(path) });
                    }
                    return file;
                })
            );
            res.json(content);
        } catch (err) {
            res.status(500).json({ error: 'Not possible to read from the folder ' + path_manipulator.resolve(path), details: err });
        }
    }

    public mkdir = async (req: Request, res: Response) => {
        const path: string = req.body.path;
        const name: string = req.params.name;
        const now = Date.now();
        const user: User = await userRepo.findOne({ where: { username: "pippo" } }) as User;
        if(user == null){
            res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        const user_group: Group = await groupRepo.findOne({ where: { users: user }}) as Group;
        if(user == null){
            res.status(500).json({ error: 'Not possible to retreive user0s group' });
        }
        
        try {
            await fs.mkdir(path_manipulator.resolve(FS_PATH, `${path}/${name}`));
            const directory = {
                path: path_manipulator.resolve(FS_PATH, path, name),
                name: name,
                owner: user,
                type: 1,
                permissions: 0x777,
                group: user_group,
                size: 0,
                atime: now,
                mtime: now,
                ctime: now,
                btime: now
            } as File;
            await fileRepo.save(directory);
            res.status(200).end();
        } catch (err: any) {
            if (err.code === 'EEXIST') { // Error Exists
                res.status(400).json({ error: 'Folder already exists' });
            } else {
                res.status(500).json({ error: 'Not possible to create the folder ' + path, details: err });
            }
        }
    }

    public rmdir = async (req: Request, res: Response) => {
        const path: string = req.body.path;
        const name: string = req.params.name;
        try {
            const dir: File = await fileRepo.findOne({ where: { name } }) as File;
            if (dir.type != 1) {
                return res.status(400).json({ error: 'ENOTDIR', message: 'The specified path is not a directory' });
            }
            await fs.rmdir(path_manipulator.resolve(FS_PATH, `${path}/${name}`), { recursive: true });
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
        try {
            await fs.writeFile(path_manipulator.resolve(FS_PATH, `${path}/${name}`), "", {flag: "wx"});
            res.status(200).end();
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'Directory not found' });
            } else if (err.code === 'EEXIST') {
                res.status(400).json({ error: 'File already exists' });
            } else {
                res.status(500).json({ error: 'Not possible to create the file ' + name, details: err });
            }
        }
    }
    
    public write = async (req: Request, res: Response) => {
        const path: string = req.body.path;
        const name: string = req.params.name;
        const text: string = req.body.text;
        try {
            await fs.writeFile(path_manipulator.resolve(FS_PATH, `${path}/${name}`), text, {flag: "w"});
            // check permissions!
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

        // check permissions!
        try {
            const content = await fs.readFile(path_manipulator.resolve(FS_PATH, `${path}/${name}`), {flag: "r"}); // tiene conto dei permessi!
            res.json({data: content.toString()});
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

        // check permissions!
        try {
            await fs.rm(path_manipulator.resolve(FS_PATH, `${path}/${name}`));
            const file: File = await fileRepo.findOne({ where: { name } }) as File;
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

        const old_path = path_manipulator.resolve(FS_PATH, `${path}/${old_name}`);
        const new_path = path_manipulator.resolve(FS_PATH, `${path}/${new_name}`);

        // check permissions!
        try {
            await fs.rename(old_path, new_path);
            const file: File = await fileRepo.findOne({ where: { path: old_path } }) as File;
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

        // check permissions!

        if (isNaN(new_mod)) {
            return res.status(400).json({ error: "Parameter 'mod' is not a valid number" });
        }

        if (new_mod < 0 || new_mod > 0o777) {
            return res.status(400).json({ error: "Parameter 'mod' out of range (0-511)" });
        }

        try {
            const file: File = await fileRepo.findOne({ where: { path: path_manipulator.resolve(FS_PATH, `${path}/${name}`) } }) as File;
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