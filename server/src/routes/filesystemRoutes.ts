import { Router } from 'express';
import { FileSystemController } from '../controllers/filesystemController';
import { Express } from 'express-serve-static-core';
import { AuthenticationController } from '../controllers/authenticationController';

const router = Router();
const filesystemController = new FileSystemController();
const isLoggedIn = (new AuthenticationController).isLoggedIn;

export function setRoutes(app: Express) {
    app.use('/', router);

    router.get('/api/directories', isLoggedIn, filesystemController.readdir);

    router.post('/api/directories/:name', isLoggedIn, filesystemController.mkdir);
    router.delete('/api/directories/:name', isLoggedIn, filesystemController.rmdir);

    router.post('/api/files/:name', isLoggedIn, filesystemController.create);
    router.put('/api/files/:name', isLoggedIn, filesystemController.write);
    router.get('/api/files/:name', isLoggedIn, filesystemController.open);
    router.delete('/api/files/:name', isLoggedIn, filesystemController.unlink);

    router.put('/api/:name', isLoggedIn, filesystemController.rename); // rename
    router.put('/api/mod/:name', isLoggedIn, filesystemController.setattr);

    
}