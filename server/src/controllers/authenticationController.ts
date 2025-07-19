import { Request, Response } from 'express';
import * as crypto from 'crypto';
import { promisify } from 'util';
import { User } from '../entities/User';
import { AppDataSource } from '../data-source';

const scryptAsync = promisify(crypto.scrypt);

export class AuthenticationController {

    // login
    public login = async (req: Request, res: Response) => {
        const { username, password } = req.body;

        try {
            const result = await this.getUser(username, password);
            if (result) {
                res.status(200).json({ username: (result as User).username });
            } else {
                res.status(401).json({ error: 'Invalid credentials' });
            }
        } catch (error) {
            res.status(500).json({ error: 'Internal server error' });
        }
    }

    public getUser = async (username: string, password: string) => {
        return new Promise(async (res, rej) => {
            const user: User | null = await AppDataSource.getRepository(User).findOneBy({ username });

            const hashedPassword = await scryptAsync(password, user?.salt || "", 32) as Buffer;

            if(crypto.timingSafeEqual(Buffer.from(user?.password || "", 'hex'), hashedPassword))
                res(user);
            else res(false);
        })
    };

    public isLoggedIn = (req: Request, res: Response, next: () => any) => {
        if(req.isAuthenticated())
            return next();

        return res.status(400).json({ message: "not authenticated" });
    }


}