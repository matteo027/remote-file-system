import { Request, Response } from 'express';
import { fileRepo,userRepo,toFsPath,has_permissions,parseIno,toEntryJson,isBadName,childPathOf, pathRepo} from '../utilities';
import { File } from '../entities/File';
import { User } from '../entities/User';
import * as fs from 'node:fs/promises';
import { Path } from '../entities/Path';
import path from 'node:path';

export class AttributeController{
    public readdir = async (req: Request, res: Response) => {
        const inoRec = parseIno(req.params.ino);
        if(!inoRec) 
            return res.status(400).json({message:"Missing ino"});

        try{
            const dir=await fileRepo.findOne({where: {ino:inoRec}, relations: ['owner', 'group', 'paths'] }) as File | null;
            if (!dir) 
                return res.status(404).json({error: "ENOENT", message: `Directory with ino=${inoRec} not found` });
            if (dir.type !==1 )
                return res.status(400).json({error: "ENOTDIR", message:`${inoRec} is not a directory`});

            if (!has_permissions(dir, 0, req.user as User)) {
                return res.status(403).json({ error: "EACCES", message: `You have not the permission to list ${inoRec}` });
            }
            const fullFsPath=toFsPath(dir.paths[0].path);
            const names=await fs.readdir(fullFsPath);

            const rows= await Promise.all(
                names.map(async (name) => {
                    const childDbPath = childPathOf(dir.paths[0].path, name);
                    const pathObj = await pathRepo.findOne({where:{path:childDbPath}, relations:["file", "file.owner", "file.group"]}) as Path | null;
                    
                    if (!pathObj) return undefined;

                    const file = pathObj?.file;
                    const stats = await fs.lstat(toFsPath(childDbPath), { bigint: true });

                    if (!file) {
                        // mismatch FS↔DB
                        throw new Error(`Mismatch FS/DB for ${childDbPath} (ino=${stats.ino})`);
                    }

                    if (!has_permissions(file, 0, req.user as User)) return undefined;
                    
                    return toEntryJson(file, stats, pathObj);
                })
            );
            const content=rows.filter(Boolean);
            return res.status(200).json(content);
        }catch (err:any){
            if (err?.code === "ENOENT") {
                return res.status(404).json({ error: "ENOENT", message: "Directory not found on filesystem" });
            }
            return res.status(500).json({ error: "EIO", message: `Not possible to read the folder (ino=${inoRec})`, details: String(err?.message ?? err) });
        }
    }

    public setattr = async (req: Request, res: Response) => {
        const inoRec=parseIno(req.params.ino);
        if(!inoRec)
            return res.status(400).json({ error: "EINVAL", message: "Invalid inode" });

        let {
            perm: rawPerm,
            uid: rawUid,
            gid: rawGid,
            size: rawSize
        } = req.body ?? {};

        try{
            const file = await fileRepo.findOne({
                where:{ino:inoRec},
                relations:["owner", "group", "paths"],
            }) as File | null;
            
            if (!file)
                return res.status(404).json({ error: "ENOENT", message: "File not found" });

            if (!has_permissions(file,1, req.user as User)) 
                return res.status(403).json({ error: "EACCES", message: `No permission on ${inoRec}` });

            const fullFsPath=toFsPath(file.paths[0].path); // every path is valid, so we can take the first one

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
                await fileRepo.update({ino:file.ino}, { owner: file.owner, group: file.group });
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

            if(newPerm !== undefined && newPerm != file.permissions){
                file.permissions=newPerm;
                await fileRepo.update({ino:file.ino}, { permissions: newPerm });
            }

            if(newSize != undefined){
                if (file.type === 1) {
                    return res.status(400).json({ error: "EISDIR", message: "Cannot truncate a directory" });
                }
                await fs.truncate(fullFsPath, newSize);
            }
            
            const stats=await fs.lstat(fullFsPath);
            
            return res.status(200).json(toEntryJson(file, stats, file.paths[0]));
        } catch (err:any){
            if (err?.code === "ENOENT") 
                return res.status(404).json({ error: "ENOENT", message: "Filesystem path not found" });
            if (err?.code === "EACCES")
                return res.status(403).json({ error: "EACCES", message: "Access denied" });
            return res.status(500).json({ error: "EIO", message: "Unable to update attributes", details: String(err?.message ?? err) });
        }
    }

