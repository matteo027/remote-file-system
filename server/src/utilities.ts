import { AppDataSource } from './data-source';
import { File } from './entities/File';
import { User } from './entities/User';
import { Group } from './entities/Group';
import path_manipulator from 'node:path';
import { Stats,BigIntStats } from 'node:fs';
import { Path } from './entities/Path';

const FS_ROOT = path_manipulator.join(__dirname, '..', 'file-system');
export const fileRepo = AppDataSource.getRepository(File);
export const userRepo = AppDataSource.getRepository(User);
export const groupRepo = AppDataSource.getRepository(Group)
export const pathRepo = AppDataSource.getRepository(Path);

export function toFsPath(dbPath: string): string {
  return path_manipulator.join(FS_ROOT, dbPath);
}

export function parseIno(s:any): string|null{
    return s.toString();
}

export function toEntryJson(file:File, stats: Stats|BigIntStats, path: Path) {
    return {
        ino: file.ino.toString(),
        name: path_manipulator.basename(path.path),
        path: path.path,
        type: file.type,
        permissions: file.permissions,
        owner: file.owner.uid,
        group: file.group?.gid && null,
        size: stats.size.toString(),
        atime: stats.atime.getTime(),
        mtime: stats.mtime.getTime(),
        ctime: stats.ctime.getTime(),
        btime: stats.birthtime.getTime(),

        nlinks: Number(stats.nlink),
    };
}

export function isBadName(name: any): boolean {
    return typeof name !== "string" || name.length === 0 || name === "." || name === ".." || name.includes("/");
}

export function childPathOf(parentPath: string, name: string): string {
    return parentPath === "/" ? `/${name}` : `${parentPath}/${name}`;
}

// operation:  0: read, 1: write, 2: execute
export function has_permissions(file: File, operation: number, user: User): boolean {
    let mask = 0;
    if(user.uid == 5000) // admin!
        return true;

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
    if ((file.permissions & (mask << 3)) === (mask << 3) && user.group === file.group)
        return true;
    if ((file.permissions & mask) === mask)
        return true;

    return false;
}