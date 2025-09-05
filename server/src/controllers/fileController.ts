import { Request, Response } from 'express';
import { fileRepo,groupRepo,toFsPath,has_permissions} from './utility';
import { File } from '../entities/File';
import { User } from '../entities/User';
import { Group } from '../entities/Group';
import * as fs from 'node:fs/promises';

export class FileController{
    public mkdir = async (req: Request, res: Response) => {
        const parentIno=BigInt(req.params.parentIno);
        const name = req.params.name;

        if(!parentIno)
            return res.status(400).json({ error: "EINVAL", message: "Parent inode missing" });
        if (!name || typeof name !== "string" || name.length === 0 || name==="." || name==="..")
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

            if(!has_permissions(parent,2,user)){
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
        const name = req.params.name;

        if(!parentIno)
            return res.status(400).json({ error: "EINVAL", message: "Parent inode missing" });
        if (!name || typeof name !== "string" || name.length === 0 || name==="." || name==="..")
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
            
            if(!has_permissions(parent,1,user)){
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
        const name = req.params.name;

        if(!parentIno)
            return res.status(400).json({ error: "EINVAL", message: "Parent inode missing" });
        if (!name || typeof name !== "string" || name.length === 0 || name==="." || name==="..")
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

            if(!has_permissions(parent,1,user)){
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
        const name = req.params.name;

        if(!parentIno)
            return res.status(400).json({ error: "EINVAL", message: "Parent inode missing" });
        if (!name || typeof name !== "string" || name.length === 0 || name==="." || name==="..")
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
            if(!has_permissions(parent,1,user)){
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

    public rename = async (req: Request, res: Response) => {
        const oldParentIno=BigInt(req.params.oldParentIno);
        const oldName=req.params.oldName;

        const {newParentIno, newName} =req.body ?? {};
        const newParentInode=BigInt(newParentIno);
        const badName = (s: any) => typeof s !== "string" || s === "" || s === "." || s === ".." ;
        if(!oldParentIno || !newParentInode){
            return res.status(400).json({ error: "EINVAL", message: "Invalid parent inode(s)" });
        }

        if(badName(oldName) || badName(newName)){
            return res.status(400).json({ error: "EINVAL", message: "Invalid name(s)" });
        }

        const user=req.user as User;

        try{
            const[oldParent,newParent]=await Promise.all([
                fileRepo.findOne({ where: { ino: oldParentIno }, relations: ["owner","group"] }),
                fileRepo.findOne({ where: { ino: newParentInode }, relations: ["owner","group"] }),
            ]);

            if (!oldParent) 
                return res.status(404).json({ error: "ENOENT", message: "Old parent not found" });
            if (!newParent) 
                return res.status(404).json({ error: "ENOENT", message: "New parent not found" });

            if (oldParent.type !== 1 || newParent.type !== 1) 
                return res.status(400).json({ error: "ENOTDIR", message: "Parent(s) must be directories" });
            
            if(!has_permissions(oldParent,1,user) || !has_permissions(newParent,1,user)){
                return res.status(403).json({ error: "EACCES", message: `Insufficient permissions` });
            }

            const oldPath = oldParent.path === "/" ? `/${oldName}` : `${oldParent.path}/${oldName}`;
            const newPath = newParent.path === "/" ? `/${newName}` : `${newParent.path}/${newName}`;
            const fullOld = toFsPath(oldPath);
            const fullNew = toFsPath(newPath);

            const entry = await fileRepo.findOne({ where: { path: oldPath }, relations: ["owner","group"] }) as File;
            if (!entry) 
                return res.status(404).json({ error: "ENOENT", message: "Source entry not found" });

            try{
                await fs.rename(fullOld,fullNew);
            }catch(err:any){
                if (err?.code === "ENOENT")   
                    return res.status(404).json({ error: "ENOENT", message: "Source or target dir missing" });
                if (err?.code === "EEXIST")   
                    return res.status(409).json({ error: "EEXIST", message: "Target exists" });
                throw err;
            }
            await fileRepo.update({ino:entry.ino}, { path: newPath });
            console.log("saved");
            const stats=await fs.lstat(fullNew,{bigint:true});
            return res.status(200).json({
                ino: entry.ino.toString(),
                path: newPath,
                type: entry.type,
                permissions: entry.permissions,
                owner: entry.owner?.uid ?? null,
                group: entry.group?.gid ?? null,
                size: stats.size.toString(),
                atime: stats.atime.getTime(),
                mtime: stats.mtime.getTime(),
                ctime: stats.ctime.getTime(),
                btime: stats.birthtime.getTime(),
            })
        }catch(err:any){
            return res.status(500).json({ error: "EIO", message: "Not possible to rename", details: String(err?.message ?? err) });
        }
    }
}