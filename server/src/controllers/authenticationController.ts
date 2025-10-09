import { Request, Response } from 'express';
import * as crypto from 'node:crypto';
import { promisify } from 'util';
import { User } from '../entities/User';
import { AppDataSource } from '../data-source';
import { promises as fs } from 'node:fs';
import { File } from '../entities/File';
import { Group } from '../entities/Group';
import { pathRepo } from '../utilities';

const scryptAsync = promisify(crypto.scrypt);

export class AuthenticationController {

    // login
    public login = async (req: Request, res: Response) => {
        console.log("[login] called with user:", (req.user as User)?.uid);
        res.json((req.user as User)?.uid);
    }

    // signup
    public signup = async (req: Request, res: Response) => {
        console.log("[signup] called with uid:", req.body?.uid);
        const { uid, password } = req.body;
        const userRepo = AppDataSource.getRepository(User);
        const fileRepo = AppDataSource.getRepository(File);

        const exists = await userRepo.findOneBy({ uid });
        if (exists) {
            console.log("[signup] status 400: User already exists");
            return res.status(400).json({ message: "User already exists" });
        }
        console.log("[signup] User does not exist, creating...");

        const salt = crypto.randomBytes(16).toString('hex');
        const hashedPassword = (await scryptAsync(password, salt, 32) as Buffer).toString('hex');

        const user = userRepo.create({ uid, password: hashedPassword, salt });
        await userRepo.save(user);
        console.log("[signup] User saved in database");

        // directory for the new user
        await fs.mkdir(`./file-system/${uid}`, { recursive: true });
        const ino = (await fs.lstat(`./file-system/${uid}`, { bigint: true })).ino;
        console.log("[signup] User directory created in file system");

        const userDir = fileRepo.create({
            ino: ino.toString(),
            owner: user,
            type: 1,
            permissions: 0o755
        });
        await fileRepo.save(userDir);
        console.log("[signup] User directory file entry saved in database");
        const userPath = pathRepo.create({
            file: userDir,
            path: `/${uid}`
        });
        await pathRepo.save(userPath);
        console.log("[signup] User directory path saved in database");

        // clearing the file create-user
        await fs.writeFile('./file-system/create-user.txt', 'User successfully created');
        console.log("[signup] status 200: User created");
        return res.status(200).json({ message: "User created" });
    }

    // logout
    public logout = async (req: Request, res: Response) => {
        console.log("[logout] called for user:", (req.user as User)?.uid);
        req.logout(() => {
            console.log("[logout] status 200: User logged out");
            res.end();
        });
    }

    public logged = async (req: Request, res: Response) => {
        console.log("[logged] called for user:", (req.user as User)?.uid);
        res.json(req.user as User);
    }

    public getUser = async (uid: number, password: string) => {
        console.log("[getUser] called with uid:", uid);
        return new Promise(async (res, rej) => {
            const user: User | null = await AppDataSource.getRepository(User).findOneBy({ uid });

            const hashedPassword = await scryptAsync(password, user?.salt || "", 32) as Buffer;

            if (Buffer.from(user?.password || "", 'hex').length !== hashedPassword.length) {
                console.log("[getUser] password length mismatch");
                return res(false);
            }

            if (crypto.timingSafeEqual(Buffer.from(user?.password || "", 'hex'), hashedPassword)) {
                console.log("[getUser] password match");
                res({ uid: user?.uid });
            } else {
                console.log("[getUser] password mismatch");
                res(false);
            }
        })
    };

    public isLoggedIn = (req: Request, res: Response, next: () => any) => {
        console.log("[isLoggedIn] called for user:", (req.user as User)?.uid);
        if (req.isAuthenticated()) {
            console.log("[isLoggedIn] status 200: Authenticated");
            return next();
        }
        console.log("[isLoggedIn] status 401: Not authenticated");
        return res.status(401).json({ message: "Not authenticated" });
    }

    // new group
    public newgroup = async (req: Request, res: Response) => {
        console.log("[newgroup] called with uid:", req.body?.uid, "gid:", req.body?.gid);
        const { uid, gid } = req.body;
        const userRepo = AppDataSource.getRepository(User);
        const groupRepo = AppDataSource.getRepository(Group);

        const user = await userRepo.findOne({ where: { uid } });
        if (!user) {
            console.log("[newgroup] status 404: User does not exist");
            return res.status(404).json({ message: "User does not exist" });
        }

        let group = await groupRepo.findOne({ where: { gid } });
        if (!group) {
            group = groupRepo.create({ gid, users: [] });
        }
        if (!Array.isArray(group.users)) {
            group.users = [];
        }
        const alreadyInGroup = group.users.some(u => u.uid === user.uid);
        if (!alreadyInGroup) {
            group.users.push(user);
        }
        await groupRepo.save(group);

        user.group = group;
        await userRepo.save(user);

        // clearing the file create-group
        await fs.writeFile('./file-system/create-group.txt', 'Group successfully created');
        console.log("[newgroup] status 200: Group created");
        return res.status(200).end();
    }
}