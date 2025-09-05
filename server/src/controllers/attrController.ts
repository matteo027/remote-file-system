import { Request, Response } from 'express';
import { fileRepo,userRepo,toFsPath,has_permissions,normalizePath} from './utility';
import { File } from '../entities/File';
import { User } from '../entities/User';
import * as fs from 'node:fs/promises';

export class AttributeController{
    public readdir = async (req: Request, res: Response) => {
        const inoRec = BigInt(req.params.ino);
        console.log(inoRec)
        if(!inoRec) 
            return res.status(400).json({message:"Missing ino"});

        try{
            const dir=await fileRepo.findOne({where: {ino:inoRec}, relations: ['owner', 'group'] }) as File | null;
            if (!dir) 
                return res.status(404).json({error: "ENOENT", message: `Directory with ino=${inoRec} not found` });
            if (dir.type !==1 )
                return res.status(400).json({error: "ENOTDIR", message:`${dir.path} is not a directory`});

            if (!(dir.path === "/" || has_permissions(dir, 0, req.user as User))) {
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

                    if (!has_permissions(file, 0, req.user as User)) return undefined;

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

    public setattr = async (req: Request, res: Response) => {
        const inoRec=BigInt(req.params.ino);
        if(!inoRec)
            return res.status(400).json({ error: "EINVAL", message: "Invalid inode" });

        let {
            perm: rawPerm,
            uid: rawUid,
            gid: rawGid,
            size: rawSize,
            // flags: rawFlags
        } = req.body ?? {};

        try{
            const file = await fileRepo.findOne({
                where:{ino:inoRec},
                relations:["owner","group"],
            }) as File;
            if (!file)
                return res.status(404).json({ error: "ENOENT", message: "File not found" });

            const fullFsPath=toFsPath(file.path);

            if(rawUid!=null || rawGid != null){
                const user=await userRepo.findOne({where:{uid:rawUid}});
                if(!user){
                    file.owner=req.user as User;
                    file.group=(req.user as User).group ?? null;
                }else{
                    // DA CONTROLLARE
                    file.owner=user;
                    file.group=user.group?? null;
                }
                await fileRepo.save(file);
            }

            let newPerm: number|undefined;
            if(rawPerm!=null){
                const n = typeof rawPerm === "number" ? rawPerm : parseInt(String(rawPerm), 10);
                if ( n < 0 || n > 0o777) {
                    return res.status(400).json({ error: "EINVAL", message: "Invalid mode (0..0o777)" });
                }
                newPerm = n;
            }

            let newSize: number | undefined;
            if (rawSize != null) {
                const n = typeof rawSize === "number" ? rawSize : parseInt(String(rawSize), 10);
                if ( n < 0) {
                    return res.status(400).json({ error: "EINVAL", message: "Invalid size" });
                }
                newSize = n;
            }

            if (!has_permissions(file,1, req.user as User)) 
                return res.status(403).json({ error: "EACCES", message: `No permission on ${file.path}` });

            if(newPerm !== undefined && newPerm != file.permissions){
                file.permissions=newPerm;
            }

            if(newSize != undefined){
                if (file.type === 1) {
                    return res.status(400).json({ error: "EISDIR", message: "Cannot truncate a directory" });
                }
                await fs.truncate(fullFsPath, newSize);
            }

            await fileRepo.save(file);

            const stats=await fs.lstat(fullFsPath,{bigint:true});
            return res.status(200).json({
                ino: file.ino.toString,
                path: file.path,
                type: file.type,
                permission: file.permissions,
                owner: file.owner?.uid ?? null,
                group: file.group?.gid ?? null,
                size: stats.size,
                atime: stats.atime.getTime(),
                mtime: stats.mtime.getTime(),
                ctime: stats.ctime.getTime(),
                btime: stats.birthtime.getTime(),
            })
        } catch (err:any){
            if (err?.code === "ENOENT") 
                return res.status(404).json({ error: "ENOENT", message: "Filesystem path not found" });
            if (err?.code === "EACCES")
                return res.status(403).json({ error: "EACCES", message: "Access denied" });
            console.error(err);
            return res.status(500).json({ error: "EIO", message: "Unable to update attributes", details: String(err?.message ?? err) });
        }
    }

    public lookup = async (req: Request, res: Response) => {
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
            if (!has_permissions(file, 0, req.user as User))
                return res.status(403).json({ error: 'You have not the permission to visualize the file ' + dbPath });
            const stats = await fs.stat(toFsPath(dbPath));

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