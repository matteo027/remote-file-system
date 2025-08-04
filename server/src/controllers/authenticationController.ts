import { Request, Response } from 'express';
import * as crypto from 'node:crypto';
import { promisify } from 'util';
import { User } from '../entities/User';
import { AppDataSource } from '../data-source';
import { promises as fs } from 'node:fs';
import { File } from '../entities/File';

const scryptAsync = promisify(crypto.scrypt);

export class AuthenticationController {

    // login
    public login = async (req: Request, res: Response) => {
        res.json((req.user as User)?.uid);
    }
    // signup
    public signup = async (req: Request, res: Response) => {
        const { uid, password } = req.body;
        const userRepo = AppDataSource.getRepository(User);
        const fileRepo = AppDataSource.getRepository(File);

        const exists = await userRepo.findOneBy({ uid });
        if (exists) {
            return res.status(400).json({ message: "User already exists" });
        }

        const salt = crypto.randomBytes(16).toString('hex');
        const hashedPassword = (await scryptAsync(password, salt, 32) as Buffer).toString('hex');

        const user = userRepo.create({ uid, password: hashedPassword, salt });
        await userRepo.save(user);

        // directory for the new user
        await fs.mkdir(`./file-system/${uid}`, { recursive: true });

        const now = Date.now();
        const userDir = fileRepo.create({
            path: `/${uid}`,
            owner: user,
            type: 1,
            permissions: 0o755,
            atime: now,
            btime: now,
            ctime: now,
            mtime: now,
        });
        await fileRepo.save(userDir);

        // clearing the file create-user
        await fs.writeFile('./file-system/create-user.txt', '');

        req.login(user, (err) => {
            if (err) return res.status(500).json({ message: "Signup succeeded, but login failed" });
            return res.status(201).json({ uid });
        });
    }
    // logout
    public logout = async (req: Request, res: Response) => {
        req.logout(() => {
            res.end();
        });
    }
    public logged = async (req: Request, res: Response) => {
        res.json(req.user as User);
    }

    public getUser = async (uid: number, password: string) => {
        return new Promise(async (res, rej) => {
            const user: User | null = await AppDataSource.getRepository(User).findOneBy({ uid });

            const hashedPassword = await scryptAsync(password, user?.salt || "", 32) as Buffer;

            if (Buffer.from(user?.password || "", 'hex').length !== hashedPassword.length) {
                return res(false);
            }

            if (crypto.timingSafeEqual(Buffer.from(user?.password || "", 'hex'), hashedPassword))
                res({ uid: user?.uid });
            else res(false);
        })
    };

    public isLoggedIn = (req: Request, res: Response, next: () => any) => {
        if (req.isAuthenticated())
            return next();

        return res.status(401).json({ message: "not authenticated" });
    }


}