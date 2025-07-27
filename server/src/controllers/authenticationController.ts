import { Request, Response } from 'express';
import * as crypto from 'node:crypto';
import { promisify } from 'util';
import { User } from '../entities/User';
import { AppDataSource } from '../data-source';

const scryptAsync = promisify(crypto.scrypt);

export class AuthenticationController {

    // login
    public login = async (req: Request, res: Response) => {
        res.json((req.user as User)?.username);
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

    public getUser = async (username: string, password: string) => {
        return new Promise(async (res, rej) => {
            const user: User | null = await AppDataSource.getRepository(User).findOneBy({ username });

            const hashedPassword = await scryptAsync(password, user?.salt || "", 32) as Buffer;

            if (crypto.timingSafeEqual(Buffer.from(user?.password || "", 'hex'), hashedPassword))
                res({ username: user?.username });
            else res(false);
        })
    };

    public isLoggedIn = (req: Request, res: Response, next: () => any) => {
        if (req.isAuthenticated())
            return next();

        return res.status(401).json({ message: "not authenticated" });
    }


}