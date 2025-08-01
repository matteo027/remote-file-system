import { Router } from 'express';
import { FileSystemController } from '../controllers/filesystemController';
import { Express } from 'express-serve-static-core';
import passport from 'passport';
import { AuthenticationController } from '../controllers/authenticationController';

const router = Router();
const authenticationController = new AuthenticationController();

export function setRoutes(app: Express) {

    app.use('/', router);
    
    router.post('/api/login', passport.authenticate('local'), authenticationController.login);
    router.post('/api/signup', authenticationController.signup);
    router.post('/api/logout', authenticationController.logout);
    router.get('/api/me', authenticationController.isLoggedIn, authenticationController.logged);
    
}