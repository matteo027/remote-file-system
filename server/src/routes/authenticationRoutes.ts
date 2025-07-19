import { Router } from 'express';
import { FileSystemController } from '../controllers/filesystemController';
import { Express } from 'express-serve-static-core';
import passport from 'passport';
import { AuthenticationController } from '../controllers/authenticationController';

const router = Router();
const authenticationController = new AuthenticationController();

export function setRoutes(app: Express) {
    // login
    router.post('/api/login', passport.authenticate('local'), authenticationController.login);

    //router.get('/api/user', authenticationController.get);
}