    // serve per ottenere l'ino del file figlio dato il nome e l'ino della cartella padre
    public lookup = async (req: Request, res: Response) => {
        const parentIno=parseIno(req.params.parentIno);
        const name= req.params.name;

        if(!parentIno)
            return res.status(400).json({ error: "EINVAL", message: "Parent inode missing" });
        if (isBadName(name))
            return res.status(400).json({ error: "EINVAL", message: "Invalid directory name" });

        try{
            const parentDir=await fileRepo.findOne({where:{ino:parentIno}, relations:['owner', 'group', 'paths']}) as File;
            if(!parentDir)
                return res.status(404).json({ error: "ENOENT", message: "Parent directory not found" });
            if(parentDir.type !==1)
                return res.status(400).json({ error: "ENOTDIR", message: "Parent is not a directory" });

            if(!has_permissions(parentDir,0, req.user as User))
                return res.status(403).json({ error: "EACCES", message: `No permission to read ${parentDir.paths[0].path}` });

            const childDbPath = childPathOf(parentDir.paths[0].path, name);
            const childFsPath = toFsPath(childDbPath);

            let stats;
            try{
                stats = await fs.lstat(childFsPath, { bigint: true });
            } catch (e:any){
                if(e?.code === "ENOENT"){
                    return res.status(404).json({ error: "ENOENT", message: `File ${name} not found in ${parentDir.paths[0].path}` });
                }
                throw e;
            }

            const childFile = await fileRepo.findOne({
                where: { ino: stats.ino.toString() },
                relations: ["owner", "group", "paths"],
            }) as File;
            const childPathObj = await pathRepo.findOne({ where: { path: childDbPath }, relations: ["file"] }) as Path | null;

            if (!childFile) {
                // mismatch FS↔DB
                return res.status(500).json({ error: "EIO", message: `Mismatch FS/DB for ${childDbPath} (ino=${stats.ino})` });
            }
            if (!childPathObj) {
                // mismatch FS↔DB
                return res.status(500).json({ error: "EIO", message: `Mismatch FS/DB for ${childDbPath}'s path (ino=${stats.ino})` });
            }

            return res.status(200).json(toEntryJson(childFile, stats, childPathObj));
        }catch (err:any){
            return res.status(500).json({
                error: "EIO",
                message: "Lookup failed",
                details: String(err?.message ?? err),
            });
        }
    }

    public getattr = async (req: Request, res: Response) => {
        
        const inoRec=parseIno(req.params.ino);
        const isModifiedHeader = req.header('if-modified-since');
        if(!inoRec)
            return res.status(400).json({ error: "EINVAL", message: "Invalid inode" });

        try{
            const file = await fileRepo.findOne({
                where:{ino:inoRec},
                relations:["owner", "group", "paths"],
            }) as File | null;

            if (!file)
                return res.status(404).json({ error: "ENOENT", message: "File not found" });
            
            if(!has_permissions(file,0, req.user as User))
                return res.status(403).json({ error: "EACCES", message: `You have not the permission to read ${inoRec}` });

            const fullFsPath=toFsPath(file.paths[0].path); // every path is valid, so we can take the first one
            const stats=await fs.lstat(fullFsPath,{bigint:true});

            const lastModifiedSecond = Math.floor(stats.mtime.getTime() / 1000);
            if (isModifiedHeader) {
                const isModifiedMs = Date.parse(isModifiedHeader);
                if (!Number.isNaN(isModifiedMs)) {
                    const isModifiedSeconds = Math.floor(isModifiedMs / 1000);
                    if (lastModifiedSecond <= isModifiedSeconds) {
                        return res.status(304).end(); // Not Modified
                    }
                }
            }

            const lastModifiedHttp=(new Date(lastModifiedSecond * 1000)).toUTCString();
            res.setHeader('Last-Modified', lastModifiedHttp);

            return res.status(200).json(toEntryJson(file, stats, file.paths[0]));
        }catch (err:any){
            if (err.code === 'ENOENT') 
                return res.status(404).json({ error: 'File not found' });
            if (err.code === 'EACCES') 
                return res.status(403).json({ error: 'Access denied' });
            return res.status(500).json({ error: 'Not possible to perform the operation', details: err });
        }
    }
}