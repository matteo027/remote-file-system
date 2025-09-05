import { AppDataSource } from '../data-source';
import { File } from '../entities/File';
import { User } from '../entities/User';
import { Group } from '../entities/Group';
import path_manipulator from 'node:path';

const FS_ROOT = path_manipulator.join(__dirname, '..', '..', 'file-system');
export const fileRepo = AppDataSource.getRepository(File);
export const userRepo = AppDataSource.getRepository(User);
export const groupRepo = AppDataSource.getRepository(Group);

export function toFsPath(dbPath: string): string {
  return path_manipulator.join(FS_ROOT, dbPath);
}

// operation:  0: read, 1: write, 2: execute
export function has_permissions(file: File, operation: number, user: User): boolean {
    let mask = 0;
    if(user.uid == 5000) // admin!
        return true;

    switch (operation) {
        case 0:
            mask = 0o4;
            if(file.path == '/') // can alwas read the root
                return true;
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