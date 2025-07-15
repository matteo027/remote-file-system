import { Request, Response } from 'express';
import { promises as fs } from 'fs';

const FS_PATH = "."; // DA MODIFICARE! E' LA "ROOT" DEL FILE SYSTEM

export class FileSystemController {
    
    private read_dir = async (path: string, depth = 0): Promise<any[]> => {
        let content = [];
        const files_dirs = await fs.readdir(path, { withFileTypes: true });
        for (const dirent of files_dirs) {
            if (dirent.name === '.' || dirent.name === '..') continue;
            const fullPath = `${FS_PATH}/${path}/${dirent.name}`;
            if (dirent.isDirectory()) {
                let dir_content = await this.read_dir(fullPath, depth + 1);
                content.push({ [dirent.name]: dir_content });
            } else {
                content.push(dirent.name);
            }
        }
        return content;
    }
    
    public readdir = async (req: Request, res: Response) => {
        const path: string = req.body.path;
        try {
            const files = await fs.readdir(path);
            const content = await Promise.all(
                files.map(async (file) => {
                    const fullPath = `${FS_PATH}/${path}/${file}`;
                    const stats = await fs.stat(fullPath);
                    return {
                        name: file,
                        isDirectory: stats.isDirectory(),
                        size: stats.size,
                        mode: stats.mode, // permessi in formato numerico
                        mtime: stats.mtime // data ultima modifica
                    };
                })
            );
            res.json(content);
        } catch (err) {
            res.status(500).json({ error: 'Not possible to read from the folder ' + path, details: err });
        }
    }

    public mkdir = async (req: Request, res: Response) => {
        const path: string = req.body.path;
        const name: string = req.params.name;
        try {
            await fs.mkdir(`${FS_PATH}/${path}/${name}`);
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
            const stats = await fs.stat(`${path}/${name}`);
            if (!stats.isDirectory()) {
                return res.status(400).json({ error: 'ENOTDIR', message: 'The specified path is not a directory' });
            }
            await fs.rmdir(`${path}/${name}`, { recursive: true });
            res.status(200).end();
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'Folder not found' });
            } else {
                res.status(500).json({ error: 'Not possible to remove the folder ' + path, details: err });
            }
        }
    }

    public create = async (req: Request, res: Response) => {
        const path: string = req.body.path;
        const name: string = req.params.name;
        try {
            await fs.writeFile(`${FS_PATH}/${path}/${name}`, "", {flag: "wx"});
            res.status(200).end();
        } catch (err: any) {
            if (err.code === 'ENOENT') {
                res.status(404).json({ error: 'Folder not found' });
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
            await fs.writeFile(`${FS_PATH}/${path}/${name}`, text, {flag: "w"}); // tiene conto dei permessi!
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

    // restituisce un oggetto che contiene il campo "data", associato al contenuto del file
    public open = async (req: Request, res: Response) => {
        const path: string = req.body.path;
        const name: string = req.params.name;
        try {
            const content = await fs.readFile(`${FS_PATH}/${path}/${name}`, {flag: "r"}); // tiene conto dei permessi!
            res.status(200).json({data: content.toString()});
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
        try {
            await fs.rm(`${FS_PATH}/${path}/${name}`);
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
        try {
            const content = await fs.rename(`${FS_PATH}/${path}/${old_name}`, `${FS_PATH}/${path}/${new_name}`);
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
    

}