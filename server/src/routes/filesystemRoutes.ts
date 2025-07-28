import { Router } from 'express';
import { FileSystemController } from '../controllers/filesystemController';
import { Express } from 'express-serve-static-core';
import { AuthenticationController } from '../controllers/authenticationController';

const router = Router();
const filesystemController = new FileSystemController();
const isLoggedIn = (new AuthenticationController).isLoggedIn;

export function setRoutes(app: Express) {
    app.use('/', router);

    router.get('/api/directories/*', isLoggedIn, filesystemController.readdir);

    router.post('/api/directories/*', isLoggedIn, filesystemController.mkdir);
    router.delete('/api/directories/*', isLoggedIn, filesystemController.rmdir);

    router.post('/api/files/*', isLoggedIn, filesystemController.create);
    router.put('/api/files/*', isLoggedIn, filesystemController.write);
    router.get('/api/files/*', isLoggedIn, filesystemController.open);
    router.delete('/api/files/*', isLoggedIn, filesystemController.unlink);

    router.put('/api/files/*', isLoggedIn, filesystemController.rename); // rename
    router.put('/api/mod/*', isLoggedIn, filesystemController.setattr);

    
}