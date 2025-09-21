import { Request, Response } from 'express';
import { fileRepo,groupRepo,pathRepo,toFsPath,has_permissions,parseIno,toEntryJson,isBadName,childPathOf} from '../utilities';
import { File } from '../entities/File';
import { User } from '../entities/User';
import { Group } from '../entities/Group';
import * as fs from 'node:fs/promises';
import { Path } from '../entities/Path';
import { permission } from 'node:process';

export class FileController{
    public mkdir = async (req: Request, res: Response) => {
        const parentIno=parseIno(req.params.parentIno);
        const name = req.params.name;

        if(!parentIno)
            return res.status(400).json({ error: "EINVAL", message: "Parent inode missing" });
        if (isBadName(name))
            return res.status(400).json({ error: "EINVAL", message: "Invalid directory name" });

        const user = req.user as User |undefined;
        if (!user)
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        const userGroup = (await groupRepo.findOne({ where: { users: user } })) as Group | null;
        
        try{
            const parent = (await fileRepo.findOne({
                where: { ino: parentIno },
                relations: ["owner", "group", "paths"],
            })) as File | null;

            if (!parent) {
                return res.status(404).json({ error: "ENOENT", message: `Parent inode ${parentIno} not found` });
            }
            if (parent.type !== 1) {
                return res.status(400).json({ error: "ENOTDIR", message: "Parent is not a directory" });
            }

            if(!has_permissions(parent,1,user)){
                return res.status(403).json({ error: "EACCES", message: `No permission to create in ${parentIno}` });
            }

            const childDbPath = childPathOf(parent.paths[0].path, name); // directories have only one path
            const childFsPath = toFsPath(childDbPath);

            await fs.mkdir(childFsPath);
            const stats=await fs.lstat(childFsPath,{bigint:true});
            
            const directory = {
                ino:stats.ino.toString(),
                owner:user,
                group: userGroup,
                type: 1,
                permissions: 0o755,
            } as File;
            await fileRepo.save(directory);

            const childPathObject = {
                file: directory,
                path: childDbPath
            } as Path;
            await pathRepo.save(childPathObject);

            return res.status(201).json(toEntryJson(directory, stats, childPathObject));
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
        const parentIno=parseIno(req.params.parentIno);
        const name = req.params.name;

        if(!parentIno)
            return res.status(400).json({ error: "EINVAL", message: "Parent inode missing" });
        if (isBadName(name))
            return res.status(400).json({ error: "EINVAL", message: "Invalid directory name" });

        const user = req.user as User | undefined;
        if (!user) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }
        try {
            const parent= await fileRepo.findOne({
                where:{ino:parentIno},
                relations:["owner", "group", "paths"],
            })as File | null;
            if (!parent){
                return res.status(404).json({ error: "ENOENT", message: `Parent inode ${parentIno} not found` });
            }
            if (parent.type !== 1) {
                return res.status(400).json({ error: "ENOTDIR", message: "Parent is not a directory" });
            }
            
            if(!has_permissions(parent,1,user)){
                return res.status(403).json({ error: "EACCES", message: `No permission to remove in ${parentIno}` });
            }
            const childDbPath = childPathOf(parent.paths[0].path, name);
            const childFsPath = toFsPath(childDbPath);

            const child= await fileRepo.findOne({
                where: { paths: { path: childDbPath } },
                relations: ["owner", "group", "paths"],
            }) as File | null;

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

            await pathRepo.remove(child.paths.find(p=>p.path===childDbPath) as Path);
            const remainingPaths = await pathRepo.find({ where: { file: child } });
            
            if (remainingPaths.length < 1) {
                await fileRepo.remove(child);
            }
            else // should not happen, but just in case
                return res.status(500).json({ error: "EIO", message: "Directory has multiple paths, manual cleanup required" });
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
        const parentIno=parseIno(req.params.parentIno);
        const name = req.params.name;

        if(!parentIno)
            return res.status(400).json({ error: "EINVAL", message: "Parent inode missing" });
        if (isBadName(name))
            return res.status(400).json({ error: "EINVAL", message: "Invalid directory name" });

        const user = req.user as User | undefined;
        if (!user) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        const userGroup = await groupRepo.findOne({where: {users:user}}) as Group;
        try{
            const parent = await fileRepo.findOne({
                where:{ino:parentIno},
                relations:["owner", "group", "paths"],
            }) as File;

            if (!parent) 
                return res.status(404).json({ error: "ENOENT", message: `Parent inode ${parentIno} not found` });

            if (parent.type !== 1) 
                return res.status(400).json({ error: "ENOTDIR", message: "Parent is not a directory" });

            if(!has_permissions(parent,1,user)){
                return res.status(403).json({ error: "EACCES", message: `No permission to create in ${parentIno}` });
            }

            const childDbPath = childPathOf(parent.paths[0].path, name);
            const childFsPath = toFsPath(childDbPath);

            await fs.writeFile(childFsPath, "", { flag: "wx" });
            const stats=await fs.lstat(childFsPath,{bigint:true});
            const file={
                ino:stats.ino.toString(),
                owner:user,
                group: userGroup ?? null,
                type: 0,
                permissions: 0o644,
            } as File;
            const pathObj={
                file:file,
                path:childDbPath
            } as Path;
            await fileRepo.save(file);
            await pathRepo.save(pathObj);

            return res.status(201).json(toEntryJson(file, stats, pathObj));
        }catch(err:any){
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
        const parentIno=parseIno(req.params.parentIno);
        const name = req.params.name;

        if(!parentIno)
            return res.status(400).json({ error: "EINVAL", message: "Parent inode missing" });
        if (isBadName(name))
            return res.status(400).json({ error: "EINVAL", message: "Invalid directory name" });

        const user = req.user as User | undefined;
        if (!user) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        try{
            const parent= await fileRepo.findOne({
                where:{ino:parentIno},
                relations:["owner", "group", "paths"],
            }) as File;
            if (!parent){
                if (!parent) return res.status(404).json({ error: "ENOENT", message: `Parent inode ${parentIno} not found` });
            }
            if (parent.type !== 1) {
                return res.status(400).json({ error: "ENOTDIR", message: "Parent is not a directory" });
            }
            if(!has_permissions(parent,1,user)){
                return res.status(403).json({ error: "EACCES", message: `No permission to remove in ${parentIno}` });
            }
            const childDbPath = parent.paths[0].path === "/" ? `/${name}` : `${parent.paths[0].path}/${name}`;
            const childFsPath = toFsPath(childDbPath);
            const child=await fileRepo.findOne({
                where: {paths: {path: childDbPath}},
                relations:["owner","group","paths"],
            })as File | null;
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
            
            await pathRepo.remove(child.paths.find(p=>p.path===childDbPath) as Path);
            const remainingPaths = await pathRepo.find({ where: { file: child } });
            
            if (remainingPaths.length < 1)
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
        const oldParentIno=parseIno(req.params.oldParentIno);
        const oldName=req.params.oldName;

        const {newParentIno, newName} =req.body ?? {};
        const newParentInode=parseIno(newParentIno);
        if(!oldParentIno || !newParentInode){
            return res.status(400).json({ error: "EINVAL", message: "Invalid parent inode(s)" });
        }

        if(isBadName(oldName) || isBadName(newName)){
            return res.status(400).json({ error: "EINVAL", message: "Invalid name(s)" });
        }

        const user=req.user as User;

        try{
            const[oldParent,newParent]=await Promise.all([
                fileRepo.findOne({ where: { ino: oldParentIno }, relations: ["owner", "group", "paths"] }),
                fileRepo.findOne({ where: { ino: newParentInode }, relations: ["owner", "group", "paths"] }),
            ]);
            console.log("oldParent", oldParent);
            console.log("newParent", newParent);

            if (!oldParent) 
                return res.status(404).json({ error: "ENOENT", message: "Old parent not found" });
            if (!newParent) 
                return res.status(404).json({ error: "ENOENT", message: "New parent not found" });

            if (oldParent.type !== 1 || newParent.type !== 1) 
                return res.status(400).json({ error: "ENOTDIR", message: "Parent(s) must be directories" });
            
            if(!has_permissions(oldParent,1,user) || !has_permissions(newParent,1,user)){
                return res.status(403).json({ error: "EACCES", message: `Insufficient permissions` });
            }

            const oldPath = childPathOf(oldParent.paths[0].path, oldName);
            const newPath = childPathOf(newParent.paths[0].path, newName);
            const fullOld = toFsPath(oldPath);
            const fullNew = toFsPath(newPath);

            const entry = await fileRepo.findOne({ where: { paths: {path: oldPath }}, relations: ["owner", "group", "paths"] }) as File | null;
            if (!entry) 
                return res.status(404).json({ error: "ENOENT", message: "Source entry not found" });
            console.log("entry trovata:", entry);
            try{
                await fs.rename(fullOld,fullNew);
            }catch(err:any){
                if (err?.code === "ENOENT")   
                    return res.status(404).json({ error: "ENOENT", message: "Source or target dir missing" });
                if (err?.code === "EEXIST")   
                    return res.status(409).json({ error: "EEXIST", message: "Target exists" });
                throw err;
            }
            const pathObj = entry.paths.find(p => p.path === oldPath);
            if (!pathObj) 
                return res.status(500).json({ error: "EIO", message: "Path data not found, manual cleanup required" });
            
            const newPathObj = {
                path: newPath,
                file: entry
            } as Path;
            await pathRepo.remove(pathObj);
            await pathRepo.save(newPathObj);
            const stats = await fs.lstat(fullNew,{bigint:true});
            return res.status(200).json(toEntryJson(entry, stats, newPathObj));
        }catch(err:any){
            return res.status(500).json({ error: "EIO", message: "Not possible to rename", details: String(err?.message ?? err) });
        }
    }

    public hardlink = async (req: Request, res: Response) => {
        const targetIno = parseIno(req.params.targetIno);
        const dirLinkIno = parseIno(req.body.linkParentIno);
        const linkName = (req.body.linkName) as string | "";

        if(!dirLinkIno){
            console.log("Missing link parent inode");
            return res.status(400).json({ error: "EINVAL", message: "Parent link missing" });
        }
        if(!targetIno)
            return res.status(400).json({ error: "EINVAL", message: "Target inode missing" });
        if (isBadName(linkName))
            return res.status(400).json({ error: "EINVAL", message: "Invalid link name" });

        const user = req.user as User | undefined;
        if (!user) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        try{
            
            const target = await fileRepo.findOne({
                where:{ ino: targetIno },
                relations:["owner", "group", "paths"]
            }) as File | null;
            const dirLink = await fileRepo.findOne({
                where:{ ino: dirLinkIno },
                relations:["owner", "group", "paths"]
            }) as File | null;

            if (!target)
                return res.status(404).json({ error: "ENOENT", message: `Target file ${targetIno} not found` });
            if (target.type === 1){
                console.log("Target is a directory");
                return res.status(400).json({ error: "EISDIR", message: "Cannot create a hard link of a directory" });
            }
            if (!dirLink)
                return res.status(404).json({ error: "ENOENT", message: `Link parent inode ${dirLinkIno} not found` });
            if (dirLink.type !== 1) {
                console.log("Link parent is not a directory");
                return res.status(400).json({ error: "ENOTDIR", message: "Link parent is not a directory" });
            }


            if(!has_permissions(dirLink,1,user)){
                console.log("No permission to create in the link parent");
                return res.status(403).json({ error: "EACCES", message: `No permission to create in ${dirLink.paths[0].path}` });
            }
            
            const targetFsPath = toFsPath(target.paths[0].path); // every path is valid, so we can take the first one
            const linkDbPath = childPathOf(dirLink.paths[0].path, linkName);
            const linkFsPath = toFsPath(linkDbPath);

            await fs.link(targetFsPath, linkFsPath);
            const stats = await fs.lstat(linkFsPath,{bigint:true});

            const linkPathObj = {
                file: target,
                path: linkDbPath
            } as Path;
            await pathRepo.save(linkPathObj);

            return res.status(200).json(toEntryJson(target, stats, linkPathObj));

        }
        catch(err:any){
            return res.status(500).json({ error: "EIO", message: "Not possible to create the hard link", details: String(err?.message ?? err) });
        }

    }

    public symlink = async (req: Request, res: Response) => {
        let targetPath = (req.body.targetPath) as string | "";
        const dirLinkIno = parseIno(req.body.linkParentIno);
        const linkName = (req.body.linkName) as string | "";

        console.log("Symlink", {targetPath, dirLinkIno, linkName});
        if(!dirLinkIno){
            console.log("Missing link parent inode");
            return res.status(400).json({ error: "EINVAL", message: "Parent link missing" });
        }
        if(!targetPath)
            return res.status(400).json({ error: "EINVAL", message: "Target path missing" });
        if (isBadName(linkName))
            return res.status(400).json({ error: "EINVAL", message: "Invalid link name" });

        const user = req.user as User | undefined;
        if (!user) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }

        try{
            
            const dirLink = await fileRepo.findOne({
                where:{ ino: dirLinkIno },
                relations:["owner", "group", "paths"]
            }) as File | null;

            if (!dirLink)
                return res.status(404).json({ error: "ENOENT", message: `Link parent inode ${dirLinkIno} not found` });
            if (dirLink.type !== 1) {
                console.log("Link parent is not a directory");
                return res.status(400).json({ error: "ENOTDIR", message: "Link parent is not a directory" });
            }


            if(!has_permissions(dirLink,1,user)){
                console.log("No permission to create in the link parent");
                return res.status(403).json({ error: "EACCES", message: `No permission to create in ${dirLink.paths[0].path}` });
            }

            const linkDbPath = childPathOf(dirLink.paths[0].path, linkName);
            const linkFsPath = toFsPath(linkDbPath);

            await fs.symlink(targetPath, linkFsPath);
            const stats = await fs.lstat(linkFsPath,{bigint:true});

            const link = {
                ino: stats.ino.toString(),
                owner: user,
                group: dirLink.group,
                type: 2,
                permissions: 0o755,
            } as File;
            await fileRepo.save(link);

            const linkPathObj = {
                file: link,
                path: linkDbPath
            } as Path;
            await pathRepo.save(linkPathObj);
            
            const linkStats = await fs.lstat(linkFsPath,{bigint:true});

            return res.status(200).json(toEntryJson(link, linkStats, linkPathObj));

        }
        catch(err:any){
            return res.status(500).json({ error: "EIO", message: "Not possible to create the symlink", details: String(err?.message ?? err) });
        }

    }

    public readlink = async (req: Request, res: Response) => {
        const linkIno = parseIno(req.params.ino);

        if(!linkIno){
            return res.status(400).json({ error: "EINVAL", message: "Link inode missing" });
        }

        const user = req.user as User | undefined;
        if (!user) {
            return res.status(500).json({ error: 'Not possible to retreive user data' });
        }
        
        try{

            const slink = await fileRepo.findOne({
                where:{ ino: linkIno },
                relations:["owner", "group", "paths"]
            }) as File | null;
            
            if (!slink)
                return res.status(404).json({ error: "ENOENT", message: `Link file ${linkIno} not found` });
            if (slink.type !== 2){
                console.log("File is not a symlink");
                return res.status(400).json({ error: "EINVAL", message: "File is not a symlink" });
            }
            
            const linkFsPath = toFsPath(slink.paths[0].path); // every path is valid, so we can take the first one
            const target = await fs.readlink(linkFsPath);
            console.log("READLINK: Symlink target:", target);

            return res.status(200).json({ target });

        }
        catch(err:any){
            return res.status(500).json({ error: "EIO", message: "Not possible to read the symlink", details: String(err?.message ?? err) });
        }

    }
}